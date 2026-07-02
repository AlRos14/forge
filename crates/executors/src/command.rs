use crate::ShellConfig;
use std::collections::HashMap;
use std::path::PathBuf;

/// Resolved argv/env/cwd for spawning a shell executor process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommandPlan {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env_remove: Vec<String>,
    pub env_set: HashMap<String, String>,
}

/// Build the shell command plan used by [`crate::ShellExecutor`].
pub fn build_shell_command_plan(
    description: &str,
    worktree_path: &str,
    max_turns: Option<u32>,
    config: Option<&ShellConfig>,
) -> ShellCommandPlan {
    let mut env_set = HashMap::new();
    if let Some(max_turns) = max_turns {
        env_set.insert("FORGE_MAX_TURNS".to_string(), max_turns.to_string());
    }
    if let Some(config) = config {
        if let Some(env) = &config.command_overrides.env {
            env_set.extend(env.clone());
        }
    }

    let program = config
        .and_then(|config| config.command.clone())
        .or_else(|| {
            config.and_then(|config| config.command_overrides.base_command_override.clone())
        })
        .unwrap_or_else(|| "sh".to_string());

    let mut args = Vec::new();
    if let Some(config) = config {
        if let Some(config_args) = &config.args {
            args.extend(config_args.clone());
        } else if program == "sh" {
            args.push("-c".to_string());
            args.push(description.to_string());
        } else {
            args.push(description.to_string());
        }
        if let Some(additional) = &config.command_overrides.additional_params {
            args.extend(additional.clone());
        }
    } else {
        args.push("-c".to_string());
        args.push(description.to_string());
    }

    ShellCommandPlan {
        program,
        args,
        cwd: PathBuf::from(worktree_path),
        env_remove: vec![
            "GIT_DIR".to_owned(),
            "GIT_WORK_TREE".to_owned(),
            "GIT_INDEX_FILE".to_owned(),
        ],
        env_set,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandOverrides, PermissionPolicy};

    #[test]
    fn shell_command_plan_defaults_to_sh_c() {
        let plan = build_shell_command_plan("echo hello", "/tmp/worktree", None, None);

        assert_eq!(plan.program, "sh");
        assert_eq!(plan.args, vec!["-c", "echo hello"]);
        assert_eq!(plan.cwd, PathBuf::from("/tmp/worktree"));
        assert_eq!(
            plan.env_remove,
            vec!["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"]
        );
        assert!(plan.env_set.is_empty());
    }

    #[test]
    fn shell_command_plan_from_snapshot_config() {
        let config = ShellConfig {
            command: Some("bash".to_owned()),
            args: Some(vec!["-lc".to_owned(), "make test".to_owned()]),
            timeout_seconds: Some(60),
            permission_policy: Some(PermissionPolicy::Supervised),
            command_overrides: CommandOverrides {
                additional_params: Some(vec!["--verbose".to_owned()]),
                env: Some(HashMap::from([("CI".to_owned(), "1".to_owned())])),
                ..CommandOverrides::default()
            },
        };

        let plan = build_shell_command_plan(
            "ignored when args are set",
            "/tmp/worktree",
            Some(3),
            Some(&config),
        );

        assert_eq!(plan.program, "bash");
        assert_eq!(plan.args, vec!["-lc", "make test", "--verbose"]);
        assert_eq!(plan.cwd, PathBuf::from("/tmp/worktree"));
        assert_eq!(plan.env_set.get("CI").map(String::as_str), Some("1"));
        assert_eq!(
            plan.env_set.get("FORGE_MAX_TURNS").map(String::as_str),
            Some("3")
        );
    }
}
