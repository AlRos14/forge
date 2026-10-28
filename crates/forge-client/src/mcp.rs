use anyhow::{anyhow, Context, Result};
use clap::Subcommand;
use serde_json::{Map, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::io::Write;

use crate::auth::resolve_access_token_for_server;

#[derive(clap::Args)]
pub struct McpArgs {
    #[command(subcommand)]
    cmd: McpCmd,
}

#[derive(Subcommand)]
pub enum McpCmd {
    Install {
        #[arg(long, value_enum, default_value = "claude")]
        agent: AgentTarget,
        #[arg(long, value_enum, default_value = "project")]
        scope: ConfigScope,
        /// User access token for `user` scope. Project/local scopes require an environment reference.
        #[arg(long)]
        token: Option<String>,
        /// Environment variable the client expands at connection time. Required for project/local
        /// scopes unless FORGE_TOKEN is already set.
        #[arg(long)]
        token_env: Option<String>,
        /// Scope this MCP endpoint to a single Forge project.
        #[arg(long)]
        project_id: Option<String>,
    },
    Uninstall {
        #[arg(long, value_enum, default_value = "claude")]
        agent: AgentTarget,
        #[arg(long, value_enum, default_value = "project")]
        scope: ConfigScope,
    },
    Status {
        #[arg(long, value_enum, default_value = "claude")]
        agent: AgentTarget,
        #[arg(long, value_enum, default_value = "project")]
        scope: ConfigScope,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AgentTarget {
    Claude,
    Codex,
    Cursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ConfigScope {
    Project,
    Local,
    User,
}

impl McpArgs {
    pub async fn run(&self, server: &str) -> Result<()> {
        match &self.cmd {
            McpCmd::Install {
                agent,
                scope,
                token,
                token_env,
                project_id,
            } => install(
                server,
                *agent,
                *scope,
                token.as_deref(),
                token_env.as_deref(),
                project_id.as_deref(),
            ),
            McpCmd::Uninstall { agent, scope } => uninstall(*agent, *scope),
            McpCmd::Status { agent, scope } => status(*agent, *scope),
        }
    }
}

pub fn config_path(agent: AgentTarget, scope: ConfigScope) -> Result<PathBuf> {
    let relative = config_relative_path(agent, scope);

    match scope {
        ConfigScope::Project | ConfigScope::Local => env::current_dir()
            .map(|cwd| cwd.join(relative))
            .context("failed to resolve current directory"),
        ConfigScope::User => env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(relative))
            .ok_or_else(|| anyhow!("HOME is not set")),
    }
}

fn config_relative_path(agent: AgentTarget, scope: ConfigScope) -> PathBuf {
    match (agent, scope) {
        (AgentTarget::Claude, ConfigScope::Project) => PathBuf::from(".claude/settings.json"),
        (AgentTarget::Claude, ConfigScope::Local) => PathBuf::from(".claude/settings.local.json"),
        (AgentTarget::Claude, ConfigScope::User) => PathBuf::from(".claude/settings.json"),
        (AgentTarget::Codex, ConfigScope::Project | ConfigScope::Local) => {
            PathBuf::from(".codex/config.toml")
        }
        (AgentTarget::Codex, ConfigScope::User) => PathBuf::from(".codex/config.toml"),
        (AgentTarget::Cursor, ConfigScope::Project | ConfigScope::Local) => {
            PathBuf::from(".cursor/mcp.json")
        }
        (AgentTarget::Cursor, ConfigScope::User) => PathBuf::from(".cursor/mcp.json"),
    }
}

pub fn read_config(path: &Path) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let value: Value = serde_json::from_str(&contents)
                .with_context(|| format!("failed to parse JSON at {}", path.display()))?;
            if value.is_object() {
                Ok(value)
            } else {
                Err(anyhow!("config at {} is not a JSON object", path.display()))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Object(Map::new())),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub fn write_config(path: &Path, config: &Value) -> Result<()> {
    if !config.is_object() {
        return Err(anyhow!("config at {} is not a JSON object", path.display()));
    }

    let mut body = serde_json::to_string_pretty(config)
        .with_context(|| format!("failed to serialize {}", path.display()))?;
    body.push('\n');
    write_private_file(path, body.as_bytes())
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("failed to write {}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
        file.write_all(contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(())
}

pub fn install(
    server: &str,
    agent: AgentTarget,
    scope: ConfigScope,
    token: Option<&str>,
    token_env: Option<&str>,
    project_id: Option<&str>,
) -> Result<()> {
    let path = config_path(agent, scope)?;
    let auth = resolve_install_auth(server, scope, token, token_env)?;
    let install_url = install_url(server, project_id)?;
    if agent == AgentTarget::Codex {
        let contents = read_codex_config(&path)?;
        let next = match &auth {
            InstallAuth::Literal(access_token) => {
                set_codex_forge_config(&contents, &install_url, access_token)
            }
            InstallAuth::Env(name) => set_codex_forge_env_config(&contents, &install_url, name),
        };
        write_codex_config_if_changed(&path, &contents, next)?;
        println!("Installed Forge MCP at {}", path.display());
        return Ok(());
    }

    let mut config = read_config(&path)?;
    let root = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("config at {} is not a JSON object", path.display()))?;

    match root.get("mcpServers") {
        Some(Value::Object(_)) => {}
        Some(Value::Null) | None => {
            root.insert("mcpServers".to_owned(), Value::Object(Map::new()));
        }
        Some(_) => {
            return Err(anyhow!(
                "mcpServers in {} is not a JSON object",
                path.display()
            ));
        }
    }

    let mcp_servers = root
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("mcpServers in {} is not a JSON object", path.display()))?;

    let authorization = match &auth {
        InstallAuth::Literal(access_token) => format!("Bearer {access_token}"),
        InstallAuth::Env(name) => match agent {
            AgentTarget::Claude => format!("Bearer ${{{name}}}"),
            AgentTarget::Cursor => format!("Bearer ${{env:{name}}}"),
            AgentTarget::Codex => unreachable!("Codex uses TOML bearer_token_env_var"),
        },
    };
    let mut forge = Map::new();
    forge.insert("type".to_owned(), Value::String("http".to_string()));
    forge.insert("url".to_owned(), Value::String(install_url));
    forge.insert(
        "headers".to_owned(),
        serde_json::json!({ "Authorization": authorization }),
    );
    mcp_servers.insert("forge".to_owned(), Value::Object(forge));

    write_config(&path, &config)?;
    println!("Installed Forge MCP at {}", path.display());
    Ok(())
}

enum InstallAuth {
    Literal(String),
    Env(String),
}

fn resolve_install_auth(
    server: &str,
    scope: ConfigScope,
    token: Option<&str>,
    token_env: Option<&str>,
) -> Result<InstallAuth> {
    if let Some(name) = token_env.map(str::trim).filter(|value| !value.is_empty()) {
        validate_env_name(name)?;
        require_env_value(name)?;
        return Ok(InstallAuth::Env(name.to_owned()));
    }
    if scope != ConfigScope::User {
        if token.is_some() {
            return Err(anyhow!(
                "--token cannot be embedded in a project/local MCP config; export FORGE_TOKEN or pass --token-env NAME"
            ));
        }
        require_env_value("FORGE_TOKEN")?;
        return Ok(InstallAuth::Env("FORGE_TOKEN".to_owned()));
    }
    let access_token = resolve_install_token(server, token)?;
    Ok(InstallAuth::Literal(access_token))
}

fn validate_env_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(anyhow!("token environment variable name must not be empty"));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|value| value.is_ascii_alphanumeric() || value == '_')
    {
        return Err(anyhow!(
            "token environment variable name must contain only letters, digits, and underscores"
        ));
    }
    Ok(())
}

fn require_env_value(name: &str) -> Result<()> {
    if env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Err(anyhow!(
            "environment variable {name} is not set; project/local MCP configs reference the token instead of embedding it"
        ));
    }
    Ok(())
}

pub fn uninstall(agent: AgentTarget, scope: ConfigScope) -> Result<()> {
    let path = config_path(agent, scope)?;
    if agent == AgentTarget::Codex {
        let contents = read_codex_config(&path)?;
        write_codex_config_if_changed(&path, &contents, remove_codex_forge(&contents))?;
        println!("Removed Forge MCP from {}", path.display());
        return Ok(());
    }

    let mut config = read_config(&path)?;
    let root = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("config at {} is not a JSON object", path.display()))?;

    let Some(mcp_servers) = root.get_mut("mcpServers") else {
        println!("Forge MCP is not installed at {}", path.display());
        return Ok(());
    };

    let mcp_servers = mcp_servers
        .as_object_mut()
        .ok_or_else(|| anyhow!("mcpServers in {} is not a JSON object", path.display()))?;

    if mcp_servers.remove("forge").is_none() {
        println!("Forge MCP is not installed at {}", path.display());
        return Ok(());
    }

    write_config(&path, &config)?;
    println!("Removed Forge MCP from {}", path.display());
    Ok(())
}

pub fn status(agent: AgentTarget, scope: ConfigScope) -> Result<()> {
    let path = config_path(agent, scope)?;
    let url = if agent == AgentTarget::Codex {
        codex_forge_url(&read_codex_config(&path)?)
    } else {
        let config = read_config(&path)?;
        forge_url(&config, &path)?
    };

    match url {
        Some(url) => println!("{url}"),
        None => println!("Not installed: {}", path.display()),
    }

    Ok(())
}

fn forge_url(config: &Value, path: &Path) -> Result<Option<String>> {
    let root = config
        .as_object()
        .ok_or_else(|| anyhow!("config at {} is not a JSON object", path.display()))?;

    let Some(mcp_servers) = root.get("mcpServers") else {
        return Ok(None);
    };

    let mcp_servers = match mcp_servers {
        Value::Null => return Ok(None),
        Value::Object(mcp_servers) => mcp_servers,
        _ => {
            return Err(anyhow!(
                "mcpServers in {} is not a JSON object",
                path.display()
            ));
        }
    };

    let Some(forge) = mcp_servers.get("forge") else {
        return Ok(None);
    };

    let forge = match forge {
        Value::Null => return Ok(None),
        Value::Object(forge) => forge,
        _ => {
            return Err(anyhow!(
                "mcpServers.forge in {} is not a JSON object",
                path.display()
            ));
        }
    };

    match forge.get("url") {
        Some(Value::String(url)) => Ok(Some(url.clone())),
        Some(_) => Err(anyhow!(
            "mcpServers.forge.url in {} is not a string",
            path.display()
        )),
        None => Ok(None),
    }
}

fn mcp_url(server: &str) -> String {
    format!("{}/mcp", server.trim_end_matches('/'))
}

fn resolve_install_token(server: &str, token: Option<&str>) -> Result<String> {
    resolve_access_token_for_server(server, token)?.ok_or_else(|| {
        anyhow!(
            "MCP install requires an access token; run `forge-ctl login`, pass --token, or set FORGE_TOKEN"
        )
    })
}

fn install_url(server: &str, project_id: Option<&str>) -> Result<String> {
    let mut url = url::Url::parse(&mcp_url(server)).context("parse Forge MCP URL")?;
    if let Some(project_id) = project_id.map(str::trim).filter(|value| !value.is_empty()) {
        url.query_pairs_mut().append_pair("project_id", project_id);
    }
    Ok(url.to_string())
}

fn read_codex_config(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn write_codex_config_if_changed(path: &Path, previous: &str, next: String) -> Result<()> {
    if next != previous {
        write_private_file(path, next.as_bytes())?;
    }
    Ok(())
}

fn codex_forge_url(contents: &str) -> Option<String> {
    codex_forge_block(contents).and_then(|block| {
        block.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            if key.trim() != "url" {
                return None;
            }
            Some(unquote_toml_string(value.trim()))
        })
    })
}

fn set_codex_forge_config(contents: &str, url: &str, token: &str) -> String {
    let mut next = remove_codex_forge(contents);
    if !next.ends_with('\n') && !next.is_empty() {
        next.push('\n');
    }
    if !next.is_empty() {
        next.push('\n');
    }
    next.push_str("[mcp_servers.forge]\n");
    next.push_str("url = ");
    next.push_str(&toml_string(url));
    next.push('\n');
    next.push_str("http_headers = { Authorization = ");
    next.push_str(&toml_string(&format!("Bearer {token}")));
    next.push_str(" }\n");
    next
}

fn set_codex_forge_env_config(contents: &str, url: &str, token_env: &str) -> String {
    let mut next = remove_codex_forge(contents);
    if !next.ends_with('\n') && !next.is_empty() {
        next.push('\n');
    }
    if !next.is_empty() {
        next.push('\n');
    }
    next.push_str("[mcp_servers.forge]\n");
    next.push_str("url = ");
    next.push_str(&toml_string(url));
    next.push('\n');
    next.push_str("bearer_token_env_var = ");
    next.push_str(&toml_string(token_env));
    next.push('\n');
    next
}

fn remove_codex_forge(contents: &str) -> String {
    let Some((start, end)) = codex_forge_block_range(contents) else {
        return contents.to_owned();
    };
    let mut next = String::new();
    next.push_str(&contents[..start]);
    next.push_str(&contents[end..]);
    while next.contains("\n\n\n") {
        next = next.replace("\n\n\n", "\n\n");
    }
    next
}

fn codex_forge_block(contents: &str) -> Option<&str> {
    let (start, end) = codex_forge_block_range(contents)?;
    Some(&contents[start..end])
}

fn codex_forge_block_range(contents: &str) -> Option<(usize, usize)> {
    let mut start = None;
    let mut end = contents.len();

    for (offset, line) in toml_lines(contents) {
        let trimmed = line.trim();
        if trimmed == "[mcp_servers.forge]" {
            start = Some(offset);
            continue;
        }
        if start.is_some()
            && trimmed.starts_with('[')
            && trimmed.ends_with(']')
            && trimmed != "[mcp_servers.forge]"
        {
            end = offset;
            break;
        }
    }

    start.map(|start| (start, end))
}

fn toml_lines(contents: &str) -> impl Iterator<Item = (usize, &str)> {
    std::iter::once(0)
        .chain(contents.match_indices('\n').map(|(offset, _)| offset + 1))
        .filter(|offset| *offset < contents.len())
        .map(|offset| {
            let line = contents[offset..].lines().next().unwrap_or("");
            (offset, line)
        })
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

fn unquote_toml_string(value: &str) -> String {
    serde_json::from_str(value).unwrap_or_else(|_| value.trim_matches('"').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn config_relative_path_uses_shared_project_file_for_claude() {
        assert_eq!(
            config_relative_path(AgentTarget::Claude, ConfigScope::Project),
            PathBuf::from(".claude/settings.json")
        );
        assert_eq!(
            config_relative_path(AgentTarget::Claude, ConfigScope::Local),
            PathBuf::from(".claude/settings.local.json")
        );
        assert_eq!(
            config_relative_path(AgentTarget::Claude, ConfigScope::User),
            PathBuf::from(".claude/settings.json")
        );
    }

    #[test]
    fn config_relative_path_keeps_cursor_local_and_project_identical() {
        assert_eq!(
            config_relative_path(AgentTarget::Cursor, ConfigScope::Project),
            PathBuf::from(".cursor/mcp.json")
        );
        assert_eq!(
            config_relative_path(AgentTarget::Cursor, ConfigScope::Local),
            PathBuf::from(".cursor/mcp.json")
        );
        assert_eq!(
            config_relative_path(AgentTarget::Cursor, ConfigScope::User),
            PathBuf::from(".cursor/mcp.json")
        );
    }

    #[test]
    fn config_relative_path_uses_codex_config_toml() {
        assert_eq!(
            config_relative_path(AgentTarget::Codex, ConfigScope::Project),
            PathBuf::from(".codex/config.toml")
        );
        assert_eq!(
            config_relative_path(AgentTarget::Codex, ConfigScope::User),
            PathBuf::from(".codex/config.toml")
        );
    }

    #[test]
    fn install_writes_http_transport_type() {
        let mut forge = Map::new();
        forge.insert("type".to_owned(), Value::String("http".to_string()));
        forge.insert(
            "url".to_owned(),
            Value::String(mcp_url("http://127.0.0.1:8080")),
        );
        let forge = Value::Object(forge);

        assert_eq!(forge["type"], "http");
        assert_eq!(forge["url"], "http://127.0.0.1:8080/mcp");
    }

    #[test]
    fn codex_forge_config_round_trips_url_and_header() {
        let initial = r#"model = "gpt-5.2"

[mcp_servers.other]
command = "other"
"#;

        let installed = set_codex_forge_config(initial, "http://127.0.0.1:8080/mcp", "fg_test");
        assert_eq!(
            codex_forge_url(&installed).as_deref(),
            Some("http://127.0.0.1:8080/mcp")
        );
        assert!(installed.contains("http_headers = { Authorization = \"Bearer fg_test\" }"));
        assert!(installed.contains("[mcp_servers.other]"));

        let removed = remove_codex_forge(&installed);
        assert_eq!(codex_forge_url(&removed), None);
        assert!(removed.contains("[mcp_servers.other]"));
    }

    #[test]
    fn install_requires_token() {
        let _guard = crate::auth::TEST_ENV_LOCK.lock().expect("test env lock");
        let temp = unique_temp_dir("forge-client-mcp-no-token");
        let _token = EnvVarGuard::unset("FORGE_TOKEN");
        let _data = EnvVarGuard::set("FORGE_DATA_DIR", temp.to_string_lossy().as_ref());

        let error =
            resolve_install_token("http://127.0.0.1:8080/", None).expect_err("token required");

        assert!(error.to_string().contains("requires an access token"));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn install_url_includes_project_but_never_the_token() {
        let url = install_url("http://127.0.0.1:8080/", Some("project 1")).expect("url builds");

        assert_eq!(url, "http://127.0.0.1:8080/mcp?project_id=project+1");
        assert!(!url.contains("token="));
    }

    #[test]
    fn install_token_falls_back_to_forge_token_env() {
        let _guard = crate::auth::TEST_ENV_LOCK.lock().expect("test env lock");
        let temp = unique_temp_dir("forge-client-mcp-env-token");
        let _env = EnvVarGuard::set("FORGE_TOKEN", "fg_from_env");
        let _data = EnvVarGuard::set("FORGE_DATA_DIR", temp.to_string_lossy().as_ref());

        let token = resolve_install_token("http://127.0.0.1:8080/", None).expect("token resolves");

        assert_eq!(token, "fg_from_env");
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn install_token_falls_back_to_stored_login_token() {
        let _guard = crate::auth::TEST_ENV_LOCK.lock().expect("test env lock");
        let temp = unique_temp_dir("forge-client-mcp-stored-token");
        fs::create_dir_all(&temp).expect("forge data dir creates");
        let _token = EnvVarGuard::unset("FORGE_TOKEN");
        let _data = EnvVarGuard::set("FORGE_DATA_DIR", temp.to_string_lossy().as_ref());
        fs::write(
            temp.join("forge_ctl_credentials.json"),
            serde_json::json!({
                "server_url": "http://127.0.0.1:8080",
                "token": "fg_stored",
                "token_id": "token-1",
                "token_prefix": "fg_st",
                "email": "user@example.com"
            })
            .to_string(),
        )
        .expect("credentials write");

        let token = resolve_install_token("http://127.0.0.1:8080/", None).expect("token resolves");

        assert_eq!(token, "fg_stored");
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn project_installs_use_client_environment_references() {
        let _guard = crate::auth::TEST_ENV_LOCK.lock().expect("test env lock");
        let data_dir = unique_temp_dir("forge-client-mcp-env-data");
        let _env = EnvVarGuard::set("FORGE_TOKEN", "fg_env_reference");
        let _data = EnvVarGuard::set("FORGE_DATA_DIR", data_dir.to_string_lossy().as_ref());
        let repo_dir = unique_temp_dir("forge-client-mcp-env-install");
        fs::create_dir_all(&repo_dir).expect("repo dir creates");
        let cwd_guard = CurrentDirGuard::set(&repo_dir);

        install(
            "http://127.0.0.1:8080",
            AgentTarget::Claude,
            ConfigScope::Project,
            None,
            None,
            Some("project-1"),
        )
        .expect("Claude install succeeds");
        install(
            "http://127.0.0.1:8080",
            AgentTarget::Cursor,
            ConfigScope::Project,
            None,
            None,
            Some("project-1"),
        )
        .expect("Cursor install succeeds");
        install(
            "http://127.0.0.1:8080",
            AgentTarget::Codex,
            ConfigScope::Project,
            None,
            None,
            Some("project-1"),
        )
        .expect("Codex install succeeds");
        drop(cwd_guard);

        let claude_path = repo_dir.join(".claude/settings.json");
        let claude = read_config(&claude_path).expect("Claude config reads");
        assert_eq!(
            claude["mcpServers"]["forge"]["headers"]["Authorization"],
            "Bearer ${FORGE_TOKEN}"
        );
        assert!(!fs::read_to_string(&claude_path)
            .expect("Claude config reads")
            .contains("fg_env_reference"));

        let cursor_path = repo_dir.join(".cursor/mcp.json");
        let cursor = read_config(&cursor_path).expect("Cursor config reads");
        assert_eq!(
            cursor["mcpServers"]["forge"]["headers"]["Authorization"],
            "Bearer ${env:FORGE_TOKEN}"
        );
        assert!(!fs::read_to_string(&cursor_path)
            .expect("Cursor config reads")
            .contains("fg_env_reference"));

        let codex_path = repo_dir.join(".codex/config.toml");
        let codex = fs::read_to_string(&codex_path).expect("Codex config reads");
        assert!(codex.contains("bearer_token_env_var = \"FORGE_TOKEN\""));
        assert!(!codex.contains("fg_env_reference"));
        assert!(!codex.contains("http_headers"));

        let _ = fs::remove_dir_all(repo_dir);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn custom_token_env_name_is_validated() {
        let _guard = crate::auth::TEST_ENV_LOCK.lock().expect("test env lock");
        let _env = EnvVarGuard::set("MY_FORGE_TOKEN", "fg_custom_env");

        match resolve_install_auth(
            "http://127.0.0.1:8080",
            ConfigScope::Project,
            None,
            Some("MY_FORGE_TOKEN"),
        )
        .expect("custom env resolves")
        {
            InstallAuth::Env(name) => assert_eq!(name, "MY_FORGE_TOKEN"),
            InstallAuth::Literal(_) => panic!("expected an environment reference"),
        }
        assert!(resolve_install_auth(
            "http://127.0.0.1:8080",
            ConfigScope::Project,
            None,
            Some("BAD-NAME")
        )
        .is_err());
    }

    #[test]
    fn project_install_fails_closed_without_an_env_reference() {
        let _guard = crate::auth::TEST_ENV_LOCK.lock().expect("test env lock");
        let data_dir = unique_temp_dir("forge-client-mcp-fail-data");
        let _token = EnvVarGuard::unset("FORGE_TOKEN");
        let _data = EnvVarGuard::set("FORGE_DATA_DIR", data_dir.to_string_lossy().as_ref());
        let repo_dir = unique_temp_dir("forge-client-mcp-fail-install");
        fs::create_dir_all(&repo_dir).expect("repo dir creates");
        let cwd_guard = CurrentDirGuard::set(&repo_dir);

        let error = install(
            "http://127.0.0.1:8080",
            AgentTarget::Claude,
            ConfigScope::Project,
            Some("fg_literal"),
            None,
            Some("project-1"),
        )
        .expect_err("literal project token rejects");
        assert!(error.to_string().contains("--token cannot be embedded"));

        let error = install(
            "http://127.0.0.1:8080",
            AgentTarget::Claude,
            ConfigScope::Project,
            None,
            None,
            Some("project-1"),
        )
        .expect_err("missing env rejects");
        assert!(error.to_string().contains("FORGE_TOKEN is not set"));
        drop(cwd_guard);

        assert!(!repo_dir.join(".claude/settings.json").exists());
        let _ = fs::remove_dir_all(repo_dir);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn user_install_writes_literal_header_with_private_permissions() {
        let _guard = crate::auth::TEST_ENV_LOCK.lock().expect("test env lock");
        let data_dir = unique_temp_dir("forge-client-mcp-user-data");
        let home_dir = unique_temp_dir("forge-client-mcp-user-home");
        fs::create_dir_all(&home_dir).expect("home dir creates");
        let _data = EnvVarGuard::set("FORGE_DATA_DIR", data_dir.to_string_lossy().as_ref());
        let _home = EnvVarGuard::set("HOME", home_dir.to_string_lossy().as_ref());

        install(
            "http://127.0.0.1:8080",
            AgentTarget::Claude,
            ConfigScope::User,
            Some("fg_user"),
            None,
            None,
        )
        .expect("user install succeeds");

        let path = home_dir.join(".claude/settings.json");
        let config = read_config(&path).expect("config reads");
        assert_eq!(
            config["mcpServers"]["forge"]["headers"]["Authorization"],
            "Bearer fg_user"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path)
                .expect("config metadata reads")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = fs::remove_dir_all(home_dir);
        let _ = fs::remove_dir_all(data_dir);
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
    }

    struct CurrentDirGuard(PathBuf);

    impl CurrentDirGuard {
        fn set(path: &Path) -> Self {
            let previous = std::env::current_dir().expect("current dir resolves");
            std::env::set_current_dir(path).expect("current dir changes");
            Self(previous)
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}
