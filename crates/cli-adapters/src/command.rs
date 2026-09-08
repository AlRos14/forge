use executors::CommandOverrides;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Run a discovery command without letting a stalled CLI pin an async worker
/// or survive after the timeout expires.
pub async fn output_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> Option<std::process::Output> {
    command.kill_on_drop(true);
    tokio::time::timeout(timeout, command.output())
        .await
        .ok()?
        .ok()
}

/// Builds a tokio Command from adapter defaults + user overrides.
pub struct CommandBuilder {
    default_program: String,
    default_args: Vec<String>,
    adapter_args: Vec<String>,
    overrides: CommandOverrides,
}

impl CommandBuilder {
    pub fn new(default_program: impl Into<String>) -> Self {
        Self {
            default_program: default_program.into(),
            default_args: Vec::new(),
            adapter_args: Vec::new(),
            overrides: CommandOverrides::default(),
        }
    }

    pub fn default_args(mut self, args: Vec<String>) -> Self {
        self.default_args = args;
        self
    }

    pub fn adapter_args(mut self, args: Vec<String>) -> Self {
        self.adapter_args = args;
        self
    }

    pub fn overrides(mut self, overrides: &CommandOverrides) -> Self {
        self.overrides = overrides.clone();
        self
    }

    /// Resolve the program to use (override or default).
    fn resolve_launch(&self) -> ResolvedLaunch {
        if let Some(ref base) = self.overrides.base_command_override {
            resolve_override(base)
        } else {
            ResolvedLaunch {
                program: self.default_program.clone(),
                prefix_args: Vec::new(),
                extra_env: HashMap::new(),
            }
        }
    }

    /// Build the full argument list: default_args + adapter_args + additional_params.
    fn resolve_args(&self, launch: &ResolvedLaunch) -> Vec<String> {
        let mut args = Vec::new();

        // If using base_command_override, skip default_args (user controls everything)
        if self.overrides.base_command_override.is_none() {
            args.extend(self.default_args.iter().cloned());
        }

        args.extend(launch.prefix_args.iter().cloned());
        args.extend(self.adapter_args.iter().cloned());

        if let Some(ref additional) = self.overrides.additional_params {
            args.extend(additional.iter().cloned());
        }

        args
    }

    /// Merge alias env then profile env into the system environment (profile wins).
    fn resolve_env(&self, launch: &ResolvedLaunch) -> HashMap<OsString, OsString> {
        let mut env: HashMap<OsString, OsString> = std::env::vars_os().collect();

        for (k, v) in &launch.extra_env {
            env.insert(OsString::from(k), OsString::from(v));
        }

        if let Some(ref profile_env) = self.overrides.env {
            for (k, v) in profile_env {
                env.insert(OsString::from(k), OsString::from(expand_user_path(v)));
            }
        }

        env
    }

    /// Build the tokio Command ready to spawn.
    pub fn build(&self) -> Command {
        let launch = self.resolve_launch();
        let args = self.resolve_args(&launch);
        let env = self.resolve_env(&launch);

        let mut cmd = Command::new(&launch.program);
        cmd.args(&args);
        cmd.env_clear();
        for (k, v) in &env {
            cmd.env(k, v);
        }

        cmd
    }

    /// Resolve the full executable path using `which`.
    pub fn resolve_executable(&self) -> Option<std::path::PathBuf> {
        let program = self.resolve_launch().program;
        let path = Path::new(&program);
        if path.is_absolute() {
            path.exists().then(|| path.to_path_buf())
        } else {
            which::which(&program).ok()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedLaunch {
    program: String,
    prefix_args: Vec<String>,
    extra_env: HashMap<String, String>,
}

fn expand_user_path(value: &str) -> String {
    if value == "~" {
        return dirs::home_dir()
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_else(|| value.to_owned());
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    value.to_owned()
}

fn is_simple_command_name(program: &str) -> bool {
    !program.is_empty()
        && !program.contains('/')
        && !program.contains('\\')
        && program
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn login_shell_which(program: &str) -> Option<PathBuf> {
    marked_bash_path(
        [
            "-lc",
            r#"printf '%s\n' FORGE_WHICH_START; command -v -- "$1"; printf '%s\n' FORGE_WHICH_END"#,
        ],
        program,
        "FORGE_WHICH_START",
        "FORGE_WHICH_END",
    )
}

fn interactive_which(program: &str) -> Option<PathBuf> {
    marked_bash_path(
        [
            "-ic",
            r#"printf '%s\n' FORGE_WHICH_START; type -P -- "$1"; printf '%s\n' FORGE_WHICH_END"#,
        ],
        program,
        "FORGE_WHICH_START",
        "FORGE_WHICH_END",
    )
}

fn marked_bash_path(args: [&str; 2], program: &str, start: &str, end: &str) -> Option<PathBuf> {
    if !is_simple_command_name(program) {
        return None;
    }
    let output = std::process::Command::new("bash")
        .args([args[0], args[1], "bash", program])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let path = extract_marked(&stdout, start, end)?;
    let path = path.lines().next()?.trim();
    if !path.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(path))
}

fn lookup_binary(program: &str) -> String {
    let expanded = expand_user_path(program);
    if expanded.contains(std::path::MAIN_SEPARATOR) || expanded.starts_with('.') {
        return expanded;
    }
    which::which(&expanded)
        .ok()
        .or_else(|| login_shell_which(&expanded))
        .or_else(|| interactive_which(&expanded))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or(expanded)
}

fn resolve_override(program: &str) -> ResolvedLaunch {
    let expanded = expand_user_path(program);
    if expanded.contains(std::path::MAIN_SEPARATOR) || expanded.starts_with('.') {
        return ResolvedLaunch {
            program: expanded,
            prefix_args: Vec::new(),
            extra_env: HashMap::new(),
        };
    }
    if let Some(path) = which::which(&expanded)
        .ok()
        .or_else(|| login_shell_which(&expanded))
    {
        return ResolvedLaunch {
            program: path.to_string_lossy().into_owned(),
            prefix_args: Vec::new(),
            extra_env: HashMap::new(),
        };
    }
    if let Some(alias) = resolve_interactive_alias(&expanded) {
        return alias;
    }
    ResolvedLaunch {
        program: lookup_binary(&expanded),
        prefix_args: Vec::new(),
        extra_env: HashMap::new(),
    }
}

fn resolve_interactive_alias(name: &str) -> Option<ResolvedLaunch> {
    if !is_simple_command_name(name) {
        return None;
    }
    let output = std::process::Command::new("bash")
        .args([
            "-ic",
            r#"printf '%s\n' FORGE_ALIAS_START; alias "$1"; printf '%s\n' FORGE_ALIAS_END"#,
            "bash",
            name,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let block = extract_marked(&stdout, "FORGE_ALIAS_START", "FORGE_ALIAS_END")?;
    let line = block
        .lines()
        .find(|line| line.trim_start().starts_with("alias "))?;
    let (_, body) = parse_bash_alias_line(line)?;
    tokenize_alias_body(body)
}

fn extract_marked(stdout: &str, start: &str, end: &str) -> Option<String> {
    let from = stdout.find(start)? + start.len();
    let rest = stdout.get(from..)?;
    let until = rest.find(end)?;
    Some(rest[..until].trim().to_owned())
}

fn parse_bash_alias_line(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim().strip_prefix("alias ")?;
    let eq = rest.find('=')?;
    let name = rest[..eq].trim();
    let mut value = rest[eq + 1..].trim();
    if value.len() >= 2
        && ((value.starts_with('\'') && value.ends_with('\''))
            || (value.starts_with('"') && value.ends_with('"')))
    {
        value = &value[1..value.len() - 1];
    }
    if name.is_empty() || value.is_empty() {
        return None;
    }
    Some((name, value))
}

fn parse_env_assignment(token: &str) -> Option<(&str, &str)> {
    let eq = token.find('=')?;
    let key = &token[..eq];
    let first = key.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_')
        || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some((key, &token[eq + 1..]))
}

fn expand_shell_vars(value: &str) -> String {
    let home = dirs::home_dir()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();
    expand_user_path(&value.replace("${HOME}", &home).replace("$HOME", &home))
}

fn tokenize_alias_body(body: &str) -> Option<ResolvedLaunch> {
    let mut extra_env = HashMap::new();
    let mut words = Vec::new();
    for token in body.split_whitespace() {
        if words.is_empty() {
            if let Some((key, value)) = parse_env_assignment(token) {
                extra_env.insert(key.to_owned(), expand_shell_vars(value));
                continue;
            }
        }
        words.push(expand_shell_vars(token));
    }
    let program = words.first()?.clone();
    let prefix_args = words.into_iter().skip(1).collect();
    Some(ResolvedLaunch {
        program: lookup_binary(&program),
        prefix_args,
        extra_env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use executors::CommandOverrides;

    #[test]
    fn default_command_no_overrides() {
        let builder = CommandBuilder::new("codex")
            .default_args(vec!["-y".into(), "@openai/codex@0.1".into()])
            .adapter_args(vec!["app-server".into()]);

        let cmd = builder.build();
        let prog = cmd.as_std().get_program();
        assert_eq!(prog, "codex");

        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(args, vec!["-y", "@openai/codex@0.1", "app-server"]);
    }

    #[test]
    fn base_command_override_skips_default_args() {
        let overrides = CommandOverrides {
            base_command_override: Some("/usr/local/bin/my-codex".into()),
            additional_params: Some(vec!["--verbose".into()]),
            env: None,
        };
        let builder = CommandBuilder::new("npx")
            .default_args(vec!["-y".into(), "@openai/codex@0.1".into()])
            .adapter_args(vec!["app-server".into()])
            .overrides(&overrides);

        let cmd = builder.build();
        let prog = cmd.as_std().get_program();
        assert_eq!(prog, "/usr/local/bin/my-codex");

        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(args, vec!["app-server", "--verbose"]);
    }

    #[test]
    fn env_merge_profile_wins() {
        let overrides = CommandOverrides {
            base_command_override: None,
            additional_params: None,
            env: Some(HashMap::from([("MY_VAR".into(), "profile_val".into())])),
        };
        let builder = CommandBuilder::new("echo").overrides(&overrides);
        let cmd = builder.build();

        let envs: HashMap<_, _> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v.to_owned())))
            .collect();
        assert_eq!(
            envs.get(&OsString::from("MY_VAR")),
            Some(&OsString::from("profile_val"))
        );
    }

    #[test]
    fn parses_codex2_style_alias() {
        let (name, body) =
            parse_bash_alias_line("alias codex2='CODEX_HOME=$HOME/.codex-plus2 codex'")
                .expect("alias line");
        assert_eq!(name, "codex2");
        let launch = tokenize_alias_body(body).expect("body");
        let home = dirs::home_dir().expect("home");
        let expected_home = home.join(".codex-plus2").to_string_lossy().into_owned();
        assert_eq!(
            launch.extra_env.get("CODEX_HOME").map(String::as_str),
            Some(expected_home.as_str())
        );
        assert!(
            launch.program.ends_with("codex") || launch.program == "codex",
            "program={}",
            launch.program
        );
        assert!(launch.prefix_args.is_empty());
    }
}
