use executors::{CommandOverrides, ProcessGroupChild};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Duration;
use tokio::{process::Command, sync::Mutex};

/// Last-resort group termination when an adapter execution future is dropped.
/// Normal cancellation still waits for the process group to exit.
pub(crate) struct GroupKillOnDrop {
    child: Arc<Mutex<ProcessGroupChild>>,
    armed: bool,
}

impl GroupKillOnDrop {
    pub(crate) fn new(child: Arc<Mutex<ProcessGroupChild>>) -> Self {
        Self { child, armed: true }
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for GroupKillOnDrop {
    fn drop(&mut self) {
        if self.armed
            && let Ok(mut child) = self.child.try_lock()
        {
            let _ = child.start_kill();
        }
    }
}

/// Terminate and verify the complete process boundary for an Execution.
pub(crate) async fn kill_group_and_wait(child: &mut ProcessGroupChild) -> io::Result<ExitStatus> {
    child.kill_and_wait().await
}

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
    use executors::ProcessGroupChild;
    use std::time::Duration;

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

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn group_kill_terminates_descendant_after_leader_was_reaped() {
        let dir = tempfile::tempdir().expect("temp directory creates");
        let pid_file = dir.path().join("descendant.pid");
        let mut command = tokio::process::Command::new("sh");
        command.args([
            "-c",
            &format!("sleep 60 & echo $! > '{}' ; exit 0", pid_file.display()),
        ]);
        let mut child = ProcessGroupChild::spawn(&mut command).expect("group spawns");
        let leader_pid = child.id().expect("leader PID exists");

        let descendant_pid = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(value) = tokio::fs::read_to_string(&pid_file).await {
                    if let Ok(pid) = value.trim().parse::<u32>() {
                        break pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("descendant writes its PID");

        tokio::time::timeout(Duration::from_secs(2), async {
            while pid_is_executable(leader_pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("leader exits before group termination");
        assert!(pid_is_executable(descendant_pid));
        assert!(
            child
                .try_wait_leader()
                .expect("leader status can be collected")
                .is_some(),
            "leader status is cached before group termination"
        );

        kill_group_and_wait(&mut child)
            .await
            .expect("group termination is verified");
        assert!(!pid_is_executable(descendant_pid));
        assert!(!child.group_is_alive().expect("group status is readable"));
    }

    #[cfg(target_os = "linux")]
    fn pid_is_executable(pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        let Some(end_of_command) = stat.rfind(')') else {
            return false;
        };
        !matches!(stat[end_of_command + 2..].chars().next(), Some('Z' | 'X'))
    }
}
