use crate::daemon_service::{
    DaemonRegisterInput, DaemonReportInput, DaemonService, DetectedCliInput, RuntimeReportInput,
};
use crate::{Result, ServiceError};
use db::SqliteDb;
use events::EventBus;
use executors::{AdapterRegistry, AvailabilityStatus, ExecutorKind};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

const DEFAULT_REPORT_INTERVAL: Duration = Duration::from_secs(60);
const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
const CREDENTIALS_FILE: &str = "daemon_credentials.json";

#[derive(Clone)]
pub struct EmbeddedDaemon {
    service: DaemonService,
    adapter_registry: Arc<AdapterRegistry>,
    forge_home: PathBuf,
    workspace_root: PathBuf,
    report_interval: Duration,
    stop_requested: Arc<AtomicBool>,
    stop_notify: Arc<Notify>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DaemonCredentials {
    daemon_id: String,
    token: String,
}

impl EmbeddedDaemon {
    pub async fn new(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        adapter_registry: Arc<AdapterRegistry>,
        forge_home: PathBuf,
        workspace_root: PathBuf,
    ) -> Result<Self> {
        Ok(Self::with_report_interval(
            db,
            event_bus,
            adapter_registry,
            forge_home,
            workspace_root,
            DEFAULT_REPORT_INTERVAL,
        ))
    }

    pub fn with_report_interval(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        adapter_registry: Arc<AdapterRegistry>,
        forge_home: PathBuf,
        workspace_root: PathBuf,
        report_interval: Duration,
    ) -> Self {
        Self {
            service: DaemonService::new(db, event_bus),
            adapter_registry,
            forge_home,
            workspace_root,
            report_interval,
            stop_requested: Arc::new(AtomicBool::new(false)),
            stop_notify: Arc::new(Notify::new()),
        }
    }

    pub fn start(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            while !self.stop_requested.load(Ordering::SeqCst) {
                if let Err(error) = self.scan_and_report().await {
                    tracing::warn!(%error, "embedded daemon report failed");
                }

                tokio::select! {
                    () = sleep(self.report_interval) => {}
                    () = self.stop_notify.notified() => {}
                }
            }
        })
    }

    pub fn stop(&self) {
        self.stop_requested.store(true, Ordering::SeqCst);
        self.stop_notify.notify_waiters();
    }

    async fn scan_and_report(&self) -> Result<()> {
        let credentials = self.register_or_load_credentials().await?;
        let detected_clis = self.detect_clis().await;
        let runtimes = vec![RuntimeReportInput {
            kind: "local".to_owned(),
            workspace_root: self.workspace_root().to_string_lossy().into_owned(),
            status: Some("ready".to_owned()),
        }];

        self.service
            .ingest_report(
                &credentials.daemon_id,
                DaemonReportInput {
                    detected_clis,
                    runtimes,
                    labels: None,
                    active_execution_ids: Some(Vec::new()),
                },
            )
            .await?;

        Ok(())
    }

    async fn register_or_load_credentials(&self) -> Result<DaemonCredentials> {
        if let Some(credentials) = self.read_credentials()? {
            if self
                .service
                .authenticate(&credentials.daemon_id, &credentials.token)
                .await
                .is_ok()
            {
                return Ok(credentials);
            }
        }

        let registration = self.service.register(self.registration_input()).await?;
        let credentials = DaemonCredentials {
            daemon_id: registration.daemon_id,
            token: registration.plaintext_token,
        };
        self.write_credentials(&credentials)?;
        Ok(credentials)
    }

    fn read_credentials(&self) -> Result<Option<DaemonCredentials>> {
        let path = self.credentials_path();
        if !path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(&path).map_err(io_error)?;
        let credentials = serde_json::from_str(&contents).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "invalid embedded daemon credentials at {}: {error}",
                path.display()
            ))
        })?;
        Ok(Some(credentials))
    }

    fn write_credentials(&self, credentials: &DaemonCredentials) -> Result<()> {
        std::fs::create_dir_all(&self.forge_home).map_err(io_error)?;
        let contents = serde_json::to_string_pretty(credentials).map_err(|error| {
            ServiceError::invalid_operation(format!("invalid embedded daemon credentials: {error}"))
        })?;
        std::fs::write(self.credentials_path(), contents).map_err(io_error)
    }

    fn registration_input(&self) -> DaemonRegisterInput {
        let hostname = local_hostname();
        DaemonRegisterInput {
            machine_id: embedded_machine_id(),
            hostname,
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            agent_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            labels: json!({ "mode": "embedded" }),
            runtimes: vec![RuntimeReportInput {
                kind: "local".to_owned(),
                workspace_root: self.workspace_root().to_string_lossy().into_owned(),
                status: Some("ready".to_owned()),
            }],
            owner_id: None,
            visibility: Some("global".to_owned()),
        }
    }

    async fn detect_clis(&self) -> Vec<DetectedCliInput> {
        let mut detected = Vec::new();
        for kind in self.adapter_registry.kinds() {
            let Some(adapter) = self.adapter_registry.get(&kind) else {
                continue;
            };
            let availability = adapter.check_availability();
            let (path, version) = cli_path_and_version(&kind).await;
            detected.push(DetectedCliInput {
                kind: kind.to_string(),
                availability: availability_status(&availability.status).to_owned(),
                config_path: availability.config_path,
                version,
                path,
            });
        }
        detected
    }

    fn credentials_path(&self) -> PathBuf {
        self.forge_home.join(CREDENTIALS_FILE)
    }

    fn workspace_root(&self) -> PathBuf {
        self.workspace_root.clone()
    }
}

pub fn embedded_machine_id() -> String {
    format!(
        "embedded:{}:{}:{}",
        local_hostname(),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

pub fn is_embedded_daemon_machine(machine_id: &str) -> bool {
    machine_id == embedded_machine_id()
}

async fn cli_path_and_version(kind: &ExecutorKind) -> (Option<String>, Option<String>) {
    match kind {
        // Forge-hosted native profiles do not have a CLI binary or daemon
        // detection record. They are dispatched by the Task executor router.
        ExecutorKind::Embedded => (None, None),
        ExecutorKind::Shell => (Some("/bin/sh".to_owned()), None),
        ExecutorKind::Codex => binary_path_and_version("codex").await,
        ExecutorKind::ClaudeCode => binary_path_and_version("claude").await,
        ExecutorKind::Cursor => binary_path_and_version("cursor-agent").await,
        ExecutorKind::Opencode => binary_path_and_version("opencode").await,
        ExecutorKind::Gemini => binary_path_and_version("gemini").await,
        ExecutorKind::Smith => binary_path_and_version("smith").await,
        ExecutorKind::Null => (None, None),
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

fn availability_status(status: &AvailabilityStatus) -> &'static str {
    match status {
        AvailabilityStatus::Authenticated => "authenticated",
        AvailabilityStatus::Installed => "installed",
        AvailabilityStatus::NotFound => "not_found",
    }
}

fn local_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(command_hostname)
}

fn command_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "localhost".to_owned())
}

fn io_error(error: std::io::Error) -> ServiceError {
    ServiceError::invalid_operation(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{create_sqlite_pool, run_migrations};
    use std::path::Path;

    async fn test_db() -> Arc<SqliteDb> {
        let pool = create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        run_migrations(&pool).await.expect("migrations run");
        Arc::new(SqliteDb::new(pool))
    }

    async fn test_daemon(forge_home: &Path) -> EmbeddedDaemon {
        EmbeddedDaemon::new(
            test_db().await,
            Arc::new(EventBus::new(16)),
            Arc::new(AdapterRegistry::new()),
            forge_home.to_path_buf(),
            forge_home.join("workspaces"),
        )
        .await
        .expect("embedded daemon creates")
    }

    #[tokio::test]
    async fn test_first_run_registers() {
        let tempdir = tempfile::tempdir().expect("tempdir creates");
        let daemon = test_daemon(tempdir.path()).await;

        let credentials = daemon
            .register_or_load_credentials()
            .await
            .expect("registers");

        assert!(!credentials.daemon_id.is_empty());
        assert!(tempdir.path().join(CREDENTIALS_FILE).exists());
    }

    #[tokio::test]
    async fn test_second_run_loads() {
        let tempdir = tempfile::tempdir().expect("tempdir creates");
        let daemon = test_daemon(tempdir.path()).await;

        let first = daemon
            .register_or_load_credentials()
            .await
            .expect("first register succeeds");
        let second = daemon
            .register_or_load_credentials()
            .await
            .expect("second load succeeds");

        assert_eq!(first.daemon_id, second.daemon_id);
    }

    #[tokio::test]
    async fn test_auth_failure_reregisters() {
        let tempdir = tempfile::tempdir().expect("tempdir creates");
        let first_daemon = test_daemon(tempdir.path()).await;
        let first = first_daemon
            .register_or_load_credentials()
            .await
            .expect("first register succeeds");

        let second_daemon = test_daemon(tempdir.path()).await;
        let second = second_daemon
            .register_or_load_credentials()
            .await
            .expect("reregister succeeds");

        assert_ne!(first.daemon_id, second.daemon_id);
        assert!(tempdir.path().join(CREDENTIALS_FILE).exists());
    }
}
