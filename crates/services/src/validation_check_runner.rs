//! Durable runner for catalog validation check requests (plan
//! coordinator-verification-and-pr-loop, node C2).
//!
//! `enqueue_check_request` resolves the exact subject and authorization from
//! durable records and inserts a queued `validation_check_request`. The runner
//! claims queued rows (and recovers `running` rows after a restart), executes
//! the pinned catalog command through the shared `ValidationService` core under
//! the Workspace execution lock, and, for `baseline=true`, repeats the same
//! command in a runner-owned disposable detached git worktree at the exact base
//! commit (no `workspace` row, decision C2) and records the immutable
//! `validation_comparison`. The coordinator never supplies a command, path,
//! commit, workspace, or timeout.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use db::{
    new_uuid_v4, now_rfc3339, CreateValidationCheckRequest, CreateValidationComparison,
    ExecutionRepo, SqliteDb, TaskCoordinationMode, TaskCoordinationRepo, TaskRepo,
    TransitionValidationCheckRequest, ValidationCatalogRepo, ValidationCheckDefinition,
    ValidationCheckRequest, ValidationCheckRequestRepo, ValidationCheckRequestState,
    ValidationCheckRequestTrigger, ValidationCheckRequestWrite, ValidationCheckSubjectKind,
    ValidationComparisonRepo, ValidationRun, ValidationRunOrigin, ValidationRunStatus,
    WorkUnitRepo, WorkUnitStatus, Workspace, WorkspaceRepo, WorkspaceStatus,
};
use events::EventBus;
use tokio::{process::Command, sync::watch, task::JoinHandle, time::interval};
use tracing::warn;

use crate::{
    domain_event_service::DomainEventService,
    task_coordination::CoordinationEnvelope,
    validation_catalog::{resolve_pin, validation_error},
    validation_service::{
        digest_json, event, output_digest, read_head, run_baseline_command, CatalogRunIdentity,
        CheckRunInput,
    },
    Result, ServiceError, ValidationService, WorkspaceExecutionLockManager,
};

const TICK_INTERVAL: Duration = Duration::from_secs(5);
const BATCH_LIMIT: i64 = 16;
const MAX_REASON_BYTES: usize = 1024;
const BASELINE_DIR: &str = "validation-baselines";

/// Exact request to run one pinned catalog check. The subject is resolved by
/// Forge from durable records; callers pass only its identity and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueCheckRequest {
    pub task_id: String,
    pub repo_id: String,
    pub check_key: String,
    pub trigger: ValidationCheckRequestTrigger,
    pub subject_kind: ValidationCheckSubjectKind,
    /// `work_unit_result`: the WorkUnit id; `task_integration`: the Task id.
    pub subject_id: String,
    /// `work_unit_result`: the WorkUnit version; `task_integration`: the Task version.
    pub subject_version: i64,
    pub baseline: bool,
    /// Required for `CoordinatorRequest`, forbidden for `AfterIntegration`.
    pub requested_by_request_id: Option<String>,
    pub requested_by_execution_id: Option<String>,
    pub reason: Option<String>,
}

struct ResolvedSubject {
    workspace: Workspace,
    commit_sha: String,
    snapshot_digest: String,
}

fn result_for_repository<'a>(
    results: &'a [db::ExecutionRepositoryResult],
    repo_id: &str,
) -> Option<&'a db::ExecutionRepositoryResult> {
    results.iter().find(|result| result.repo_id == repo_id)
}

async fn resolve_subject(
    db: &SqliteDb,
    task: &db::Task,
    kind: ValidationCheckSubjectKind,
    subject_id: &str,
    subject_version: i64,
    repo_id: &str,
) -> Result<ResolvedSubject> {
    let workspace = match kind {
        ValidationCheckSubjectKind::WorkUnitResult => {
            let unit = WorkUnitRepo::get_by_id(db, subject_id)
                .await?
                .filter(|unit| unit.task_id == task.id)
                .ok_or_else(|| ServiceError::not_found("work_unit", subject_id.to_owned()))?;
            if unit.version != subject_version {
                return Err(validation_error(
                    "validation.subject_stale",
                    "the WorkUnit version no longer matches the requested subject",
                ));
            }
            if unit.status != WorkUnitStatus::Completed {
                return Err(validation_error(
                    "validation.subject_unavailable",
                    "only a completed WorkUnit result can be validated",
                ));
            }
            let execution_id: Option<String> = sqlx::query_scalar(
                "SELECT e.id FROM execution e
                 JOIN execution_repository_result rr ON rr.execution_id=e.id
                 WHERE e.task_id=? AND e.work_unit_id=? AND e.work_unit_version=?
                   AND e.status='completed' AND rr.repo_id=?
                 ORDER BY e.created_at DESC, e.id DESC LIMIT 1",
            )
            .bind(&task.id)
            .bind(&unit.id)
            .bind(subject_version)
            .bind(repo_id)
            .fetch_optional(db.pool())
            .await?;
            let execution_id = execution_id.ok_or_else(|| {
                validation_error(
                    "validation.subject_unavailable",
                    "the exact WorkUnit result has no repository result for the requested repo",
                )
            })?;
            let results = ExecutionRepo::list_repository_results(db, &execution_id).await?;
            let result = result_for_repository(&results, repo_id)
                .ok_or_else(|| {
                    validation_error(
                        "validation.subject_unavailable",
                        "the exact WorkUnit result has no repository result for the requested repo",
                    )
                })?;
            let workspace = WorkspaceRepo::get_by_id(db, &result.workspace_id)
                .await?
                .filter(|workspace| {
                    workspace.task_id == task.id
                        && workspace.repo_id == repo_id
                        && workspace.status == WorkspaceStatus::Ready
                })
                .ok_or_else(|| {
                    validation_error(
                        "validation.subject_unavailable",
                        "the exact repository result has no Ready Workspace",
                    )
                })?;
            let commit_sha = read_head(&workspace.worktree_path).await?;
            if result.after_sha.as_deref() != Some(commit_sha.as_str()) {
                return Err(validation_error(
                    "validation.subject_stale",
                    "the repository Workspace no longer matches the exact completed result commit",
                ));
            }
            let snapshot_digest =
                ValidationService::snapshot_digest(&workspace.worktree_path).await?;
            if result.snapshot_digest.as_deref() != Some(snapshot_digest.as_str()) {
                return Err(validation_error(
                    "validation.subject_stale",
                    "the repository Workspace no longer matches the exact completed result snapshot",
                ));
            }
            return Ok(ResolvedSubject {
                workspace,
                commit_sha,
                snapshot_digest,
            });
        }
        ValidationCheckSubjectKind::TaskIntegration => {
            if subject_id != task.id {
                return Err(ServiceError::not_found("task", subject_id.to_owned()));
            }
            if task.version != subject_version {
                return Err(validation_error(
                    "validation.subject_stale",
                    "the Task version no longer matches the requested subject",
                ));
            }
            WorkspaceRepo::get_integration_by_task_repo(db, &task.id, repo_id).await?
        }
    }
    .filter(|workspace| {
        workspace.task_id == task.id
            && workspace.repo_id == repo_id
            && workspace.status == WorkspaceStatus::Ready
    })
    .ok_or_else(|| {
        validation_error(
            "validation.subject_unavailable",
            "the validation subject has no Ready Workspace",
        )
    })?;
    let commit_sha = read_head(&workspace.worktree_path).await?;
    let snapshot_digest = ValidationService::snapshot_digest(&workspace.worktree_path).await?;
    Ok(ResolvedSubject {
        workspace,
        commit_sha,
        snapshot_digest,
    })
}

/// Authorize against the current envelope pin, resolve the exact subject, and
/// insert (or replay) a queued request. Idempotent per
/// `(task, authorization_version, pin, trigger, exact subject)`.
///
/// Typed errors: `validation.check_not_authorized`, `validation.check_unknown`,
/// `validation.check_stale_pin`, `validation.subject_stale`,
/// `validation.subject_unavailable`, `validation.workspace_leased`.
pub async fn enqueue_check_request(
    db: &SqliteDb,
    event_bus: &EventBus,
    input: EnqueueCheckRequest,
) -> Result<ValidationCheckRequestWrite> {
    if input
        .reason
        .as_deref()
        .is_some_and(|reason| reason.len() > MAX_REASON_BYTES)
    {
        return Err(ServiceError::invalid_operation(
            "validation request reason must not exceed 1024 bytes",
        ));
    }
    if (input.trigger == ValidationCheckRequestTrigger::CoordinatorRequest)
        != input.requested_by_request_id.is_some()
    {
        return Err(ServiceError::invalid_operation(
            "a coordinator validation request must name its coordination request, and only it",
        ));
    }
    let task = TaskRepo::get_by_id(db, &input.task_id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", input.task_id.clone()))?;
    let not_authorized = || {
        validation_error(
            "validation.check_not_authorized",
            "the current coordination envelope does not pin this check for this trigger",
        )
    };
    let coordination = TaskCoordinationRepo::get_task_coordination(db, &task.id)
        .await?
        .filter(|coordination| {
            coordination.revoked_at.is_none()
                && coordination.mode != TaskCoordinationMode::Manual
                && !coordination.team_stale
        })
        .ok_or_else(not_authorized)?;
    let envelope: CoordinationEnvelope =
        serde_json::from_str(&coordination.envelope_json).map_err(|_| not_authorized())?;
    let envelope_trigger = match input.trigger {
        ValidationCheckRequestTrigger::CoordinatorRequest => db::EnvelopeCheckTrigger::OnRequest,
        ValidationCheckRequestTrigger::AfterIntegration => {
            db::EnvelopeCheckTrigger::AfterIntegration
        }
    };
    let pin = envelope
        .validation_checks
        .iter()
        .find(|pin| pin.check_key == input.check_key && pin.trigger == envelope_trigger)
        .ok_or_else(not_authorized)?;
    let definition = resolve_pin(
        db,
        &task.project_id,
        Some(&input.repo_id),
        &pin.check_key,
        &pin.definition_digest,
    )
    .await?;
    let subject = resolve_subject(
        db,
        &task,
        input.subject_kind,
        &input.subject_id,
        input.subject_version,
        &input.repo_id,
    )
    .await?;
    let idempotency_key = digest_json(&(
        "validation_check_request:v2",
        &task.id,
        coordination.authorization_version,
        &definition.id,
        &definition.definition_digest,
        input.trigger.to_string(),
        input.subject_kind.to_string(),
        &input.subject_id,
        input.subject_version,
        &input.repo_id,
        &subject.workspace.id,
        &subject.commit_sha,
        &subject.snapshot_digest,
        input.baseline,
    ))?;
    let id = new_uuid_v4();
    let now = now_rfc3339();
    let queued_event = event(
        "validation_check_request.queued",
        "validation_check_request",
        &id,
        &task.id,
        &task.project_id,
        &format!("validation-check-request-queued:{idempotency_key}"),
        serde_json::json!({
            "check_request_id": id,
            "task_id": task.id,
            "project_id": task.project_id,
            "check_key": definition.check_key,
            "definition_digest": definition.definition_digest,
            "trigger": input.trigger,
            "subject_kind": input.subject_kind,
            "subject_id": input.subject_id,
            "subject_version": input.subject_version,
            "repo_id": subject.workspace.repo_id,
            "workspace_id": subject.workspace.id,
            "commit_sha": subject.commit_sha,
            "baseline": input.baseline,
        }),
        &now,
    );
    let write = ValidationCheckRequestRepo::create_validation_check_request(
        db,
        CreateValidationCheckRequest {
            id,
            task_id: task.id.clone(),
            authorization_version: coordination.authorization_version,
            check_definition_id: definition.id,
            definition_digest: definition.definition_digest,
            trigger_kind: input.trigger,
            subject_kind: input.subject_kind,
            subject_id: input.subject_id,
            subject_version: input.subject_version,
            workspace_id: subject.workspace.id,
            commit_sha: subject.commit_sha,
            workspace_snapshot_digest: subject.snapshot_digest,
            baseline: input.baseline,
            requested_by_request_id: input.requested_by_request_id,
            requested_by_execution_id: input.requested_by_execution_id,
            reason: input.reason,
            idempotency_key,
            created_at: now,
        },
        Some(queued_event),
    )
    .await?;
    if let Some(event) = write.event.as_ref() {
        DomainEventService::publish_committed_hint(event_bus, event);
    }
    Ok(write)
}

pub struct ValidationCheckRunner {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    validation: ValidationService,
    workspace_locks: Arc<WorkspaceExecutionLockManager>,
    baseline_root: PathBuf,
    in_flight: Mutex<HashSet<String>>,
    tick_interval: Duration,
}

struct InFlightGuard<'a> {
    runner: &'a ValidationCheckRunner,
    id: String,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut in_flight) = self.runner.in_flight.lock() {
            in_flight.remove(&self.id);
        }
    }
}

impl ValidationCheckRunner {
    pub fn new(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        workspace_locks: Arc<WorkspaceExecutionLockManager>,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            validation: ValidationService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            db,
            event_bus,
            workspace_locks,
            baseline_root: workspace_root.join(BASELINE_DIR),
            in_flight: Mutex::new(HashSet::new()),
            tick_interval: TICK_INTERVAL,
        }
    }

    pub fn with_tick_interval(mut self, tick_interval: Duration) -> Self {
        self.tick_interval = tick_interval.max(Duration::from_millis(50));
        self
    }

    /// Public enqueue entry point for coordinator `request_validation` outputs
    /// and the post-integration hook. See [`enqueue_check_request`].
    pub async fn enqueue_check_request(
        &self,
        input: EnqueueCheckRequest,
    ) -> Result<ValidationCheckRequestWrite> {
        enqueue_check_request(&self.db, &self.event_bus, input).await
    }

    /// Start the polling loop. `running` rows left by a previous process are
    /// recovered by the first tick (nothing of this process is in flight yet).
    pub fn spawn(self: Arc<Self>, mut shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = interval(self.tick_interval);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if let Err(error) = self.tick().await {
                            warn!(%error, "validation check runner tick failed");
                        }
                    }
                    result = shutdown_rx.changed() => {
                        if result.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        })
    }

    /// One pass: dispatch every claimable queued request and every `running`
    /// request not owned by this process. Each request runs in its own task.
    pub async fn tick(self: &Arc<Self>) -> Result<usize> {
        let mut candidates = ValidationCheckRequestRepo::list_validation_check_requests_by_state(
            &*self.db,
            ValidationCheckRequestState::Running,
            BATCH_LIMIT,
        )
        .await?;
        candidates.extend(
            ValidationCheckRequestRepo::list_validation_check_requests_by_state(
                &*self.db,
                ValidationCheckRequestState::Queued,
                BATCH_LIMIT,
            )
            .await?,
        );
        let mut started = 0;
        for request in candidates {
            if !self.try_mark_in_flight(&request.id) {
                continue;
            }
            started += 1;
            let runner = Arc::clone(self);
            tokio::spawn(async move {
                let _guard = InFlightGuard {
                    runner: &runner,
                    id: request.id.clone(),
                };
                if let Err(error) = runner.process_request(&request.id).await {
                    warn!(request_id = %request.id, %error, "validation check request failed");
                }
            });
        }
        Ok(started)
    }

    fn try_mark_in_flight(&self, id: &str) -> bool {
        self.in_flight
            .lock()
            .map(|mut in_flight| in_flight.insert(id.to_owned()))
            .unwrap_or(false)
    }

    /// Process one request to a terminal state, or leave it `queued`/`running`
    /// when it must be retried (active worker lease, claim still held by a
    /// dead process). Idempotent: terminal requests are returned untouched and
    /// an existing terminal run or comparison is reused.
    pub async fn process_request(&self, request_id: &str) -> Result<ValidationCheckRequest> {
        let mut request =
            ValidationCheckRequestRepo::get_validation_check_request(&*self.db, request_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("validation check request", request_id.to_owned())
                })?;
        if request.state.is_terminal() {
            return Ok(request);
        }
        let was_running = request.state == ValidationCheckRequestState::Running;
        let task = TaskRepo::get_by_id(&*self.db, &request.task_id, true)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", request.task_id.clone()))?;
        let definition = ValidationCatalogRepo::get_validation_check_definition(
            &*self.db,
            &request.check_definition_id,
        )
        .await?
        .ok_or_else(|| {
            ServiceError::not_found(
                "validation check definition",
                request.check_definition_id.clone(),
            )
        })?;
        let workspace = WorkspaceRepo::get_by_id(&*self.db, &request.workspace_id)
            .await?
            .filter(|workspace| {
                workspace.task_id == request.task_id && workspace.status == WorkspaceStatus::Ready
            });
        let Some(workspace) = workspace else {
            return self
                .finish(
                    &request,
                    &task,
                    ValidationCheckRequestState::FailedToStart,
                    None,
                    Some("validation.workspace_not_ready"),
                )
                .await;
        };
        if self.has_active_worker_lease(&workspace.id).await? {
            // The worker owns the Workspace; retry on a later tick.
            return Ok(request);
        }

        let workspace_guard = self.workspace_locks.acquire(&workspace.id).await;
        if request.state == ValidationCheckRequestState::Queued {
            let baseline_commit_sha = if request.baseline {
                match workspace.before_sha.clone() {
                    Some(sha) => Some(sha),
                    None => {
                        drop(workspace_guard);
                        return self
                            .finish(
                                &request,
                                &task,
                                ValidationCheckRequestState::FailedToStart,
                                None,
                                Some("validation.baseline_unavailable"),
                            )
                            .await;
                    }
                }
            } else {
                None
            };
            request = self
                .transition(
                    &request,
                    &task,
                    ValidationCheckRequestState::Running,
                    None,
                    baseline_commit_sha,
                    None,
                    "validation_check_request.started",
                )
                .await?;
        } else if request.baseline && request.baseline_commit_sha.is_none() {
            drop(workspace_guard);
            return self
                .finish(
                    &request,
                    &task,
                    ValidationCheckRequestState::FailedToStart,
                    None,
                    Some("validation.baseline_unavailable"),
                )
                .await;
        }

        let run_origin = match request.trigger_kind {
            ValidationCheckRequestTrigger::CoordinatorRequest => {
                ValidationRunOrigin::CoordinatorRequest
            }
            ValidationCheckRequestTrigger::AfterIntegration => {
                ValidationRunOrigin::AfterIntegration
            }
        };
        let outcome = self
            .validation
            .run_check_core(CheckRunInput {
                task_id: &request.task_id,
                workspace_id: &request.workspace_id,
                command: &definition.command,
                caused_by_execution_id: None,
                check_identity: catalog_check_identity(&definition),
                evidence_key: format!("catalog:{}", definition.check_key),
                timeout_seconds: u64::try_from(definition.timeout_seconds).unwrap_or(3600),
                catalog: Some(CatalogRunIdentity {
                    catalog_check_id: definition.id.clone(),
                    check_key: definition.check_key.clone(),
                    definition_digest: definition.definition_digest.clone(),
                    run_origin,
                    check_request_id: request.id.clone(),
                }),
                expected_subject: Some((&request.commit_sha, &request.workspace_snapshot_digest)),
            })
            .await;
        drop(workspace_guard);
        let result = match outcome {
            Ok(result) => result,
            Err(ServiceError::Coordination {
                code: "validation.subject_stale",
                ..
            }) => {
                return self
                    .finish(
                        &request,
                        &task,
                        ValidationCheckRequestState::FailedToStart,
                        None,
                        Some("validation.subject_changed"),
                    )
                    .await;
            }
            // A recovered request may still be fenced by the dead process's
            // claim; keep it `running` and retry after the lease expires.
            Err(error) if was_running => return Err(error),
            Err(error) => {
                warn!(request_id = %request.id, %error, "validation check could not start");
                return self
                    .finish(
                        &request,
                        &task,
                        ValidationCheckRequestState::FailedToStart,
                        None,
                        Some("validation.start_failed"),
                    )
                    .await;
            }
        };
        let run = result.run;
        if run.status == ValidationRunStatus::Running {
            return Ok(request);
        }
        if run.check_request_id.as_deref() != Some(request.id.as_str()) {
            // The exact subject and definition already have a terminal run from
            // another request: that run is the evidence, nothing new to record.
            return self
                .finish(
                    &request,
                    &task,
                    ValidationCheckRequestState::Cancelled,
                    None,
                    None,
                )
                .await;
        }
        if request.baseline {
            let candidate_digest = candidate_output_digest(&result.evidence);
            if let Err(error) = self
                .record_baseline(
                    &request,
                    &task,
                    &definition,
                    &workspace,
                    &run,
                    candidate_digest,
                )
                .await
            {
                warn!(request_id = %request.id, %error, "validation baseline failed");
                return self
                    .finish(
                        &request,
                        &task,
                        ValidationCheckRequestState::FailedToStart,
                        Some(run.id.clone()),
                        Some("validation.baseline_failed"),
                    )
                    .await;
            }
        }
        self.finish(
            &request,
            &task,
            ValidationCheckRequestState::Completed,
            Some(run.id.clone()),
            None,
        )
        .await
    }

    async fn has_active_worker_lease(&self, workspace_id: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workspace_lease
             WHERE workspace_id = ? AND status = 'active' AND role = 'worker'",
        )
        .bind(workspace_id)
        .fetch_one(self.db.pool())
        .await?;
        Ok(count > 0)
    }

    async fn record_baseline(
        &self,
        request: &ValidationCheckRequest,
        task: &db::Task,
        definition: &ValidationCheckDefinition,
        workspace: &Workspace,
        run: &ValidationRun,
        candidate_output_digest: Option<String>,
    ) -> Result<()> {
        if ValidationComparisonRepo::get_validation_comparison_by_request(&*self.db, &request.id)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let base_commit = request
            .baseline_commit_sha
            .clone()
            .ok_or_else(|| ServiceError::invalid_operation("baseline commit is not resolved"))?;
        let directory = self.baseline_root.join(&request.id);
        let directory_text = directory.to_string_lossy().into_owned();
        remove_baseline_worktree(&workspace.worktree_path, &directory).await;
        if let Some(parent) = directory.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|error| {
                ServiceError::invalid_operation(format!("baseline directory failed: {error}"))
            })?;
        }
        add_baseline_worktree(&workspace.worktree_path, &directory_text, &base_commit).await?;
        let execution = run_baseline_command(
            &directory_text,
            &definition.command,
            u64::try_from(definition.timeout_seconds).unwrap_or(3600),
        )
        .await;
        let cleaned = remove_baseline_worktree(&workspace.worktree_path, &directory).await;
        let execution = execution?;
        let now = now_rfc3339();
        let provenance = serde_json::json!({
            "kind": "runner_detached_worktree",
            "runner": "validation_check_runner",
            "check_request_id": request.id,
            "base_commit": base_commit,
            "worktree_cleaned": cleaned,
        })
        .to_string();
        let comparison_id = new_uuid_v4();
        let write = ValidationComparisonRepo::create_validation_comparison(
            &*self.db,
            CreateValidationComparison {
                id: comparison_id.clone(),
                task_id: request.task_id.clone(),
                check_request_id: request.id.clone(),
                candidate_run_id: run.id.clone(),
                candidate_output_digest,
                baseline_commit_sha: base_commit,
                baseline_snapshot_digest: execution.snapshot_digest,
                baseline_provenance_json: provenance,
                baseline_status: execution.status,
                baseline_exit_code: execution.exit_code,
                baseline_output_digest: Some(execution.output_digest),
                created_at: now.clone(),
            },
            Some(event(
                "validation_comparison.recorded",
                "validation_comparison",
                &comparison_id,
                &task.id,
                &task.project_id,
                &format!("validation-comparison:{}", request.id),
                serde_json::json!({
                    "check_request_id": request.id,
                    "task_id": task.id,
                    "project_id": task.project_id,
                    "candidate_run_id": run.id,
                }),
                &now,
            )),
        )
        .await?;
        if let Some(event) = write.event.as_ref() {
            DomainEventService::publish_committed_hint(&self.event_bus, event);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition(
        &self,
        request: &ValidationCheckRequest,
        task: &db::Task,
        to_state: ValidationCheckRequestState,
        candidate_run_id: Option<String>,
        baseline_commit_sha: Option<String>,
        failure_code: Option<&str>,
        event_type: &str,
    ) -> Result<ValidationCheckRequest> {
        let now = now_rfc3339();
        let completed_at = to_state.is_terminal().then(|| now.clone());
        let write = ValidationCheckRequestRepo::transition_validation_check_request(
            &*self.db,
            TransitionValidationCheckRequest {
                id: request.id.clone(),
                expected_version: request.version,
                to_state,
                candidate_run_id: candidate_run_id.clone(),
                baseline_commit_sha,
                failure_code: failure_code.map(str::to_owned),
                updated_at: now.clone(),
                completed_at,
            },
            Some(event(
                event_type,
                "validation_check_request",
                &request.id,
                &task.id,
                &task.project_id,
                &format!("{event_type}:{}:{}", request.id, request.version + 1),
                serde_json::json!({
                    "check_request_id": request.id,
                    "task_id": task.id,
                    "project_id": task.project_id,
                    "state": to_state,
                    "candidate_run_id": candidate_run_id,
                    "failure_code": failure_code,
                }),
                &now,
            )),
        )
        .await?;
        if let Some(event) = write.event.as_ref() {
            DomainEventService::publish_committed_hint(&self.event_bus, event);
        }
        Ok(write.request)
    }

    async fn finish(
        &self,
        request: &ValidationCheckRequest,
        task: &db::Task,
        to_state: ValidationCheckRequestState,
        candidate_run_id: Option<String>,
        failure_code: Option<&str>,
    ) -> Result<ValidationCheckRequest> {
        let event_type = match to_state {
            ValidationCheckRequestState::Completed => "validation_check_request.completed",
            ValidationCheckRequestState::Cancelled => "validation_check_request.cancelled",
            _ => "validation_check_request.failed_to_start",
        };
        self.transition(
            request,
            task,
            to_state,
            candidate_run_id,
            None,
            failure_code,
            event_type,
        )
        .await
    }
}

fn catalog_check_identity(definition: &ValidationCheckDefinition) -> String {
    format!(
        "catalog:{}@{}",
        definition.check_key,
        &definition.definition_digest[..16]
    )
}

fn candidate_output_digest(evidence: &[db::Evidence]) -> Option<String> {
    let content: serde_json::Value = serde_json::from_str(&evidence.first()?.content_json).ok()?;
    Some(output_digest(
        content.get("stdout_tail")?.as_str()?,
        content.get("stderr_tail")?.as_str()?,
    ))
}

async fn git_in(repo_worktree: &str, args: &[&str]) -> Result<bool> {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo_worktree)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("git failed to start: {error}"))
        })?;
    Ok(status.success())
}

async fn add_baseline_worktree(
    repo_worktree: &str,
    directory: &str,
    base_commit: &str,
) -> Result<()> {
    if git_in(
        repo_worktree,
        &[
            "worktree",
            "add",
            "--detach",
            "--force",
            directory,
            base_commit,
        ],
    )
    .await?
    {
        Ok(())
    } else {
        Err(ServiceError::invalid_operation(
            "could not create the baseline worktree at the exact base commit",
        ))
    }
}

/// Remove the runner-owned baseline worktree and its git metadata. Returns
/// whether nothing is left behind; failures never fail the request.
async fn remove_baseline_worktree(repo_worktree: &str, directory: &std::path::Path) -> bool {
    let directory_text = directory.to_string_lossy().into_owned();
    let _ = git_in(
        repo_worktree,
        &["worktree", "remove", "--force", &directory_text],
    )
    .await;
    if directory.exists() {
        let _ = tokio::fs::remove_dir_all(directory).await;
    }
    let _ = git_in(repo_worktree, &["worktree", "prune"]).await;
    !directory.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation_catalog::{
        test_support::{seed_project, seed_task},
        PublishCheckInput, ValidationCatalogService,
    };
    use db::{
        CreateTaskRole, CreateWorkspace, TaskRoleRepo, ValidationRunRepo, WorkUnitWorkspaceRepo,
    };
    use tempfile::TempDir;

    struct Fixture {
        seed: crate::validation_catalog::test_support::Seed,
        task: db::Task,
        workspace: Workspace,
        worktree: PathBuf,
        base_sha: String,
        _temp: TempDir,
        runner: Arc<ValidationCheckRunner>,
        definition: ValidationCheckDefinition,
    }

    /// Task with a real git workspace whose `before_sha` is the first commit and
    /// whose HEAD is a second commit adding `bad.txt`; an authorized coordination
    /// pins `after_integration` for the published check `command`.
    async fn make_fixture(command: &str, trigger: &str) -> Fixture {
        let temp = TempDir::new().expect("temp dir");
        let seed = seed_project(&temp.path().to_string_lossy()).await;
        let task = seed_task(&seed.db, &seed.project_id, &seed.repo_id).await;
        let worktree = temp.path().join("worktree");
        std::fs::create_dir_all(&worktree).expect("worktree dir");
        git::init(&worktree).await.expect("git init");
        std::fs::write(worktree.join("README.md"), "base\n").expect("base file");
        let base_sha = git::commit_all(&worktree, "base")
            .await
            .expect("base commit");
        std::fs::write(worktree.join("bad.txt"), "bad\n").expect("candidate file");
        git::commit_all(&worktree, "candidate")
            .await
            .expect("candidate commit");
        let workspace = WorkspaceRepo::create(
            &*seed.db,
            CreateWorkspace {
                id: new_uuid_v4(),
                task_id: task.id.clone(),
                repo_id: seed.repo_id.clone(),
                worktree_path: worktree.to_string_lossy().into_owned(),
                branch: "forge/test".to_owned(),
                status: WorkspaceStatus::Ready,
                before_sha: Some(base_sha.clone()),
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("workspace");
        let definition = ValidationCatalogService::new(Arc::clone(&seed.db))
            .create_version(
                &seed.project_id,
                &seed.owner_id,
                PublishCheckInput {
                    repo_id: None,
                    check_key: "no-bad".to_owned(),
                    command: command.to_owned(),
                    timeout_seconds: 60,
                    env_policy: None,
                },
            )
            .await
            .expect("publish");
        let now = now_rfc3339();
        let role_id = new_uuid_v4();
        TaskRoleRepo::create(
            &*seed.db,
            CreateTaskRole {
                id: role_id.clone(),
                task_id: task.id.clone(),
                role: "coordinator".to_owned(),
                coordination_mode: None,
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("role");
        let envelope = serde_json::json!({
            "schema_version": 2,
            "validation_checks": [{
                "check_key": "no-bad",
                "definition_digest": definition.definition_digest,
                "trigger": trigger,
            }],
        });
        sqlx::query(
            "UPDATE task_coordination SET mode = 'prepare_and_execute',
                coordinator_task_role_id = ?, coordinator_actor_kind = 'agent',
                coordinator_actor_id = 'agent-runner', authorization_version = 1,
                envelope_json = ?, envelope_digest = ?, team_digest = ?, requested_by = ?
             WHERE task_id = ?",
        )
        .bind(&role_id)
        .bind(envelope.to_string())
        .bind(hex::encode(sha2::Sha256::digest(b"envelope")))
        .bind(hex::encode(sha2::Sha256::digest(b"team")))
        .bind(&seed.owner_id)
        .bind(&task.id)
        .execute(seed.db.pool())
        .await
        .expect("authorize coordination");
        sqlx::query("UPDATE task_coordination SET team_stale = 0 WHERE task_id = ?")
            .bind(&task.id)
            .execute(seed.db.pool())
            .await
            .expect("settle team");
        let runner = Arc::new(ValidationCheckRunner::new(
            Arc::clone(&seed.db),
            Arc::new(EventBus::new(64)),
            Arc::new(WorkspaceExecutionLockManager::new()),
            temp.path().join("workspaces"),
        ));
        let task = TaskRepo::get_by_id(&*seed.db, &task.id, false)
            .await
            .expect("task")
            .expect("task exists");
        Fixture {
            seed,
            task,
            workspace,
            worktree,
            base_sha,
            _temp: temp,
            runner,
            definition,
        }
    }

    fn after_integration(fixture: &Fixture, baseline: bool) -> EnqueueCheckRequest {
        EnqueueCheckRequest {
            task_id: fixture.task.id.clone(),
            repo_id: fixture
                .task
                .repo_id
                .clone()
                .expect("after-integration fixture has a primary repository"),
            check_key: "no-bad".to_owned(),
            trigger: ValidationCheckRequestTrigger::AfterIntegration,
            subject_kind: ValidationCheckSubjectKind::TaskIntegration,
            subject_id: fixture.task.id.clone(),
            subject_version: fixture.task.version,
            baseline,
            requested_by_request_id: None,
            requested_by_execution_id: None,
            reason: None,
        }
    }

    use sha2::Digest as _;

    #[test]
    fn multirepo_result_validation_selects_requested_repo_workspace() {
        let results = [
            db::ExecutionRepositoryResult {
                execution_id: "execution".to_owned(),
                task_id: "task".to_owned(),
                repo_id: "repo-a".to_owned(),
                workspace_id: "workspace-a".to_owned(),
                before_sha: Some("base-a".to_owned()),
                after_sha: Some("commit-a".to_owned()),
                snapshot_digest: Some("digest-a".to_owned()),
                created_at: "now".to_owned(),
            },
            db::ExecutionRepositoryResult {
                execution_id: "execution".to_owned(),
                task_id: "task".to_owned(),
                repo_id: "repo-b".to_owned(),
                workspace_id: "workspace-b".to_owned(),
                before_sha: Some("base-b".to_owned()),
                after_sha: Some("commit-b".to_owned()),
                snapshot_digest: Some("digest-b".to_owned()),
                created_at: "now".to_owned(),
            },
        ];
        let selected = result_for_repository(&results, "repo-b").expect("repo-b is exact");
        assert_eq!(selected.workspace_id, "workspace-b");
        assert_eq!(selected.after_sha.as_deref(), Some("commit-b"));
        assert_eq!(selected.snapshot_digest.as_deref(), Some("digest-b"));
        assert!(result_for_repository(&results, "repo-c").is_none());
    }

    #[tokio::test]
    async fn validation_check_runner_enqueue_requires_the_exact_envelope_pin() {
        let fixture = make_fixture("true", "after_integration").await;
        // Wrong trigger: the pin authorizes only after_integration.
        let mut request = after_integration(&fixture, false);
        request.trigger = ValidationCheckRequestTrigger::CoordinatorRequest;
        request.requested_by_request_id = Some("request".to_owned());
        let error = fixture
            .runner
            .enqueue_check_request(request)
            .await
            .expect_err("on_request is not pinned");
        assert!(matches!(
            error,
            ServiceError::Coordination {
                code: "validation.check_not_authorized",
                ..
            }
        ));
        // Unknown key is not authorized either; the coordinator never names commands.
        let mut unknown = after_integration(&fixture, false);
        unknown.check_key = "other".to_owned();
        assert!(matches!(
            fixture.runner.enqueue_check_request(unknown).await,
            Err(ServiceError::Coordination {
                code: "validation.check_not_authorized",
                ..
            })
        ));
        // A stale subject version is typed.
        let mut stale = after_integration(&fixture, false);
        stale.subject_version += 5;
        assert!(matches!(
            fixture.runner.enqueue_check_request(stale).await,
            Err(ServiceError::Coordination {
                code: "validation.subject_stale",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn validation_check_runner_enqueue_is_idempotent_for_the_exact_subject() {
        let fixture = make_fixture("true", "after_integration").await;
        let first = fixture
            .runner
            .enqueue_check_request(after_integration(&fixture, false))
            .await
            .expect("enqueue");
        assert!(first.created);
        assert_eq!(first.request.state, ValidationCheckRequestState::Queued);
        assert_eq!(first.request.commit_sha.len(), 40);
        let replay = fixture
            .runner
            .enqueue_check_request(after_integration(&fixture, false))
            .await
            .expect("replay");
        assert!(!replay.created);
        assert_eq!(replay.request.id, first.request.id);
    }

    #[tokio::test]
    async fn validation_check_runner_runs_the_pinned_command_and_records_catalog_identity() {
        let fixture = make_fixture("test ! -e bad.txt || exit 3", "after_integration").await;
        let queued = fixture
            .runner
            .enqueue_check_request(after_integration(&fixture, false))
            .await
            .expect("enqueue")
            .request;
        let done = fixture
            .runner
            .process_request(&queued.id)
            .await
            .expect("process");
        assert_eq!(done.state, ValidationCheckRequestState::Completed);
        let run = ValidationRunRepo::get_validation_run(
            &*fixture.seed.db,
            done.candidate_run_id.as_deref().expect("candidate run"),
        )
        .await
        .expect("run lookup")
        .expect("run exists");
        assert_eq!(run.status, ValidationRunStatus::Failed);
        assert_eq!(run.exit_code, Some(3));
        assert_eq!(run.run_origin, ValidationRunOrigin::AfterIntegration);
        assert_eq!(run.check_request_id.as_deref(), Some(queued.id.as_str()));
        assert_eq!(
            run.definition_digest.as_deref(),
            Some(fixture.definition.definition_digest.as_str())
        );
        assert_eq!(run.commit_sha, queued.commit_sha);
        assert_eq!(
            run.check_identity,
            format!(
                "catalog:no-bad@{}",
                &fixture.definition.definition_digest[..16]
            )
        );
        assert!(run.config_summary_json.contains("definition_digest"));
        // Replaying a terminal request changes nothing.
        let again = fixture
            .runner
            .process_request(&queued.id)
            .await
            .expect("terminal replay");
        assert_eq!(again.version, done.version);
        assert_eq!(
            ValidationRunRepo::list_validation_runs_by_task(&*fixture.seed.db, &fixture.task.id)
                .await
                .expect("runs")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn validation_check_runner_baseline_classifies_a_regression_and_cleans_up() {
        let fixture = make_fixture("test ! -e bad.txt", "after_integration").await;
        let queued = fixture
            .runner
            .enqueue_check_request(after_integration(&fixture, true))
            .await
            .expect("enqueue")
            .request;
        let done = fixture
            .runner
            .process_request(&queued.id)
            .await
            .expect("process");
        assert_eq!(done.state, ValidationCheckRequestState::Completed);
        assert_eq!(
            done.baseline_commit_sha.as_deref(),
            Some(fixture.base_sha.as_str())
        );
        let comparison = ValidationComparisonRepo::get_validation_comparison_by_request(
            &*fixture.seed.db,
            &queued.id,
        )
        .await
        .expect("comparison lookup")
        .expect("comparison exists");
        assert_eq!(comparison.candidate_status, ValidationRunStatus::Failed);
        assert_eq!(comparison.baseline_status, ValidationRunStatus::Passed);
        assert_eq!(
            comparison.classification,
            db::ValidationComparisonClass::Regression
        );
        assert_eq!(comparison.baseline_commit_sha, fixture.base_sha);
        assert!(comparison
            .baseline_provenance_json
            .contains("runner_detached_worktree"));
        // The disposable worktree and its git metadata are gone; no workspace row was added.
        assert!(!fixture.runner.baseline_root.join(&queued.id).exists());
        let listed = WorkUnitWorkspaceRepo::list_by_task(&*fixture.seed.db, &fixture.task.id)
            .await
            .expect("workspaces");
        assert!(listed
            .iter()
            .all(|workspace| workspace.id == fixture.workspace.id));
        assert_eq!(
            fixture.worktree,
            PathBuf::from(&fixture.workspace.worktree_path)
        );
    }

    #[tokio::test]
    async fn validation_check_runner_pre_existing_failure_and_changed_subject() {
        let fixture = make_fixture("exit 9", "after_integration").await;
        let queued = fixture
            .runner
            .enqueue_check_request(after_integration(&fixture, true))
            .await
            .expect("enqueue")
            .request;
        let done = fixture
            .runner
            .process_request(&queued.id)
            .await
            .expect("process");
        let comparison = ValidationComparisonRepo::get_validation_comparison_by_request(
            &*fixture.seed.db,
            &done.id,
        )
        .await
        .expect("lookup")
        .expect("comparison");
        assert_eq!(
            comparison.classification,
            db::ValidationComparisonClass::PreExistingFailure
        );
        assert_eq!(comparison.baseline_exit_code, Some(9));

        // A queued request whose workspace changed after enqueue never runs against
        // a different subject: it fails to start with a typed code.
        let fresh = make_fixture("true", "after_integration").await;
        let queued = fresh
            .runner
            .enqueue_check_request(after_integration(&fresh, false))
            .await
            .expect("enqueue")
            .request;
        std::fs::write(fresh.worktree.join("drift.txt"), "drift\n").expect("drift");
        let failed = fresh
            .runner
            .process_request(&queued.id)
            .await
            .expect("process");
        assert_eq!(failed.state, ValidationCheckRequestState::FailedToStart);
        assert_eq!(
            failed.failure_code.as_deref(),
            Some("validation.subject_changed")
        );
    }

    #[tokio::test]
    async fn validation_check_runner_recovers_a_running_request_after_restart() {
        let fixture = make_fixture("true", "after_integration").await;
        let queued = fixture
            .runner
            .enqueue_check_request(after_integration(&fixture, false))
            .await
            .expect("enqueue")
            .request;
        // Simulate a crash after queued -> running and before any run was recorded.
        ValidationCheckRequestRepo::transition_validation_check_request(
            &*fixture.seed.db,
            TransitionValidationCheckRequest {
                id: queued.id.clone(),
                expected_version: queued.version,
                to_state: ValidationCheckRequestState::Running,
                candidate_run_id: None,
                baseline_commit_sha: None,
                failure_code: None,
                updated_at: now_rfc3339(),
                completed_at: None,
            },
            None,
        )
        .await
        .expect("running");
        let restarted = Arc::new(ValidationCheckRunner::new(
            Arc::clone(&fixture.seed.db),
            Arc::new(EventBus::new(16)),
            Arc::new(WorkspaceExecutionLockManager::new()),
            fixture._temp.path().join("workspaces"),
        ));
        assert_eq!(restarted.tick().await.expect("tick"), 1);
        for _ in 0..200 {
            let current = ValidationCheckRequestRepo::get_validation_check_request(
                &*fixture.seed.db,
                &queued.id,
            )
            .await
            .expect("lookup")
            .expect("exists");
            if current.state == ValidationCheckRequestState::Completed {
                assert!(current.candidate_run_id.is_some());
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("recovered request did not complete");
    }

    #[tokio::test]
    async fn validation_check_runner_post_integration_enqueues_only_after_integration_pins() {
        let fixture = make_fixture("true", "after_integration").await;
        let enqueued = crate::validation_post_integration::enqueue_after_integration(
            &fixture.seed.db,
            &EventBus::new(8),
            &fixture.task.id,
            &fixture.seed.repo_id,
        )
        .await
        .expect("hook");
        assert_eq!(enqueued, 1);
        // Replaying the hook for the same integration subject is idempotent.
        assert_eq!(
            crate::validation_post_integration::enqueue_after_integration(
                &fixture.seed.db,
                &EventBus::new(8),
                &fixture.task.id,
                &fixture.seed.repo_id,
            )
            .await
            .expect("replay"),
            1
        );
        let listed = ValidationCheckRequestRepo::list_validation_check_requests_by_state(
            &*fixture.seed.db,
            ValidationCheckRequestState::Queued,
            10,
        )
        .await
        .expect("queued");
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].trigger_kind,
            ValidationCheckRequestTrigger::AfterIntegration
        );
        assert!(!listed[0].baseline);

        let on_request_only = make_fixture("true", "on_request").await;
        assert_eq!(
            crate::validation_post_integration::enqueue_after_integration(
                &on_request_only.seed.db,
                &EventBus::new(8),
                &on_request_only.task.id,
                &on_request_only.seed.repo_id,
            )
            .await
            .expect("hook"),
            0
        );
    }

    async fn merge_readiness_gate(
        fixture: &Fixture,
        required_checks: Vec<crate::gate_engine::RequiredCheck>,
    ) -> (crate::gate_engine::GateEngine, db::Gate) {
        let engine = crate::gate_engine::GateEngine::new(
            Arc::clone(&fixture.seed.db),
            Arc::new(EventBus::new(16)),
        );
        let (gate, _) = engine
            .create_gate_with_initial_policy_v2(
                &fixture.task.id,
                "merge_readiness",
                db::GateScopeKind::Task,
                &fixture.task.id,
                crate::gate_engine::GatePolicyDocumentV2 {
                    schema_version: 2,
                    scope_requirement: None,
                    review: None,
                    validations: Vec::new(),
                    decisions: Vec::new(),
                    work_units: Vec::new(),
                    required_checks,
                },
            )
            .await
            .expect("v2 Gate policy is accepted");
        (engine, gate)
    }

    #[tokio::test]
    async fn gate_required_checks_resolve_by_exact_subject_and_outcome() {
        let fixture = make_fixture("test ! -e bad.txt || exit 3", "after_integration").await;
        let required = vec![crate::gate_engine::RequiredCheck {
            check_key: "no-bad".to_owned(),
            definition_digest: fixture.definition.definition_digest.clone(),
            required_outcome: ValidationRunStatus::Passed,
        }];
        let (engine, gate) = merge_readiness_gate(&fixture, required).await;

        // No run for the exact merge candidate yet: missing, never "latest".
        let before = engine.evaluate_active(&gate.id).await.expect("evaluation");
        assert_eq!(
            before.evaluation.outcome,
            db::GateEvaluationOutcome::Unsatisfied
        );
        assert!(before
            .evaluation
            .result_json
            .contains("required_check:no-bad"));

        let queued = fixture
            .runner
            .enqueue_check_request(after_integration(&fixture, false))
            .await
            .expect("enqueue")
            .request;
        fixture
            .runner
            .process_request(&queued.id)
            .await
            .expect("run");

        // The run exists but failed: the required outcome (passed) is not met.
        let failed = engine.evaluate_active(&gate.id).await.expect("evaluation");
        assert_eq!(
            failed.evaluation.outcome,
            db::GateEvaluationOutcome::Unsatisfied
        );
        assert!(failed
            .evaluation
            .result_json
            .contains("required_check_outcome_mismatch:no-bad:failed"));
    }

    #[tokio::test]
    async fn gate_required_checks_satisfied_by_the_exact_passing_run() {
        let fixture = make_fixture("true", "after_integration").await;
        let required = vec![crate::gate_engine::RequiredCheck {
            check_key: "no-bad".to_owned(),
            definition_digest: fixture.definition.definition_digest.clone(),
            required_outcome: ValidationRunStatus::Passed,
        }];
        let (engine, gate) = merge_readiness_gate(&fixture, required).await;
        let queued = fixture
            .runner
            .enqueue_check_request(after_integration(&fixture, false))
            .await
            .expect("enqueue")
            .request;
        let done = fixture
            .runner
            .process_request(&queued.id)
            .await
            .expect("run");
        let satisfied = engine.evaluate_active(&gate.id).await.expect("evaluation");
        assert_eq!(
            satisfied.evaluation.outcome,
            db::GateEvaluationOutcome::Satisfied
        );
        assert!(satisfied
            .inputs
            .iter()
            .any(|input| input.input_kind == "validation_run"
                && Some(input.input_id.as_str()) == done.candidate_run_id.as_deref()));
        assert!(satisfied
            .inputs
            .iter()
            .any(|input| input.input_kind == "evidence"));
    }

    #[tokio::test]
    async fn gate_required_checks_policy_v2_is_validated_and_v1_is_unchanged() {
        let fixture = make_fixture("true", "after_integration").await;
        let digest = fixture.definition.definition_digest.clone();
        let engine = crate::gate_engine::GateEngine::new(
            Arc::clone(&fixture.seed.db),
            Arc::new(EventBus::new(16)),
        );
        for bad in [
            vec![crate::gate_engine::RequiredCheck {
                check_key: "no-bad".to_owned(),
                definition_digest: digest.clone(),
                required_outcome: ValidationRunStatus::Failed,
            }],
            vec![crate::gate_engine::RequiredCheck {
                check_key: "Bad Key".to_owned(),
                definition_digest: digest.clone(),
                required_outcome: ValidationRunStatus::Passed,
            }],
            vec![crate::gate_engine::RequiredCheck {
                check_key: "no-bad".to_owned(),
                definition_digest: "short".to_owned(),
                required_outcome: ValidationRunStatus::Passed,
            }],
        ] {
            assert!(engine
                .create_gate_with_initial_policy_v2(
                    &fixture.task.id,
                    "merge_readiness",
                    db::GateScopeKind::Task,
                    &fixture.task.id,
                    crate::gate_engine::GatePolicyDocumentV2 {
                        schema_version: 2,
                        scope_requirement: None,
                        review: None,
                        validations: Vec::new(),
                        decisions: Vec::new(),
                        work_units: Vec::new(),
                        required_checks: bad,
                    },
                )
                .await
                .is_err());
        }
        // A v1 document cannot smuggle `required_checks` (deny_unknown_fields) and a
        // v2 document must declare schema 2.
        let v1_with_checks = serde_json::json!({"schema_version": 1, "required_checks": []});
        assert!(
            serde_json::from_value::<crate::gate_engine::GatePolicyDocument>(v1_with_checks)
                .is_err()
        );
        assert!(engine
            .create_gate_with_initial_policy_v2(
                &fixture.task.id,
                "merge_readiness",
                db::GateScopeKind::Task,
                &fixture.task.id,
                crate::gate_engine::GatePolicyDocumentV2 {
                    schema_version: 1,
                    scope_requirement: None,
                    review: None,
                    validations: Vec::new(),
                    decisions: Vec::new(),
                    work_units: Vec::new(),
                    required_checks: Vec::new(),
                },
            )
            .await
            .is_err());
    }
}
