use crate::auditor::{self, AuditorVerdict};
use db::{
    new_uuid_v4, now_rfc3339, Agent, AgentRepo, AgentStatus, CreateExecution, CreateReview,
    Execution, ExecutionRepo, ExecutionStatus, RepoRepo, Review, ReviewRepo, ReviewStatus,
    SqliteDb, TaskRepo, UpdateExecution,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use executors::{
    resolve_config_value, AdapterExecutor, AdapterRegistry, ExecutionContext, ExecutionOutcome,
    ExecutionOverrides, LogEntry, LogKind, LogStream, LogWriter, TaskExecutor,
};
use serde_json::{json, Value};
use std::{path::PathBuf, process::ExitStatus, sync::Arc};
use thiserror::Error;
use tokio::process::Command;
use uuid::Uuid;

const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 64 * 1024;
const STDERR_TAIL_BYTES: usize = 4096;
const RESUME_THREAD_ID_CONFIG_KEY: &str = "resume_thread_id";

pub struct ReviewRunner {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    executor: Arc<dyn TaskExecutor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewOutcome {
    Passed,
    PassedCiOnly,
    AuditorFailed {
        reason: String,
    },
    CiFailed {
        failing_steps: Vec<StepResult>,
    },
    MergeConflict {
        conflict_paths: Vec<PathBuf>,
        conflict_summary: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    pub index: usize,
    pub command: String,
    pub exit_code: i32,
    pub stderr_tail: String,
    pub output_tail: String,
    pub started_at: String,
    pub finished_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRequest {
    pub task_id: Uuid,
    pub executor_execution_id: Uuid,
    pub workspace_path: PathBuf,
    pub ci_steps: Vec<String>,
    pub logs_path: String,
    pub auditor_agent_id: Option<String>,
    pub review_prompt: Option<String>,
    pub executor_thread_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum ReviewError {
    #[error(transparent)]
    Db(#[from] db::DbError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Serde(#[from] serde_json::Error),

    #[error(transparent)]
    Executor(#[from] executors::ExecutorError),

    #[error(transparent)]
    Git(#[from] git::GitError),

    #[error("executor execution not found: {0}")]
    ExecutorExecutionNotFound(Uuid),

    #[error("executor execution has no workspace: {0}")]
    ExecutorExecutionMissingWorkspace(Uuid),
}

impl ReviewRunner {
    pub fn new(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        adapter_registry: Arc<AdapterRegistry>,
    ) -> Self {
        Self {
            db,
            event_bus,
            executor: Arc::new(AdapterExecutor::new(adapter_registry)),
        }
    }

    #[cfg(test)]
    #[allow(dead_code)] // pre-existing warning, out of scope for this change
    fn new_for_tests(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        executor: Arc<dyn TaskExecutor>,
    ) -> Self {
        Self {
            db,
            event_bus,
            executor,
        }
    }

    pub async fn run(&self, req: ReviewRequest) -> Result<(Review, ReviewOutcome), ReviewError> {
        let task_id = req.task_id.to_string();
        let executor_execution_id = req.executor_execution_id.to_string();
        let attempt_number = ReviewRepo::next_attempt_number(&*self.db, &task_id).await?;
        let executor_execution = ExecutionRepo::get_by_id(&*self.db, &executor_execution_id)
            .await?
            .ok_or(ReviewError::ExecutorExecutionNotFound(
                req.executor_execution_id,
            ))?;
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or(db::DbError::NotFound)?;
        let ci_only_review = task.review_passed_at.is_some();
        let state_config = read_review_state_config(task.task_state_config.as_deref())?;
        let ci_steps = read_ci_steps(&state_config);
        let review_prompt = read_review_prompt(&state_config);
        let workspace_id = executor_execution.workspace_id.clone().ok_or(
            ReviewError::ExecutorExecutionMissingWorkspace(req.executor_execution_id),
        )?;

        let reviewer_execution = self
            .create_reviewer_execution(&task_id, &executor_execution_id, workspace_id.clone(), &req)
            .await?;
        let review = self
            .create_review(&task_id, &reviewer_execution.id, attempt_number)
            .await?;

        let (mut status, mut outcome, step_results, mut failed_step_index) = if ci_steps.is_empty()
        {
            (
                ReviewStatus::Passed,
                ReviewOutcome::Passed,
                Vec::new(),
                None,
            )
        } else {
            self.run_steps(&req, &reviewer_execution, &ci_steps).await?
        };

        let mut auditor_details = None;
        if status == ReviewStatus::Passed && ci_only_review {
            outcome = ReviewOutcome::PassedCiOnly;
            auditor_details = Some(AuditorDetails::pass_ci_only());
        } else if status == ReviewStatus::Passed {
            if let Some(result) = self
                .run_auditor(
                    &req,
                    &executor_execution,
                    workspace_id,
                    review_prompt.as_deref(),
                )
                .await?
            {
                status = result.status;
                outcome = result.outcome;
                failed_step_index = None;
                auditor_details = Some(result.details);
            }
        }

        let finished_at = now_rfc3339();
        let step_results_json = review_details_json(&step_results, auditor_details.as_ref())?;
        let review = ReviewRepo::update_status(
            &*self.db,
            &review.id,
            status,
            step_results_json,
            Some(finished_at.clone()),
            &finished_at,
        )
        .await?;

        match (&outcome, ci_only_review) {
            (ReviewOutcome::Passed, false) => {
                TaskRepo::set_review_passed_at(
                    &*self.db,
                    &task_id,
                    Some(finished_at.clone()),
                    &finished_at,
                )
                .await?;
            }
            (ReviewOutcome::CiFailed { .. }, true) => {
                TaskRepo::set_review_passed_at(&*self.db, &task_id, None, &finished_at).await?;
            }
            _ => {}
        }

        ExecutionRepo::update(
            &*self.db,
            UpdateExecution {
                id: reviewer_execution.id.clone(),
                status: Some(ExecutionStatus::Completed),
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: Some(Some(format!("review:{}", review.id))),
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                updated_at: finished_at,
            },
        )
        .await?;

        self.publish_review_event(&task_id, &review, outcome.clone(), failed_step_index);

        Ok((review, outcome))
    }

    async fn create_reviewer_execution(
        &self,
        task_id: &str,
        executor_execution_id: &str,
        workspace_id: String,
        req: &ReviewRequest,
    ) -> Result<Execution, ReviewError> {
        let now = now_rfc3339();
        ExecutionRepo::create(
            &*self.db,
            CreateExecution {
                id: new_uuid_v4(),
                task_id: task_id.to_owned(),
                agent_id: None,
                role: "reviewer".to_string(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: Some(executor_execution_id.to_owned()),
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: Some(req.logs_path.clone()),
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                workspace_id: Some(workspace_id),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .map_err(Into::into)
    }

    async fn create_review(
        &self,
        task_id: &str,
        execution_id: &str,
        attempt_number: i64,
    ) -> Result<Review, ReviewError> {
        let now = now_rfc3339();
        ReviewRepo::create(
            &*self.db,
            CreateReview {
                id: new_uuid_v4(),
                task_id: task_id.to_owned(),
                execution_id: execution_id.to_owned(),
                attempt_number,
                status: ReviewStatus::Running,
                step_results_json: "[]".to_owned(),
                started_at: now.clone(),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .map_err(Into::into)
    }

    async fn run_steps(
        &self,
        req: &ReviewRequest,
        reviewer_execution: &Execution,
        ci_steps: &[String],
    ) -> Result<(ReviewStatus, ReviewOutcome, Vec<StepResult>, Option<usize>), ReviewError> {
        let mut writer =
            LogWriter::new(&req.logs_path, reviewer_execution.id.clone(), MAX_LOG_BYTES);
        let mut step_results = Vec::new();

        for (index, step) in ci_steps.iter().enumerate() {
            let started_at = now_rfc3339();
            let output = Command::new("bash")
                .arg("-lc")
                .arg(step)
                .current_dir(&req.workspace_path)
                .output()
                .await?;
            let finished_at = now_rfc3339();

            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let combined_output = combined_output(&stdout, &stderr);
            let exit_code = exit_code(output.status);
            let result = StepResult {
                index,
                command: step.clone(),
                exit_code,
                stderr_tail: tail_bytes(&stderr, STDERR_TAIL_BYTES),
                output_tail: tail_bytes(&combined_output, STDERR_TAIL_BYTES),
                started_at,
                finished_at,
            };

            writer
                .write(
                    LogKind::ShellCommand,
                    LogStream::Main,
                    serde_json::json!({
                        "index": index,
                        "command": step,
                        "exit_code": exit_code,
                        "stdout": stdout,
                        "stderr": stderr,
                        "output": combined_output,
                    }),
                )
                .await?;

            step_results.push(result.clone());
            if exit_code != 0 {
                let failing_steps = vec![result];
                return Ok((
                    ReviewStatus::Failed,
                    ReviewOutcome::CiFailed { failing_steps },
                    step_results,
                    Some(index),
                ));
            }
        }

        Ok((
            ReviewStatus::Passed,
            ReviewOutcome::Passed,
            step_results,
            None,
        ))
    }

    async fn run_auditor(
        &self,
        req: &ReviewRequest,
        executor_execution: &Execution,
        workspace_id: String,
        review_prompt: Option<&str>,
    ) -> Result<Option<AuditorRunResult>, ReviewError> {
        let Some(auditor_agent_id) = req.auditor_agent_id.as_deref() else {
            return Ok(None);
        };

        let task_id = req.task_id.to_string();
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or(db::DbError::NotFound)?;
        let repo_id = task.repo_id.as_deref().ok_or(db::DbError::NotFound)?;
        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or(db::DbError::NotFound)?;
        let Some(auditor_agent) = self.load_auditor_agent(auditor_agent_id).await? else {
            return Ok(Some(AuditorRunResult::failed("auditor_agent_unavailable")));
        };

        let diff_text = read_git_diff(&req.workspace_path, &repo.default_branch).await?;
        let prompt = auditor::render_auditor_prompt(
            &task.title,
            task.description.as_deref(),
            &diff_text,
            review_prompt,
        );
        let auditor_execution_id = new_uuid_v4();
        let auditor_before_sha = git::get_current_sha(&req.workspace_path).await?;
        let auditor_logs_path = auditor_logs_path(&req.logs_path, &auditor_execution_id);
        let executor_type = executor_type_for_execution(&self.db, executor_execution).await?;
        let extra_config = auditor_resume_thread_extra_config(
            executor_execution,
            executor_type.as_deref(),
            &auditor_agent,
        );
        let snapshot = build_auditor_config_snapshot(&auditor_agent, extra_config).await?;
        let now = now_rfc3339();
        let auditor_execution = ExecutionRepo::create(
            &*self.db,
            CreateExecution {
                id: auditor_execution_id.clone(),
                task_id: task_id.clone(),
                agent_id: Some(auditor_agent.id.clone()),
                role: "auditor".to_string(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: Some(executor_execution.id.clone()),
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: Some(auditor_logs_path.clone()),
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: Some(snapshot.clone()),
                workspace_id: Some(workspace_id),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;

        let execution_result = self
            .executor
            .execute(ExecutionContext {
                task_id,
                execution_id: auditor_execution.id.clone(),
                worktree_path: req.workspace_path.display().to_string(),
                description: prompt,
                agent_config: serde_json::from_str(&snapshot)?,
                logs_path: auditor_logs_path.clone(),
                heartbeat_interval_seconds: heartbeat_interval(&auditor_agent),
                max_turns: None,
                log_sender: None,
            })
            .await;
        let restore_result = git::restore_worktree(&req.workspace_path, &auditor_before_sha)
            .await
            .map_err(|error| {
                executors::ExecutorError::Other(format!(
                    "failed to restore auditor worktree state: {error}"
                ))
            });
        let execution_result = match (execution_result, restore_result) {
            (_, Err(error)) => Err(error),
            (Ok(mut result), Ok(())) => {
                result.after_sha = Some(auditor_before_sha);
                Ok(result)
            }
            (Err(error), Ok(())) => Err(error),
        };

        let result = match execution_result {
            Ok(result) => result,
            Err(error) => {
                let finished_at = now_rfc3339();
                ExecutionRepo::update(
                    &*self.db,
                    UpdateExecution {
                        id: auditor_execution.id,
                        status: Some(ExecutionStatus::Failed),
                        stop_reason: None,
                        stopped_by: None,
                        resume_policy: None,
                        stopped_at: None,
                        agent_session_id: None,
                        agent_message_id: None,
                        last_activity_at: None,
                        summary: None,
                        logs_path: None,
                        before_sha: None,
                        after_sha: None,
                        error: Some(Some(error.to_string())),
                        executor_config_snapshot_json: None,
                        updated_at: finished_at,
                    },
                )
                .await?;
                return Ok(Some(AuditorRunResult::failed("auditor_execution_failed")));
            }
        };

        let execution_status = match result.status {
            ExecutionOutcome::Completed => ExecutionStatus::Completed,
            ExecutionOutcome::Failed => ExecutionStatus::Failed,
            ExecutionOutcome::Cancelled => ExecutionStatus::Cancelled,
        };
        let finished_at = now_rfc3339();
        ExecutionRepo::update(
            &*self.db,
            UpdateExecution {
                id: auditor_execution.id,
                status: Some(execution_status),
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                agent_session_id: Some(result.agent_session_id),
                agent_message_id: None,
                last_activity_at: None,
                summary: Some(result.summary),
                logs_path: None,
                before_sha: None,
                after_sha: Some(result.after_sha),
                error: Some(result.error.clone()),
                executor_config_snapshot_json: None,
                updated_at: finished_at,
            },
        )
        .await?;

        if result.status != ExecutionOutcome::Completed {
            return Ok(Some(AuditorRunResult::failed(
                result
                    .error
                    .as_deref()
                    .unwrap_or("auditor_execution_failed"),
            )));
        }

        let final_message = last_assistant_message(&auditor_logs_path).await?;
        Ok(Some(match auditor::parse_verdict(&final_message) {
            AuditorVerdict::Passed => AuditorRunResult {
                status: ReviewStatus::Passed,
                outcome: ReviewOutcome::Passed,
                details: AuditorDetails::passed(),
            },
            AuditorVerdict::Failed { reason } => AuditorRunResult::failed(reason),
        }))
    }

    async fn load_auditor_agent(
        &self,
        auditor_agent_id: &str,
    ) -> Result<Option<Agent>, ReviewError> {
        let Some(agent) = AgentRepo::get_by_id(&*self.db, auditor_agent_id).await? else {
            return Ok(None);
        };
        if !matches!(agent.status, AgentStatus::Idle | AgentStatus::Busy) {
            return Ok(None);
        }
        Ok(Some(agent))
    }

    fn publish_review_event(
        &self,
        task_id: &str,
        review: &Review,
        outcome: ReviewOutcome,
        failed_step_index: Option<usize>,
    ) {
        let (event_type, context) = match outcome {
            ReviewOutcome::Passed | ReviewOutcome::PassedCiOnly => (
                "review.passed",
                EventContext::ReviewPassed {
                    task_id: task_id.to_owned(),
                    review_id: review.id.clone(),
                    attempt_number: review.attempt_number,
                },
            ),
            ReviewOutcome::AuditorFailed { .. }
            | ReviewOutcome::CiFailed { .. }
            | ReviewOutcome::MergeConflict { .. } => (
                "review.failed",
                EventContext::ReviewFailed {
                    task_id: task_id.to_owned(),
                    review_id: review.id.clone(),
                    attempt_number: review.attempt_number,
                    failed_step_index: failed_step_index.unwrap_or(0),
                },
            ),
        };

        self.event_bus.publish(ForgeEvent {
            event_type: event_type.to_owned(),
            entity_id: review.id.clone(),
            timestamp: event_timestamp(),
            context,
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditorDetails {
    verdict: &'static str,
    reason: Option<String>,
}

impl AuditorDetails {
    fn passed() -> Self {
        Self {
            verdict: "pass",
            reason: None,
        }
    }

    fn pass_ci_only() -> Self {
        Self {
            verdict: "pass_ci_only",
            reason: Some("CI-only re-review".to_owned()),
        }
    }

    fn failed(reason: impl Into<String>) -> Self {
        Self {
            verdict: "fail",
            reason: Some(reason.into()),
        }
    }

    fn to_json(&self) -> Value {
        match &self.reason {
            Some(reason) => json!({
                "verdict": self.verdict,
                "reason": reason,
            }),
            None => json!({
                "verdict": self.verdict,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditorRunResult {
    status: ReviewStatus,
    outcome: ReviewOutcome,
    details: AuditorDetails,
}

impl AuditorRunResult {
    fn failed(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            status: ReviewStatus::Failed,
            outcome: ReviewOutcome::AuditorFailed {
                reason: reason.clone(),
            },
            details: AuditorDetails::failed(reason),
        }
    }
}

async fn read_git_diff(
    workspace_path: &std::path::Path,
    default_branch: &str,
) -> Result<String, ReviewError> {
    let branch_ref = format!("{default_branch}...HEAD");
    let output = Command::new("git")
        .arg("diff")
        .arg(branch_ref)
        .current_dir(workspace_path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await?;
    let stdout = if output.status.success() {
        output.stdout
    } else {
        Command::new("git")
            .arg("diff")
            .current_dir(workspace_path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .await?
            .stdout
    };
    Ok(truncate_utf8_bytes(&stdout, MAX_DIFF_BYTES))
}

fn truncate_utf8_bytes(bytes: &[u8], max_bytes: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= max_bytes {
        return text.into_owned();
    }

    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = text[..end].to_owned();
    truncated.push_str("[truncated]");
    truncated
}

async fn build_auditor_config_snapshot(
    agent: &Agent,
    extra_config: Option<Value>,
) -> Result<String, ReviewError> {
    let mut base_config = parse_json_value("agent config_json", &agent.config_json)?;
    apply_agent_fields_to_config(agent, &mut base_config)?;
    let capabilities = parse_json_value("agent capabilities_json", &agent.capabilities_json)?;
    let kind = agent
        .executor_type
        .parse()
        .map_err(executors::ExecutorError::Other)?;
    let execution_overrides = extra_config.unwrap_or_else(|| json!({}));
    let (merged_config, overrides_applied) =
        merge_config_layers(&base_config, &execution_overrides);
    let normalized_config =
        resolve_config_value(kind, &merged_config, &ExecutionOverrides::default())?;
    let overrides_applied = overrides_applied.retain_config_keys(&normalized_config);
    serde_json::to_string(&json!({
        "agent_id": agent.id,
        "executor_type": agent.executor_type,
        "model": agent.model,
        "reasoning_effort": agent.reasoning_effort,
        "permission_policy": agent.permission_policy,
        "config": normalized_config,
        "capabilities": capabilities,
        "overrides_applied": overrides_applied.to_json(),
        "snapshotted_at": now_rfc3339(),
    }))
    .map_err(Into::into)
}

fn auditor_resume_thread_extra_config(
    executor_execution: &Execution,
    executor_type: Option<&str>,
    auditor_agent: &Agent,
) -> Option<Value> {
    let thread_id = executor_execution.agent_session_id.as_deref()?;
    if executor_type == Some("codex") && auditor_agent.executor_type == "codex" {
        Some(json!({ RESUME_THREAD_ID_CONFIG_KEY: thread_id }))
    } else {
        None
    }
}

async fn executor_type_for_execution(
    db: &SqliteDb,
    executor_execution: &Execution,
) -> Result<Option<String>, ReviewError> {
    if let Some(snapshot) = executor_execution
        .executor_config_snapshot_json
        .as_deref()
        .and_then(|snapshot| serde_json::from_str::<Value>(snapshot).ok())
    {
        if let Some(executor_type) = snapshot.get("executor_type").and_then(Value::as_str) {
            return Ok(Some(executor_type.to_owned()));
        }
    }

    let Some(agent_id) = executor_execution.agent_id.as_deref() else {
        return Ok(None);
    };
    let Some(agent) = AgentRepo::get_by_id(db, agent_id).await? else {
        return Ok(None);
    };
    Ok(Some(agent.executor_type))
}

fn apply_agent_fields_to_config(agent: &Agent, config: &mut Value) -> Result<(), ReviewError> {
    let Some(config_object) = config.as_object_mut() else {
        return Err(ReviewError::Executor(executors::ExecutorError::Other(
            "agent config_json must be a JSON object".to_owned(),
        )));
    };
    if let Some(model) = &agent.model {
        config_object.insert("model".to_owned(), Value::String(model.clone()));
    }
    if let Some(reasoning_effort) = &agent.reasoning_effort {
        config_object.insert(
            "model_reasoning_effort".to_owned(),
            Value::String(reasoning_effort.clone()),
        );
        config_object.insert("effort".to_owned(), Value::String(reasoning_effort.clone()));
    }
    if let Some(permission_policy) = &agent.permission_policy {
        config_object.insert(
            "permission_policy".to_owned(),
            Value::String(permission_policy.clone()),
        );
    }
    Ok(())
}

fn heartbeat_interval(agent: &Agent) -> u64 {
    u64::try_from(agent.heartbeat_interval_seconds)
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(30)
}

fn auditor_logs_path(reviewer_logs_path: &str, auditor_execution_id: &str) -> String {
    let path = std::path::Path::new(reviewer_logs_path);
    path.parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!("{auditor_execution_id}.jsonl"))
        .display()
        .to_string()
}

async fn last_assistant_message(logs_path: &str) -> Result<String, ReviewError> {
    let contents = match tokio::fs::read_to_string(logs_path).await {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error.into()),
    };
    let mut message = String::new();
    for line in contents.lines() {
        let Ok(entry) = serde_json::from_str::<LogEntry>(line) else {
            continue;
        };
        if entry.kind == LogKind::Assistant {
            append_assistant_log_text(&entry.payload, &mut message);
        } else if entry.kind == LogKind::SessionInfo
            && entry.payload.get("subtype").and_then(Value::as_str) == Some("success")
        {
            if let Some(result) = entry.payload.get("result").and_then(Value::as_str) {
                message.push_str(result);
            }
        }
    }
    Ok(message)
}

fn append_assistant_log_text(payload: &Value, message: &mut String) {
    if let Some(text) = payload.get("text").and_then(Value::as_str) {
        message.push_str(text);
    }

    let Some(content) = payload
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return;
    };

    for item in content {
        if item.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                message.push_str(text);
            }
        }
    }
}

fn combined_output(stdout: &str, stderr: &str) -> String {
    let mut output = String::with_capacity(stdout.len() + stderr.len());
    output.push_str(stdout);
    output.push_str(stderr);
    output
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

fn review_details_json(
    results: &[StepResult],
    auditor: Option<&AuditorDetails>,
) -> Result<String, serde_json::Error> {
    let ci_steps = step_results_value(results);
    match auditor {
        Some(auditor) => serde_json::to_string(&json!({
            "ci_steps": ci_steps,
            "auditor": auditor.to_json(),
        })),
        None => serde_json::to_string(&ci_steps),
    }
}

fn step_results_value(results: &[StepResult]) -> Value {
    Value::Array(
        results
            .iter()
            .map(|result| {
                json!({
                    "index": result.index,
                    "command": result.command,
                    "exit_code": result.exit_code,
                    "stderr_tail": result.stderr_tail,
                    "output_tail": result.output_tail,
                    "started_at": result.started_at,
                    "finished_at": result.finished_at,
                })
            })
            .collect(),
    )
}

fn read_review_state_config(task_state_config: Option<&str>) -> Result<Value, ReviewError> {
    let Some(raw_config) = task_state_config else {
        return Ok(json!({}));
    };
    if raw_config.trim().is_empty() {
        return Ok(json!({}));
    }

    let value: Value = serde_json::from_str(raw_config)?;
    Ok(value.get("review").cloned().unwrap_or(value))
}

fn read_ci_steps(state_config: &Value) -> Vec<String> {
    state_config
        .get("ci_steps")
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn read_review_prompt(state_config: &Value) -> Option<String> {
    state_config
        .get("review_prompt")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn parse_json_value(field: &str, value: &str) -> Result<Value, ReviewError> {
    serde_json::from_str(value).map_err(|error| {
        serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid {field}: {error}"),
        ))
        .into()
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OverridesApplied {
    agent: Vec<String>,
    execution: Vec<String>,
}

impl OverridesApplied {
    fn to_json(&self) -> Value {
        json!({
            "agent": self.agent,
            "execution": self.execution,
        })
    }

    fn retain_config_keys(mut self, config: &Value) -> Self {
        let Some(config_object) = config.as_object() else {
            self.agent.clear();
            self.execution.clear();
            return self;
        };

        self.agent
            .retain(|key| config_object.contains_key(key.as_str()));
        self.execution
            .retain(|key| config_object.contains_key(key.as_str()));
        self
    }
}

fn merge_config_layers(agent: &Value, execution: &Value) -> (Value, OverridesApplied) {
    let mut merged = agent.clone();
    let mut overrides_applied = OverridesApplied {
        agent: object_keys(agent),
        execution: Vec::new(),
    };

    merge_override_layer(&mut merged, execution, &mut overrides_applied.execution);

    (merged, overrides_applied)
}

fn object_keys(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

fn merge_override_layer(merged: &mut Value, layer: &Value, applied_keys: &mut Vec<String>) {
    let Some(layer_object) = layer.as_object() else {
        return;
    };
    let Some(merged_object) = merged.as_object_mut() else {
        return;
    };
    for (key, value) in layer_object {
        merged_object.insert(key.clone(), value.clone());
        applied_keys.push(key.clone());
    }
}

fn tail_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }

    let mut start = value.len() - max_bytes;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_owned()
}

#[cfg(test)]
mod tests;
