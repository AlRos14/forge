use std::{process::Stdio, time::Duration};

use api_types::DetectedCli;
use executors::{AvailabilityStatus, ExecutorKind};
use tokio::{process::Command, time::timeout};

const VERSION_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn detect_clis() -> Vec<DetectedCli> {
    let registry = cli_adapters::default_registry();
    let mut kinds = registry.kinds();
    kinds.sort_by_key(|kind| kind.to_string());

    let mut detected = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let Some(adapter) = registry.get(&kind) else {
            continue;
        };
        let availability = adapter.check_availability();
        let (path, version) = cli_path_and_version(&kind).await;
        detected.push(DetectedCli {
            kind: kind.to_string(),
            availability: availability_status(&availability.status).to_owned(),
            config_path: availability.config_path,
            version,
            path,
        });
    }

    detected
}

fn availability_status(status: &AvailabilityStatus) -> &'static str {
    match status {
        AvailabilityStatus::Authenticated => "authenticated",
        AvailabilityStatus::Installed => "installed",
        AvailabilityStatus::NotFound => "not_found",
    }
}

async fn cli_path_and_version(kind: &ExecutorKind) -> (Option<String>, Option<String>) {
    match kind {
        ExecutorKind::Embedded => (None, None),
        ExecutorKind::Shell => shell_path_and_version(),
        ExecutorKind::Codex => binary_path_and_version("codex").await,
        ExecutorKind::ClaudeCode => binary_path_and_version("claude").await,
        ExecutorKind::Cursor => binary_path_and_version("cursor-agent").await,
        ExecutorKind::Opencode => binary_path_and_version("opencode").await,
        ExecutorKind::Gemini => binary_path_and_version("gemini").await,
        ExecutorKind::Smith => binary_path_and_version("smith").await,
        ExecutorKind::Null => (None, None),
    }
}

fn shell_path_and_version() -> (Option<String>, Option<String>) {
    if cfg!(windows) {
        (Some("cmd.exe".to_owned()), None)
    } else {
        (Some("/bin/sh".to_owned()), None)
    }
}

async fn binary_path_and_version(binary: &str) -> (Option<String>, Option<String>) {
    let path = which::which(binary)
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    let version = if path.is_some() {
        binary_version(binary).await
    } else {
        None
    };
    (path, version)
}

async fn binary_version(binary: &str) -> Option<String> {
    let mut command = Command::new(binary);
    command
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let child = command.spawn().ok()?;
    let output = timeout(VERSION_TIMEOUT, child.wait_with_output())
        .await
        .ok()?
        .ok()?;

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}
