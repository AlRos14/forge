use executors::CommandOverrides;
use std::collections::HashMap;
use std::ffi::OsString;
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
    fn resolve_program(&self) -> String {
        if let Some(ref base) = self.overrides.base_command_override {
            base.clone()
        } else {
            self.default_program.clone()
        }
    }

    /// Build the full argument list: default_args + adapter_args + additional_params.
    fn resolve_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        // If using base_command_override, skip default_args (user controls everything)
        if self.overrides.base_command_override.is_none() {
            args.extend(self.default_args.iter().cloned());
        }

        args.extend(self.adapter_args.iter().cloned());

        if let Some(ref additional) = self.overrides.additional_params {
            args.extend(additional.iter().cloned());
        }

        args
    }

    /// Merge profile env into the system environment (profile wins on conflict).
    fn resolve_env(&self) -> HashMap<OsString, OsString> {
        let mut env: HashMap<OsString, OsString> = std::env::vars_os().collect();

        if let Some(ref profile_env) = self.overrides.env {
            for (k, v) in profile_env {
                env.insert(OsString::from(k), OsString::from(v));
            }
        }

        env
    }

    /// Build the tokio Command ready to spawn.
    pub fn build(&self) -> Command {
        let program = self.resolve_program();
        let args = self.resolve_args();
        let env = self.resolve_env();

        let mut cmd = Command::new(&program);
        cmd.args(&args);
        cmd.env_clear();
        for (k, v) in &env {
            cmd.env(k, v);
        }

        cmd
    }

    /// Resolve the full executable path using `which`.
    pub fn resolve_executable(&self) -> Option<std::path::PathBuf> {
        let program = self.resolve_program();
        which::which(&program).ok()
    }
}

/// Remove provider-native session selectors from authored extra arguments for
/// a generic fresh Start. Adapter callers pass only the flags understood by
/// their concrete integration; opaque wrappers remain outside this parser.
pub(crate) fn clear_session_arguments(
    overrides: &mut CommandOverrides,
    value_flags: &[&str],
    boolean_flags: &[&str],
) {
    let Some(arguments) = overrides.additional_params.take() else {
        return;
    };
    let mut filtered = Vec::with_capacity(arguments.len());
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if boolean_flags.iter().any(|flag| argument.as_str() == *flag) {
            index += 1;
            continue;
        }
        let exact_value_flag = value_flags.iter().any(|flag| argument.as_str() == *flag);
        let inline_value_flag = value_flags.iter().any(|flag| {
            argument
                .strip_prefix(*flag)
                .is_some_and(|suffix| suffix.starts_with('='))
        });
        if exact_value_flag || inline_value_flag {
            index += 1;
            if exact_value_flag
                && arguments
                    .get(index)
                    .is_some_and(|value| !value.starts_with('-'))
            {
                index += 1;
            }
            continue;
        }
        filtered.push(argument.clone());
        index += 1;
    }
    overrides.additional_params = Some(filtered);
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
    fn fresh_start_filters_provider_session_arguments_only() {
        let mut overrides = CommandOverrides {
            additional_params: Some(vec![
                "--verbose".into(),
                "--resume".into(),
                "old-session".into(),
                "--session=stale".into(),
                "--continue".into(),
                "--model".into(),
                "model-a".into(),
            ]),
            ..CommandOverrides::default()
        };
        clear_session_arguments(&mut overrides, &["--resume", "--session"], &["--continue"]);
        assert_eq!(
            overrides.additional_params,
            Some(vec!["--verbose".into(), "--model".into(), "model-a".into()])
        );
    }
}
