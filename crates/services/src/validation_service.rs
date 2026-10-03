#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use db::{
    new_uuid_v4, now_rfc3339, CreateDomainEvent, CreateEvidence, CreateValidationRun,
    CreateValidationRunArtifact, Evidence, ExecutionRepo, TaskRepo, ValidationRun,
    ValidationRunRepo, ValidationRunStatus, WorkspaceRepo, WorkspaceStatus,
};
use events::EventBus;
use sha2::{Digest, Sha256};
use tokio::{io::AsyncReadExt, process::Command, time::Instant};

use crate::{domain_event_service::DomainEventService, Result, ServiceError};

const OUTPUT_TAIL_LIMIT: usize = 8 * 1024;
const CLAIM_SECONDS: i64 = 60;
const HEARTBEAT_SECONDS: u64 = 20;
const COMMAND_TIMEOUT_SECONDS: u64 = 60 * 60;
// The HEAD-to-index and HEAD-to-worktree diffs share one total byte budget.
const MAX_TRACKED_DIFF_BYTES: usize = 256 * 1024 * 1024;
const MAX_UNTRACKED_PATH_BYTES: usize = 16 * 1024 * 1024;
const MAX_UNTRACKED_FILES: usize = 100_000;
const MAX_UNTRACKED_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_UNTRACKED_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ValidationService {
    db: Arc<db::SqliteDb>,
    event_bus: Arc<EventBus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCheckResult {
    pub run: ValidationRun,
    pub evidence: Vec<Evidence>,
}

impl ValidationService {
    pub fn new(db: Arc<db::SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self { db, event_bus }
    }

    pub(crate) async fn snapshot_digest(worktree_path: &str) -> Result<String> {
        workspace_snapshot_digest(worktree_path).await
    }

    /// Execute one configured deterministic shell check against one frozen
    /// Workspace/commit identity. Retries reuse the same logical run key.
    pub async fn run_command(
        &self,
        task_id: &str,
        workspace_id: &str,
        command: &str,
        check_index: usize,
        caused_by_execution_id: Option<&str>,
        workspace_locks: Option<&crate::WorkspaceExecutionLockManager>,
    ) -> Result<ValidationCheckResult> {
        if command.trim().is_empty() || command.len() > 8192 {
            return Err(ServiceError::invalid_operation(
                "validation command must contain between 1 and 8192 bytes",
            ));
        }
        let _workspace_guard = match workspace_locks {
            Some(locks) => Some(locks.acquire(workspace_id).await),
            None => None,
        };
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
        if workspace.task_id != task_id || workspace.status != WorkspaceStatus::Ready {
            return Err(ServiceError::invalid_operation(
                "ValidationRun requires a Ready Workspace belonging to the exact Task",
            ));
        }

        let initial_commit = read_head(&workspace.worktree_path).await?;
        let environment = capture_environment()?;
        let initial_snapshot_digest = workspace_snapshot_digest(&workspace.worktree_path).await?;
        let environment_digest = digest_json(
            &environment
                .iter()
                .map(|(key, value)| (key.as_str(), hex::encode(Sha256::digest(value.as_bytes()))))
                .collect::<Vec<_>>(),
        )?;
        let command_digest = hex::encode(Sha256::digest(command.as_bytes()));
        let check_identity = format!("ci:{check_index}:{command_digest}");
        let config_summary_json = serde_json::json!({
            "shell": "bash -c",
            "environment_digest": environment_digest,
            "environment_variables": environment.len(),
            "output_tail_bytes_per_stream": OUTPUT_TAIL_LIMIT,
            "timeout_seconds": COMMAND_TIMEOUT_SECONDS,
        })
        .to_string();
        if config_summary_json.len() > 8192 {
            return Err(ServiceError::invalid_operation(
                "validation environment summary exceeds its storage bound",
            ));
        }
        let config_digest = hex::encode(Sha256::digest(config_summary_json.as_bytes()));
        let idempotency_key = digest_json(&(
            task_id,
            workspace_id,
            initial_commit.as_str(),
            initial_snapshot_digest.as_str(),
            caused_by_execution_id,
            check_identity.as_str(),
            config_digest.as_str(),
        ))?;
        if let Some(existing) =
            ValidationRunRepo::get_validation_run_by_idempotency_key(&*self.db, &idempotency_key)
                .await?
        {
            if existing.status != ValidationRunStatus::Running {
                let evidence =
                    ValidationRunRepo::list_evidence_for_validation_run(&*self.db, &existing.id)
                        .await?;
                return Ok(ValidationCheckResult {
                    run: existing,
                    evidence,
                });
            }
        }

        let now = now_rfc3339();
        let run_id = new_uuid_v4();
        let work_unit_id = if let Some(execution_id) = caused_by_execution_id {
            ExecutionRepo::get_by_id(&*self.db, execution_id)
                .await?
                .filter(|execution| execution.task_id == task_id)
                .and_then(|execution| execution.work_unit_id)
        } else {
            None
        };
        let start = ValidationRunRepo::start_validation_run(
            &*self.db,
            CreateValidationRun {
                id: run_id.clone(),
                task_id: task_id.to_owned(),
                work_unit_id,
                caused_by_execution_id: caused_by_execution_id.map(str::to_owned),
                check_identity: check_identity.clone(),
                command: command.to_owned(),
                config_summary_json: config_summary_json.clone(),
                config_digest: config_digest.clone(),
                workspace_id: workspace_id.to_owned(),
                commit_sha: initial_commit.clone(),
                workspace_snapshot_digest: initial_snapshot_digest.clone(),
                idempotency_key: idempotency_key.clone(),
                started_at: now.clone(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            event(
                "validation_run.started",
                "validation_run",
                &run_id,
                &task.id,
                &task.project_id,
                &format!("validation-run-started:{idempotency_key}"),
                serde_json::json!({
                    "validation_run_id": run_id,
                    "task_id": task.id,
                    "project_id": task.project_id,
                    "workspace_id": workspace_id,
                    "commit_sha": initial_commit,
                    "workspace_snapshot_digest": initial_snapshot_digest,
                    "check_identity": check_identity,
                }),
                &now,
            ),
        )
        .await?;
        if let Some(event) = start.event.as_ref() {
            DomainEventService::publish_committed_hint(&self.event_bus, event);
        }
        if start.validation_run.status != ValidationRunStatus::Running {
            let evidence = ValidationRunRepo::list_evidence_for_validation_run(
                &*self.db,
                &start.validation_run.id,
            )
            .await?;
            return Ok(ValidationCheckResult {
                run: start.validation_run,
                evidence,
            });
        }

        let owner = new_uuid_v4();
        let claim_start = now_rfc3339();
        let claim_until = timestamp_after(CLAIM_SECONDS);
        if !ValidationRunRepo::claim_validation_run(
            &*self.db,
            &start.validation_run.id,
            &owner,
            &claim_start,
            &claim_until,
        )
        .await?
        {
            return Err(ServiceError::invalid_operation(
                "ValidationRun is already claimed by another active check",
            ));
        }

        let execution = run_bounded_command(
            &*self.db,
            &start.validation_run.id,
            &owner,
            &workspace.worktree_path,
            command,
            &environment,
        )
        .await;
        let finished_at = now_rfc3339();
        let observed_commit = read_head(&workspace.worktree_path).await.ok();
        let observed_snapshot_digest = workspace_snapshot_digest(&workspace.worktree_path)
            .await
            .ok();
        let (status, exit_code, stdout_tail, stderr_tail, stdout_bytes, stderr_bytes, spawn_error) =
            match execution {
                Ok(result) => {
                    let status = match (
                        result.exit_code,
                        observed_commit.as_deref(),
                        observed_snapshot_digest.as_deref(),
                    ) {
                        (_, Some(observed), _) if observed != initial_commit => {
                            ValidationRunStatus::Stale
                        }
                        (_, _, Some(observed)) if observed != initial_snapshot_digest => {
                            ValidationRunStatus::Stale
                        }
                        (Some(0), Some(_), Some(_)) => ValidationRunStatus::Passed,
                        (Some(_), Some(_), Some(_)) => ValidationRunStatus::Failed,
                        _ => ValidationRunStatus::Error,
                    };
                    (
                        status,
                        result.exit_code,
                        result.stdout_tail,
                        result.stderr_tail,
                        result.stdout_bytes,
                        result.stderr_bytes,
                        result.spawn_error,
                    )
                }
                Err(error) => (
                    ValidationRunStatus::Error,
                    None,
                    String::new(),
                    error.to_string(),
                    0,
                    error.to_string().len(),
                    Some(error.to_string()),
                ),
            };
        let evidence_id = new_uuid_v4();
        let evidence_content = serde_json::json!({
            "validation_run_id": start.validation_run.id,
            "task_id": task.id,
            "check_identity": check_identity,
            "command": command,
            "config_digest": config_digest,
            "workspace_id": workspace_id,
            "commit_sha": initial_commit,
            "workspace_snapshot_digest": initial_snapshot_digest,
            "observed_commit_after": observed_commit,
            "observed_snapshot_digest_after": observed_snapshot_digest,
            "status": status,
            "exit_code": exit_code,
            "started_at": start.validation_run.started_at,
            "finished_at": finished_at,
            "stdout_tail": bounded(&stdout_tail),
            "stderr_tail": bounded(&stderr_tail),
            "stdout_bytes": stdout_bytes,
            "stderr_bytes": stderr_bytes,
            "spawn_error": spawn_error,
        })
        .to_string();
        if evidence_content.len() > 32768 {
            return Err(ServiceError::invalid_operation(
                "Validation Evidence content exceeds its durable storage bound",
            ));
        }
        let evidence_digest = hex::encode(Sha256::digest(evidence_content.as_bytes()));
        let evidence_key = format!("check:{check_index}");
        let evidence = CreateEvidence {
            id: evidence_id.clone(),
            task_id: task_id.to_owned(),
            validation_run_id: start.validation_run.id.clone(),
            evidence_key,
            kind: "deterministic_check_output".to_owned(),
            content_json: evidence_content,
            digest: evidence_digest,
            created_at: finished_at.clone(),
        };
        let report_content = serde_json::json!({
            "kind": "validation_report",
            "validation_run_id": start.validation_run.id,
            "task_id": task.id,
            "check_identity": check_identity,
            "workspace_id": workspace_id,
            "commit_sha": initial_commit,
            "workspace_snapshot_digest": initial_snapshot_digest,
            "status": status,
            "exit_code": exit_code,
            "evidence_ids": [evidence_id],
        })
        .to_string();
        let report_id = new_uuid_v4();
        let report = CreateValidationRunArtifact {
            id: report_id.clone(),
            task_id: task_id.to_owned(),
            validation_run_id: start.validation_run.id.clone(),
            digest: hex::encode(Sha256::digest(report_content.as_bytes())),
            content: report_content,
            metadata_json: serde_json::json!({
                "source": "deterministic_validation",
                "validation_run_id": start.validation_run.id,
            })
            .to_string(),
            created_at: finished_at.clone(),
        };
        let terminal_event_type = if status == ValidationRunStatus::Passed {
            "validation_run.completed"
        } else {
            "validation_run.failed"
        };
        let completion = ValidationRunRepo::finish_validation_run(
            &*self.db,
            db::FinishValidationRun {
                id: start.validation_run.id.clone(),
                claim_owner: owner,
                status,
                exit_code,
                finished_at: finished_at.clone(),
                logs_ref: format!("validation-evidence://{evidence_id}"),
                evidence: vec![evidence],
                validation_report: Some(report),
                events: vec![
                    event(
                        terminal_event_type,
                        "validation_run",
                        &start.validation_run.id,
                        task_id,
                        &task.project_id,
                        &format!("validation-run-terminal:{}", start.validation_run.id),
                        serde_json::json!({
                            "validation_run_id": start.validation_run.id,
                            "task_id": task.id,
                            "project_id": task.project_id,
                            "status": status,
                            "exit_code": exit_code,
                        }),
                        &finished_at,
                    ),
                    event(
                        "evidence.created",
                        "evidence",
                        &evidence_id,
                        task_id,
                        &task.project_id,
                        &format!("validation-evidence:{evidence_id}"),
                        serde_json::json!({
                            "evidence_id": evidence_id,
                            "validation_run_id": start.validation_run.id,
                            "task_id": task.id,
                            "project_id": task.project_id,
                            "kind": "deterministic_check_output",
                        }),
                        &finished_at,
                    ),
                    event(
                        "artifact.created",
                        "artifact",
                        &report_id,
                        task_id,
                        &task.project_id,
                        &format!("validation-report:{report_id}"),
                        serde_json::json!({
                            "artifact_id": report_id,
                            "validation_run_id": start.validation_run.id,
                            "task_id": task.id,
                            "project_id": task.project_id,
                            "kind": "validation_report",
                        }),
                        &finished_at,
                    ),
                ],
            },
        )
        .await?;
        for event in &completion.events {
            DomainEventService::publish_committed_hint(&self.event_bus, event);
        }
        Ok(ValidationCheckResult {
            run: completion.validation_run,
            evidence: completion.evidence,
        })
    }
}

struct CommandOutput {
    exit_code: Option<i32>,
    stdout_tail: String,
    stderr_tail: String,
    stdout_bytes: usize,
    stderr_bytes: usize,
    spawn_error: Option<String>,
}

async fn run_bounded_command(
    db: &db::SqliteDb,
    run_id: &str,
    owner: &str,
    worktree_path: &str,
    command: &str,
    environment: &[(String, String)],
) -> std::result::Result<CommandOutput, std::io::Error> {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(worktree_path)
        .env_clear()
        .envs(environment.iter().cloned())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout pipe configured");
    let stderr = child.stderr.take().expect("stderr pipe configured");
    let stdout_task = tokio::spawn(read_tail(stdout));
    let stderr_task = tokio::spawn(read_tail(stderr));
    let mut heartbeat = tokio::time::interval_at(
        Instant::now() + Duration::from_secs(HEARTBEAT_SECONDS),
        Duration::from_secs(HEARTBEAT_SECONDS),
    );
    let timeout = tokio::time::sleep(Duration::from_secs(COMMAND_TIMEOUT_SECONDS));
    tokio::pin!(timeout);
    let status = loop {
        tokio::select! {
            result = child.wait() => break Some(result?),
            _ = heartbeat.tick() => {
                let now = now_rfc3339();
                let until = timestamp_after(CLAIM_SECONDS);
                let active = db::ValidationRunRepo::heartbeat_validation_run(
                    db, run_id, owner, &now, &until
                ).await.map_err(|error| std::io::Error::other(error.to_string()))?;
                if !active {
                    let _ = child.kill().await;
                    return Err(std::io::Error::other("ValidationRun claim was lost"));
                }
            }
            _ = &mut timeout => {
                let _ = child.kill().await;
                break child.wait().await.ok();
            }
        }
    };
    let (stdout_tail, stdout_bytes) = stdout_task
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))??;
    let (stderr_tail, stderr_bytes) = stderr_task
        .await
        .map_err(|error| std::io::Error::other(error.to_string()))??;
    let exit_code = status.and_then(|status| status.code());
    Ok(CommandOutput {
        exit_code,
        stdout_tail,
        stderr_tail,
        stdout_bytes,
        stderr_bytes,
        spawn_error: None,
    })
}

async fn read_tail<R>(mut reader: R) -> std::io::Result<(String, usize)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut tail = Vec::with_capacity(OUTPUT_TAIL_LIMIT);
    let mut chunk = [0u8; 4096];
    let mut total = 0usize;
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        if read >= OUTPUT_TAIL_LIMIT {
            tail.clear();
            tail.extend_from_slice(&chunk[read - OUTPUT_TAIL_LIMIT..read]);
        } else {
            let overflow = tail
                .len()
                .saturating_add(read)
                .saturating_sub(OUTPUT_TAIL_LIMIT);
            if overflow > 0 {
                tail.drain(..overflow);
            }
            tail.extend_from_slice(&chunk[..read]);
        }
    }
    Ok((String::from_utf8_lossy(&tail).into_owned(), total))
}

fn capture_environment() -> Result<Vec<(String, String)>> {
    let mut environment = std::env::vars().collect::<Vec<_>>();
    environment.sort_by(|left, right| left.0.cmp(&right.0));
    if environment.len() > 2048 {
        return Err(ServiceError::invalid_operation(
            "validation process environment exceeds the configured bound",
        ));
    }
    Ok(environment)
}

async fn read_head(worktree_path: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(worktree_path)
        .env_clear()
        .envs(capture_environment()?)
        .output()
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("could not read workspace commit: {error}"))
        })?;
    if !output.status.success() {
        return Err(ServiceError::invalid_operation(
            "could not freeze ValidationRun commit identity",
        ));
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !(7..=128).contains(&sha.len()) || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ServiceError::invalid_operation(
            "workspace returned an invalid commit identity",
        ));
    }
    Ok(sha)
}

pub(crate) async fn workspace_snapshot_digest(worktree_path: &str) -> Result<String> {
    let environment = capture_environment()?;
    let (staged_bytes, staged_digest) = bounded_git_diff_digest(
        worktree_path,
        &[
            "diff",
            "--cached",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            "--",
        ],
        &environment,
        MAX_TRACKED_DIFF_BYTES as u64,
        "staged workspace files",
    )
    .await?;
    let remaining_tracked_bytes = (MAX_TRACKED_DIFF_BYTES as u64).saturating_sub(staged_bytes);
    let (tracked_bytes, tracked_digest) = bounded_git_diff_digest(
        worktree_path,
        &[
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            "--",
        ],
        &environment,
        remaining_tracked_bytes,
        "tracked workspace files",
    )
    .await?;

    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .current_dir(worktree_path)
        .env_clear()
        .envs(environment.iter().cloned())
        .output()
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!(
                "could not list untracked workspace files: {error}"
            ))
        })?;
    if !untracked.status.success() || untracked.stdout.len() > MAX_UNTRACKED_PATH_BYTES {
        return Err(ServiceError::invalid_operation(
            "untracked workspace file list is unavailable or exceeds its bound",
        ));
    }
    let mut paths = untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect::<Vec<_>>();
    if paths.len() > MAX_UNTRACKED_FILES {
        return Err(ServiceError::invalid_operation(
            "untracked workspace file count exceeds its bound",
        ));
    }
    paths.sort();

    let mut digest = Sha256::new();
    digest.update(b"forge-workspace-snapshot-v2\0");
    digest.update(b"section:head-to-index\0");
    digest.update(staged_bytes.to_be_bytes());
    digest.update(staged_digest);
    digest.update(b"section:head-to-worktree\0");
    digest.update((tracked_bytes as u64).to_be_bytes());
    digest.update(tracked_digest);
    digest.update(b"section:untracked\0");
    digest.update((paths.len() as u64).to_be_bytes());
    let root = Path::new(worktree_path);
    let mut total_bytes = 0u64;
    for raw_path in paths {
        let relative = std::str::from_utf8(&raw_path).map_err(|_| {
            ServiceError::invalid_operation("untracked workspace contains a non-UTF8 path")
        })?;
        let relative_path = PathBuf::from(relative);
        if relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(ServiceError::invalid_operation(
                "git returned an unsafe untracked workspace path",
            ));
        }
        let full_path = root.join(&relative_path);
        let metadata = tokio::fs::symlink_metadata(&full_path)
            .await
            .map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "could not inspect untracked workspace file: {error}"
                ))
            })?;
        digest.update(b"entry\0");
        digest.update((raw_path.len() as u64).to_be_bytes());
        digest.update(&raw_path);
        if metadata.file_type().is_symlink() {
            let target = tokio::fs::read_link(&full_path).await.map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "could not read untracked workspace symlink: {error}"
                ))
            })?;
            let target = target.to_str().ok_or_else(|| {
                ServiceError::invalid_operation("untracked workspace symlink target is non-UTF8")
            })?;
            digest.update(b"type:symlink\0");
            digest.update((target.len() as u64).to_be_bytes());
            digest.update(Sha256::digest(target.as_bytes()));
        } else if metadata.is_file() {
            if metadata.len() > MAX_UNTRACKED_FILE_BYTES
                || total_bytes.saturating_add(metadata.len()) > MAX_UNTRACKED_TOTAL_BYTES
            {
                return Err(ServiceError::invalid_operation(
                    "untracked workspace file contents exceed the snapshot bound",
                ));
            }
            digest.update(b"type:file\0");
            digest.update(metadata.len().to_be_bytes());
            #[cfg(unix)]
            digest.update(metadata.permissions().mode().to_be_bytes());
            #[cfg(not(unix))]
            digest.update([u8::from(metadata.permissions().readonly())]);
            let file = tokio::fs::File::open(&full_path).await.map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "could not read untracked workspace file: {error}"
                ))
            })?;
            let mut content_digest = Sha256::new();
            let read = hash_reader_into(file, &mut content_digest, metadata.len())
                .await
                .map_err(|error| {
                    ServiceError::invalid_operation(format!(
                        "could not hash untracked workspace file: {error}"
                    ))
                })?;
            if read != metadata.len() {
                return Err(ServiceError::invalid_operation(
                    "untracked workspace file changed while its identity was read",
                ));
            }
            digest.update(content_digest.finalize());
            total_bytes += read;
        } else {
            return Err(ServiceError::invalid_operation(
                "untracked workspace contains a non-file entry",
            ));
        }
    }
    Ok(hex::encode(digest.finalize()))
}

async fn bounded_git_diff_digest(
    worktree_path: &str,
    args: &[&str],
    environment: &[(String, String)],
    max_bytes: u64,
    subject: &str,
) -> Result<(u64, Vec<u8>)> {
    let mut command = Command::new("git")
        .args(args)
        .current_dir(worktree_path)
        .env_clear()
        .envs(environment.iter().cloned())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            ServiceError::invalid_operation(format!("could not snapshot {subject}: {error}"))
        })?;
    let mut content_digest = Sha256::new();
    let stdout = command.stdout.take().expect("git stdout was piped");
    let byte_count = hash_reader_into(stdout, &mut content_digest, max_bytes)
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("could not hash {subject}: {error}"))
        })?;
    if !command
        .wait()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
    {
        return Err(ServiceError::invalid_operation(format!(
            "could not snapshot {subject} within the shared tracked-state bound"
        )));
    }
    Ok((byte_count, content_digest.finalize().to_vec()))
}

async fn hash_reader_into<R>(
    mut reader: R,
    digest: &mut Sha256,
    max_bytes: u64,
) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunk = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > max_bytes {
            return Err(std::io::Error::other(
                "workspace snapshot size exceeds its bound",
            ));
        }
        digest.update(&chunk[..read]);
    }
    Ok(total)
}

fn timestamp_after(seconds: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(seconds)).to_rfc3339()
}

fn bounded(value: &str) -> String {
    if value.len() <= OUTPUT_TAIL_LIMIT {
        return value.to_owned();
    }
    let mut start = value.len() - OUTPUT_TAIL_LIMIT;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_owned()
}

fn digest_json(value: &impl serde::Serialize) -> Result<String> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "validation identity serialization failed: {error}"
        ))
    })?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn event(
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    task_id: &str,
    project_id: &str,
    dedupe_key: &str,
    payload: serde_json::Value,
    created_at: &str,
) -> CreateDomainEvent {
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: entity_type.to_owned(),
        entity_id: entity_id.to_owned(),
        actor_type: "validation_service".to_owned(),
        actor_id: None,
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: task_id.to_owned(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: Some(dedupe_key.to_owned()),
        payload_json: serde_json::json!({
            "entity_type": entity_type,
            "entity_id": entity_id,
            "task_id": task_id,
            "project_id": project_id,
            "details": payload,
        })
        .to_string(),
        created_at: created_at.to_owned(),
    }
}
