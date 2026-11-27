//! Git attribution for harness-bound Agent Executions.
//!
//! Forge gives every Agent Execution that works in a repository Workspace a
//! distinct author/committer identity and deterministic `Agent-Role` /
//! `Agent-Run` commit trailers. The identity travels in the in-memory
//! `runtime_env` of the Execution snapshot, so it never mutates the user's
//! global or repository Git configuration. Trailers are added by a
//! per-Execution `commit-msg` hook selected through `GIT_CONFIG_*` process
//! environment, which makes the mechanism local to the harness process tree.

use crate::ExecutorError;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Reserved, non-routable domain (RFC 2606) for Forge-attributed identities.
pub const EMAIL_DOMAIN: &str = "agents.forge.invalid";
// Forge's own deterministic Git operations use the fixed identity defined
// in the `git` crate (`git::FORGE_SYSTEM_NAME` / `git::FORGE_SYSTEM_EMAIL`).

pub const ENV_AUTHOR_NAME: &str = "GIT_AUTHOR_NAME";
pub const ENV_AUTHOR_EMAIL: &str = "GIT_AUTHOR_EMAIL";
pub const ENV_COMMITTER_NAME: &str = "GIT_COMMITTER_NAME";
pub const ENV_COMMITTER_EMAIL: &str = "GIT_COMMITTER_EMAIL";
pub const ENV_AGENT_ROLE: &str = "FORGE_AGENT_ROLE";
pub const ENV_EXECUTION_ID: &str = "FORGE_EXECUTION_ID";

const IDENTITY_KEYS: [&str; 4] = [
    ENV_AUTHOR_NAME,
    ENV_AUTHOR_EMAIL,
    ENV_COMMITTER_NAME,
    ENV_COMMITTER_EMAIL,
];

const HOOK_NAMES: [&str; 14] = [
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "post-rewrite",
    "pre-push",
    "pre-auto-gc",
];

/// Effective Git identity of one Agent Execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGitIdentity {
    pub name: String,
    pub email: String,
    pub role: String,
    pub execution_id: String,
}

impl AgentGitIdentity {
    /// Derive the identity from the Agent Actor and the Execution TaskRole.
    pub fn new(role: &str, agent_name: &str, agent_id: &str, execution_id: &str) -> Self {
        let role_label = clean_text(role, "agent");
        let agent_label = clean_text(agent_name, "agent");
        let role_slug = slug(role, "agent");
        let short_id: String = agent_id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(8)
            .collect::<String>()
            .to_ascii_lowercase();
        let short_id = if short_id.is_empty() {
            "unknown".to_owned()
        } else {
            short_id
        };
        Self {
            name: format!("{role_label} ({agent_label})"),
            email: format!("agent+{role_slug}.{short_id}@{EMAIL_DOMAIN}"),
            role: role_label,
            execution_id: execution_id.trim().to_owned(),
        }
    }

    /// Environment entries that Forge owns for this Execution.
    pub fn runtime_env(&self) -> Map<String, Value> {
        let mut env = Map::new();
        for (key, value) in [
            (ENV_AUTHOR_NAME, &self.name),
            (ENV_AUTHOR_EMAIL, &self.email),
            (ENV_COMMITTER_NAME, &self.name),
            (ENV_COMMITTER_EMAIL, &self.email),
            (ENV_AGENT_ROLE, &self.role),
            (ENV_EXECUTION_ID, &self.execution_id),
        ] {
            env.insert(key.to_owned(), Value::String(value.clone()));
        }
        env
    }
}

fn clean_text(value: &str, fallback: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        fallback.to_owned()
    } else {
        collapsed
    }
}

fn slug(value: &str, fallback: &str) -> String {
    let slug: String = value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() {
        fallback.to_owned()
    } else {
        slug
    }
}

fn env_string<'a>(config: &'a Value, key: &str) -> Option<&'a str> {
    config
        .get("env")?
        .get(key)?
        .as_str()
        .filter(|value| !value.is_empty())
}

/// Identity and trailer variables of an invocation config, for Git commands
/// that Forge runs on behalf of the Execution (adapter auto-commit).
pub fn commit_environment(agent_config: &Value) -> Vec<(String, String)> {
    IDENTITY_KEYS
        .iter()
        .filter_map(|key| env_string(agent_config, key).map(|v| ((*key).to_owned(), v.to_owned())))
        .collect()
}

/// Append the deterministic Agent trailers when the invocation carries an
/// Agent identity; otherwise return the message unchanged.
pub fn message_with_trailers(message: &str, agent_config: &Value) -> String {
    let (Some(role), Some(run)) = (
        env_string(agent_config, ENV_AGENT_ROLE),
        env_string(agent_config, ENV_EXECUTION_ID),
    ) else {
        return message.to_owned();
    };
    format!(
        "{}\n\nAgent-Role: {role}\nAgent-Run: {run}\n",
        message.trim_end()
    )
}

/// Source of the shared Forge hook. Trailers are added by
/// `git interpret-trailers --if-exists replace`; every other hook name chains
/// to the repository's own hook so the hooks path override loses nothing.
pub fn hook_script() -> String {
    r#"#!/bin/sh
# Managed by Forge for one Agent Execution. Not installed in any repository.
name="$(basename "$0")"
if [ "$name" = "commit-msg" ] && [ -n "$FORGE_AGENT_ROLE" ] && [ -n "$FORGE_EXECUTION_ID" ]; then
  git interpret-trailers --if-exists replace --in-place \
    --trailer "Agent-Role: $FORGE_AGENT_ROLE" \
    --trailer "Agent-Run: $FORGE_EXECUTION_ID" "$1" || exit 1
fi
orig_dir="$(git config --local --get core.hooksPath 2>/dev/null)"
if [ -z "$orig_dir" ]; then
  orig_dir="$(git rev-parse --git-common-dir 2>/dev/null)/hooks"
fi
if [ -x "$orig_dir/$name" ]; then
  exec "$orig_dir/$name" "$@"
fi
exit 0
"#
    .to_owned()
}

/// Where the per-Execution hook directory lives, derived from its log path.
pub fn hooks_dir_for_logs(logs_path: &str) -> PathBuf {
    PathBuf::from(format!("{logs_path}.git-hooks"))
}

/// Write the hook directory (idempotent).
pub fn write_hooks(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let script = hook_script();
    for name in HOOK_NAMES {
        let path = dir.join(name);
        std::fs::write(&path, &script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// When the invocation config carries an Agent Execution identity, install
/// the trailer hooks and point Git at them through process environment. Any
/// agent-authored `GIT_CONFIG_*` entries are dropped: Forge's win.
pub fn apply_trailer_hooks(config: &mut Value, logs_path: &str) -> Result<(), ExecutorError> {
    if env_string(config, ENV_AGENT_ROLE).is_none()
        || env_string(config, ENV_EXECUTION_ID).is_none()
    {
        return Ok(());
    }
    if logs_path.trim().is_empty() {
        return Ok(());
    }
    let dir = hooks_dir_for_logs(logs_path);
    write_hooks(&dir).map_err(|error| {
        ExecutorError::Other(format!("failed to prepare Agent Git hooks: {error}"))
    })?;
    let Some(env) = config.get_mut("env").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    env.retain(|key, _| !key.starts_with("GIT_CONFIG_") && key != "GIT_CONFIG");
    env.insert("GIT_CONFIG_COUNT".to_owned(), Value::String("1".to_owned()));
    env.insert(
        "GIT_CONFIG_KEY_0".to_owned(),
        Value::String("core.hooksPath".to_owned()),
    );
    env.insert(
        "GIT_CONFIG_VALUE_0".to_owned(),
        Value::String(dir.to_string_lossy().into_owned()),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identity_is_derived_from_role_and_agent() {
        let identity =
            AgentGitIdentity::new("Implementer", "Codex <Main>", "AG-1234abcd-ef", "ex1");
        assert_eq!(identity.name, "Implementer (Codex Main)");
        assert_eq!(
            identity.email,
            "agent+implementer.ag1234ab@agents.forge.invalid"
        );
        let env = identity.runtime_env();
        assert_eq!(env[ENV_COMMITTER_EMAIL], json!(identity.email));
        assert_eq!(env[ENV_EXECUTION_ID], json!("ex1"));
    }

    #[test]
    fn runtime_env_wins_over_agent_configured_identity() {
        let config = json!({"env": {"GIT_AUTHOR_EMAIL": "human@example.com", "KEEP": "1"}});
        let identity = AgentGitIdentity::new("worker", "A", "a1", "ex");
        let snapshot = json!({ "runtime_env": identity.runtime_env() });
        let merged = crate::config::with_runtime_environment(&config, &snapshot).unwrap();
        assert_eq!(merged["env"][ENV_AUTHOR_EMAIL], json!(identity.email));
        assert_eq!(merged["env"][ENV_COMMITTER_NAME], json!(identity.name));
        assert_eq!(merged["env"]["KEEP"], json!("1"));
    }

    #[test]
    fn hook_adds_both_trailers_and_replaces_agent_git_config() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("run.jsonl");
        let mut config = json!({"env": {
            ENV_AGENT_ROLE: "worker", ENV_EXECUTION_ID: "exec-1",
            "GIT_CONFIG_COUNT": "3", "GIT_CONFIG_KEY_2": "user.email",
        }});
        apply_trailer_hooks(&mut config, &logs.to_string_lossy()).unwrap();
        assert_eq!(config["env"]["GIT_CONFIG_COUNT"], json!("1"));
        assert!(config["env"].get("GIT_CONFIG_KEY_2").is_none());
        let hooks = config["env"]["GIT_CONFIG_VALUE_0"]
            .as_str()
            .unwrap()
            .to_owned();

        let run = |args: &[&str], dir: &Path| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "core.hooksPath")
                .env("GIT_CONFIG_VALUE_0", &hooks)
                .env("GIT_AUTHOR_NAME", "W (A)")
                .env("GIT_AUTHOR_EMAIL", "agent+w.a@agents.forge.invalid")
                .env("GIT_COMMITTER_NAME", "W (A)")
                .env("GIT_COMMITTER_EMAIL", "agent+w.a@agents.forge.invalid")
                .env(ENV_AGENT_ROLE, "worker")
                .env(ENV_EXECUTION_ID, "exec-1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        let repo = tempfile::tempdir().unwrap();
        run(&["init", "-q"], repo.path());
        std::fs::write(repo.path().join("f"), "x").unwrap();
        run(&["add", "f"], repo.path());
        run(&["commit", "-q", "-m", "subject"], repo.path());
        let body = run(&["log", "-1", "--format=%an <%ae>%n%B"], repo.path());
        assert!(body.contains("W (A) <agent+w.a@agents.forge.invalid>"));
        assert!(body.contains("Agent-Role: worker"));
        assert!(body.contains("Agent-Run: exec-1"));
    }

    #[test]
    fn message_helper_appends_trailers_only_for_agent_identity() {
        let with = json!({"env": {ENV_AGENT_ROLE: "r", ENV_EXECUTION_ID: "e"}});
        let text = message_with_trailers("agent: x", &with);
        assert!(text.contains("Agent-Role: r") && text.contains("Agent-Run: e"));
        assert_eq!(message_with_trailers("agent: x", &json!({})), "agent: x");
    }
}
