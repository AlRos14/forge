//! Aggregate Task progress, persisted independently from actor activity.

use std::sync::Arc;

use api_types::{Actor, SystemComponent};
use db::{
    new_uuid_v4, now_rfc3339, CreateDomainEvent, DomainEventRepo, ExecutionRepo,
    GateEvaluationOutcome, GateRepo, SqliteDb, Task, TaskIntegrationOperationRepo, TaskLifecycle,
    TaskLifecycleRepo, TaskLifecycleState, TaskLifecycleTransitionWrite, TaskRepo,
    TransitionTaskLifecycle, ValidationRunRepo, WorkUnitRepo,
};
use events::EventBus;
use serde_json::json;

use crate::{DomainEventService, Result, ServiceError};

#[derive(Debug, Clone)]
pub enum LifecycleCause {
    Actor(Actor),
    GateEvaluation(String),
    Execution(String),
    ValidationRun(String),
    WorkUnit(String),
    MergeOperation(String),
    DomainEvent(String),
    System(SystemComponent),
}

#[derive(Debug, Clone)]
pub struct TransitionLifecycleInput {
    pub task_id: String,
    pub expected_task_version: i64,
    pub to_state: TaskLifecycleState,
    pub cause: LifecycleCause,
    pub reason_kind: Option<String>,
    pub reason_ref: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct TaskLifecycleTransitionResult {
    pub task: Task,
    pub lifecycle: TaskLifecycle,
    pub transition: Option<TaskLifecycleTransitionWrite>,
}

#[derive(Clone)]
pub struct TaskLifecycleService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
}

impl TaskLifecycleService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self { db, event_bus }
    }

    pub async fn get(&self, task_id: &str) -> Result<Option<TaskLifecycle>> {
        Ok(TaskLifecycleRepo::get_task_lifecycle(&*self.db, task_id).await?)
    }

    pub async fn block(
        &self,
        task_id: &str,
        cause: LifecycleCause,
        reason_kind: impl Into<String>,
        reason_ref: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Result<TaskLifecycleTransitionResult> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.to_owned()))?;
        if matches!(
            lifecycle.state,
            TaskLifecycleState::Blocked | TaskLifecycleState::Done | TaskLifecycleState::Cancelled
        ) {
            return Ok(TaskLifecycleTransitionResult {
                task,
                lifecycle,
                transition: None,
            });
        }
        self.transition(TransitionLifecycleInput {
            task_id: task.id,
            expected_task_version: task.version,
            to_state: TaskLifecycleState::Blocked,
            cause,
            reason_kind: Some(reason_kind.into()),
            reason_ref: Some(reason_ref.into()),
            idempotency_key: idempotency_key.into(),
        })
        .await
    }

    pub async fn transition(
        &self,
        input: TransitionLifecycleInput,
    ) -> Result<TaskLifecycleTransitionResult> {
        self.validate_cause(&input.task_id, &input.cause).await?;
        let cause = lifecycle_cause(input.cause);
        // Resolve the durable receipt first. This lets a retry after a crash
        // return the original GateEvaluation reference even if the aggregate
        // has advanced since the transition committed.
        let replay = TaskLifecycleRepo::get_task_lifecycle_transition(
            &*self.db,
            db::TaskLifecycleTransitionIdentity {
                task_id: input.task_id.clone(),
                idempotency_key: input.idempotency_key.clone(),
                expected_task_version: input.expected_task_version,
                to_state: input.to_state,
                cause_kind: cause.kind.clone(),
                cause_ref: cause.cause_ref.clone(),
                reason_kind: input.reason_kind.clone(),
                reason_ref: input.reason_ref.clone(),
                gate_evaluation_id: cause.gate_evaluation_id.clone(),
            },
        )
        .await?;
        let task = TaskRepo::get_by_id(&*self.db, &input.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", input.task_id.clone()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &input.task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", input.task_id.clone()))?;
        if let Some(transition) = replay {
            return Ok(TaskLifecycleTransitionResult {
                task,
                lifecycle,
                transition: Some(transition),
            });
        }
        if matches!(
            input.to_state,
            TaskLifecycleState::Ready | TaskLifecycleState::Active
        ) && crate::task_failure_retry::TaskFailureRetryService::has_exhausted_retry_budget(
            &self.db,
            &input.task_id,
        )
        .await?
        {
            return Err(ServiceError::invalid_operation(
                "Task retry budget is exhausted; runnable lifecycle states are fenced",
            ));
        }
        if task.version != input.expected_task_version {
            return Err(ServiceError::Db(db::DbError::TaskVersionConflict {
                expected: input.expected_task_version,
                actual: task.version,
            }));
        }
        if !allowed_transition(lifecycle.state, input.to_state) {
            return Err(ServiceError::Db(db::DbError::InvalidTransition));
        }
        if lifecycle.state == TaskLifecycleState::ReadyToMerge
            && input.to_state == TaskLifecycleState::Active
            && cause.kind != "gate_evaluation"
        {
            let exact_retry_rework = if cause.kind == "domain_event" {
                match cause.cause_ref.as_deref() {
                    Some(event_id) => {
                        if let Some(event) = DomainEventRepo::get_event(&*self.db, event_id).await?
                        {
                            let failure_ref =
                                serde_json::from_str::<serde_json::Value>(&event.payload_json)
                                    .ok()
                                    .and_then(|payload| {
                                        payload
                                            .get("failure_ref")
                                            .and_then(serde_json::Value::as_str)
                                            .map(str::to_owned)
                                    });
                            failure_ref.as_deref() == input.reason_ref.as_deref()
                                && crate::task_failure_retry::TaskFailureRetryService::is_rework_request_event(
                                    &self.db, &event,
                                )
                                .await?
                        } else {
                            false
                        }
                    }
                    None => false,
                }
            } else {
                false
            };
            if !exact_retry_rework {
                return Err(ServiceError::invalid_operation(
                    "ready_to_merge can return to active only from a new exact GateEvaluation or exact retry receipt",
                ));
            }
        }
        if input.to_state == TaskLifecycleState::Merging {
            return Err(ServiceError::invalid_operation(
                "merging is admitted only by an atomic TaskMerge integration operation",
            ));
        }
        if let Some(evaluation_id) = cause.gate_evaluation_id.as_deref() {
            self.validate_gate_lifecycle_cause(
                &task.id,
                lifecycle.state,
                input.to_state,
                lifecycle.reason_ref.as_deref(),
                input.reason_ref.as_deref(),
                evaluation_id,
            )
            .await?;
        }
        if lifecycle.state == input.to_state {
            return Ok(TaskLifecycleTransitionResult {
                task,
                lifecycle,
                transition: None,
            });
        }
        if input.to_state == TaskLifecycleState::Done && cause.kind != "merge_operation" {
            return Err(ServiceError::invalid_operation(
                "done requires the exact successful TaskMerge operation",
            ));
        }
        if lifecycle.state == TaskLifecycleState::Merging
            && input.to_state == TaskLifecycleState::Blocked
            && cause.kind != "merge_operation"
        {
            return Err(ServiceError::invalid_operation(
                "failed merge lifecycle transition requires its exact TaskMerge operation",
            ));
        }

        let now = now_rfc3339();
        let transition_id = new_uuid_v4();
        let event_id = new_uuid_v4();
        let event = CreateDomainEvent {
            id: event_id,
            event_type: "task.lifecycle_changed".to_owned(),
            entity_type: "task".to_owned(),
            entity_id: task.id.clone(),
            actor_type: cause.actor_type.clone(),
            actor_id: cause.actor_id.clone(),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: input.idempotency_key.clone(),
            causation_id: cause.cause_ref.clone(),
            causation_depth: 1,
            dedupe_key: Some(format!(
                "task-lifecycle:{}:{}",
                task.id, input.idempotency_key
            )),
            payload_json: json!({
                "task_id": task.id,
                "from_state": lifecycle.state,
                "to_state": input.to_state,
                "from_version": lifecycle.version,
                "to_version": lifecycle.version + 1,
                "cause_kind": cause.kind,
                "cause_ref": cause.cause_ref,
                "gate_evaluation_id": cause.gate_evaluation_id,
                "reason_kind": input.reason_kind,
                "reason_ref": input.reason_ref,
            })
            .to_string(),
            created_at: now.clone(),
        };
        let write = TaskLifecycleRepo::transition_task_lifecycle(
            &*self.db,
            TransitionTaskLifecycle {
                id: transition_id,
                task_id: task.id.clone(),
                expected_task_version: input.expected_task_version,
                expected_lifecycle_version: lifecycle.version,
                expected_state: lifecycle.state,
                to_state: input.to_state,
                cause_kind: cause.kind,
                cause_ref: cause.cause_ref,
                reason_kind: input.reason_kind,
                reason_ref: input.reason_ref,
                gate_evaluation_id: cause.gate_evaluation_id,
                idempotency_key: input.idempotency_key,
                updated_at: now,
                event,
            },
        )
        .await?;
        if let Some(event) = write.event.as_ref() {
            DomainEventService::publish_committed_hint(&self.event_bus, event);
        }
        let updated_task = TaskRepo::get_by_id(&*self.db, &task.id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?;
        Ok(TaskLifecycleTransitionResult {
            task: updated_task,
            lifecycle: write.lifecycle.clone(),
            transition: Some(write),
        })
    }

    async fn validate_gate_lifecycle_cause(
        &self,
        task_id: &str,
        from_state: TaskLifecycleState,
        to_state: TaskLifecycleState,
        current_reason_ref: Option<&str>,
        reason_ref: Option<&str>,
        evaluation_id: &str,
    ) -> Result<()> {
        let evaluation = GateRepo::get_gate_evaluation(&*self.db, evaluation_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("GateEvaluation", evaluation_id.to_owned()))?;
        let gate = GateRepo::get_gate(&*self.db, &evaluation.gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("Gate", evaluation.gate_id.clone()))?;
        let is_merge_readiness = evaluation.task_id == task_id
            && gate.task_id == task_id
            && gate.gate_kind == "merge_readiness"
            && gate.scope_kind == db::GateScopeKind::Task
            && gate.scope_id == task_id
            && gate.active_policy_revision == Some(evaluation.policy_revision);
        let is_supported_edge = match (from_state, to_state) {
            (TaskLifecycleState::Active, TaskLifecycleState::ReadyToMerge) => {
                evaluation.outcome == GateEvaluationOutcome::Satisfied
                    && reason_ref == Some(evaluation_id)
            }
            (TaskLifecycleState::ReadyToMerge, TaskLifecycleState::Active) => {
                current_reason_ref != Some(evaluation_id) && reason_ref == Some(evaluation_id)
            }
            _ => false,
        };
        if !is_merge_readiness
            || !is_supported_edge
            || !GateRepo::is_latest_gate_evaluation(
                &*self.db,
                &gate.id,
                evaluation.policy_revision,
                evaluation_id,
            )
            .await?
            || !GateRepo::gate_evaluation_inputs_are_current(&*self.db, evaluation_id).await?
        {
            return Err(ServiceError::invalid_operation(
                "GateEvaluation is stale or does not authorize this exact merge lifecycle edge",
            ));
        }
        Ok(())
    }

    async fn validate_cause(&self, task_id: &str, cause: &LifecycleCause) -> Result<()> {
        let matches_task = match cause {
            LifecycleCause::Actor(_) | LifecycleCause::System(_) => return Ok(()),
            LifecycleCause::GateEvaluation(id) => GateRepo::get_gate_evaluation(&*self.db, id)
                .await?
                .is_some_and(|evaluation| evaluation.task_id == task_id),
            LifecycleCause::Execution(id) => ExecutionRepo::get_by_id(&*self.db, id)
                .await?
                .is_some_and(|execution| execution.task_id == task_id),
            LifecycleCause::ValidationRun(id) => {
                ValidationRunRepo::get_validation_run(&*self.db, id)
                    .await?
                    .is_some_and(|run| run.task_id == task_id)
            }
            LifecycleCause::WorkUnit(id) => WorkUnitRepo::get_by_id(&*self.db, id)
                .await?
                .is_some_and(|unit| unit.task_id == task_id),
            LifecycleCause::MergeOperation(id) => {
                TaskIntegrationOperationRepo::get_by_id(&*self.db, id)
                    .await?
                    .is_some_and(|operation| operation.task_id == task_id)
            }
            LifecycleCause::DomainEvent(id) => DomainEventRepo::get_event(&*self.db, id)
                .await?
                .is_some_and(|event| event.scope_type == "task" && event.scope_id == task_id),
        };
        if matches_task {
            Ok(())
        } else {
            Err(ServiceError::invalid_operation(
                "Task lifecycle cause is missing or belongs to another Task",
            ))
        }
    }
}

struct CauseFields {
    kind: String,
    cause_ref: Option<String>,
    gate_evaluation_id: Option<String>,
    actor_type: String,
    actor_id: Option<String>,
}

fn lifecycle_cause(cause: LifecycleCause) -> CauseFields {
    match cause {
        LifecycleCause::Actor(actor) => match actor {
            Actor::User { user_id, source } => CauseFields {
                kind: "actor".to_owned(),
                cause_ref: Some(format!(
                    "user:{}:source:{source}",
                    user_id.as_deref().unwrap_or("unknown")
                )),
                gate_evaluation_id: None,
                actor_type: "human".to_owned(),
                actor_id: user_id,
            },
            Actor::Agent {
                agent_id,
                execution_id,
            } => CauseFields {
                kind: "actor".to_owned(),
                cause_ref: Some(format!(
                    "agent:{agent_id}:execution:{}",
                    execution_id.as_deref().unwrap_or("none")
                )),
                gate_evaluation_id: None,
                actor_type: "agent".to_owned(),
                actor_id: Some(agent_id),
            },
            Actor::System { component } => CauseFields {
                kind: "actor".to_owned(),
                cause_ref: Some(format!("system:{component}")),
                gate_evaluation_id: None,
                actor_type: "system".to_owned(),
                actor_id: None,
            },
        },
        LifecycleCause::GateEvaluation(id) => CauseFields {
            kind: "gate_evaluation".to_owned(),
            cause_ref: Some(id.clone()),
            gate_evaluation_id: Some(id),
            actor_type: "system".to_owned(),
            actor_id: None,
        },
        LifecycleCause::Execution(id) => fact_cause("execution", id),
        LifecycleCause::ValidationRun(id) => fact_cause("validation_run", id),
        LifecycleCause::WorkUnit(id) => fact_cause("work_unit", id),
        LifecycleCause::MergeOperation(id) => fact_cause("merge_operation", id),
        LifecycleCause::DomainEvent(id) => fact_cause("domain_event", id),
        LifecycleCause::System(component) => CauseFields {
            kind: "system".to_owned(),
            cause_ref: Some(component.to_string()),
            gate_evaluation_id: None,
            actor_type: "system".to_owned(),
            actor_id: None,
        },
    }
}

fn fact_cause(kind: &str, id: String) -> CauseFields {
    CauseFields {
        kind: kind.to_owned(),
        cause_ref: Some(id),
        gate_evaluation_id: None,
        actor_type: "system".to_owned(),
        actor_id: None,
    }
}

fn allowed_transition(from: TaskLifecycleState, to: TaskLifecycleState) -> bool {
    use TaskLifecycleState as S;
    matches!(
        (from, to),
        (S::Backlog, S::Ready | S::Cancelled)
            | (S::Backlog, S::Blocked)
            | (S::Ready, S::Active | S::Blocked | S::Cancelled)
            | (
                S::Active,
                S::Ready | S::Blocked | S::ReadyToMerge | S::Cancelled
            )
            | (S::Blocked, S::Ready | S::Active | S::Cancelled)
            | (
                S::ReadyToMerge,
                S::Active | S::Merging | S::Blocked | S::Cancelled
            )
            | (S::Merging, S::Done | S::Blocked)
    )
}

pub fn lifecycle_state_for_legacy_status(status: &str) -> Option<TaskLifecycleState> {
    match status {
        "backlog" => Some(TaskLifecycleState::Backlog),
        "todo" | "ready" => Some(TaskLifecycleState::Ready),
        "planning" | "in_progress" | "working" => Some(TaskLifecycleState::Active),
        "review" => Some(TaskLifecycleState::Blocked),
        "blocked" | "merge_failed" => Some(TaskLifecycleState::Blocked),
        "ready_to_merge" => Some(TaskLifecycleState::ReadyToMerge),
        "merging" => Some(TaskLifecycleState::Merging),
        "done" => Some(TaskLifecycleState::Done),
        "cancelled" => Some(TaskLifecycleState::Cancelled),
        _ => None,
    }
}

pub fn legacy_status_projection(state: TaskLifecycleState) -> &'static str {
    match state {
        TaskLifecycleState::Backlog => "backlog",
        TaskLifecycleState::Ready => "todo",
        TaskLifecycleState::Active
        | TaskLifecycleState::ReadyToMerge
        | TaskLifecycleState::Merging => "in_progress",
        TaskLifecycleState::Blocked => "blocked",
        TaskLifecycleState::Done => "done",
        TaskLifecycleState::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate_engine::{
        GateEngine, GatePolicyDocument, GateScopeRequirement, ValidationRequirement,
    };
    use db::{
        CreateDomainEvent, CreateEvidence, CreateProject, CreateRepo, CreateTask,
        CreateTaskIntegrationOperation, CreateValidationRun, CreateWorkspace,
        FinishTaskIntegrationOperation, FinishValidationRun, GateScopeKind, ProjectRepo, RepoRepo,
        TaskIntegrationOperationKind, TaskIntegrationOperationRepo, TaskIntegrationOperationStatus,
        TaskLifecycleRepo, ValidationRunRepo, ValidationRunStatus, WorkMode, WorkspaceRepo,
        WorkspaceStatus,
    };
    use sha2::Digest;

    async fn seeded_ready_task() -> (Arc<SqliteDb>, Arc<EventBus>, Task) {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool");
        db::run_migrations(&pool).await.expect("migrations");
        let db = Arc::new(SqliteDb::new(pool));
        let now = now_rfc3339();
        let project_id = new_uuid_v4();
        ProjectRepo::create(
            &*db,
            CreateProject {
                id: project_id.clone(),
                name: "PR9 Gate and lifecycle test".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Project");
        let repo_id = new_uuid_v4();
        RepoRepo::create(
            &*db,
            CreateRepo {
                id: repo_id.clone(),
                project_id: project_id.clone(),
                name: "test repo".to_owned(),
                remote_url: "https://example.invalid/forge.git".to_owned(),
                local_path: None,
                work_mode: WorkMode::DirectMerge,
                default_branch: "main".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Repo");
        let task = TaskRepo::create(
            &*db,
            CreateTask {
                id: new_uuid_v4(),
                project_id,
                repo_id: Some(repo_id),
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "Lifecycle target".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("Task");
        (db, Arc::new(EventBus::new(16)), task)
    }

    #[test]
    fn legacy_workflow_labels_collapse_into_aggregate_progress() {
        assert_eq!(
            lifecycle_state_for_legacy_status("ready"),
            Some(TaskLifecycleState::Ready)
        );
        assert_eq!(
            lifecycle_state_for_legacy_status("planning"),
            Some(TaskLifecycleState::Active)
        );
        assert_eq!(
            lifecycle_state_for_legacy_status("in_progress"),
            Some(TaskLifecycleState::Active)
        );
        assert_eq!(
            lifecycle_state_for_legacy_status("working"),
            Some(TaskLifecycleState::Active)
        );
        assert_eq!(
            lifecycle_state_for_legacy_status("review"),
            Some(TaskLifecycleState::Blocked)
        );
        assert_eq!(
            lifecycle_state_for_legacy_status("merge_failed"),
            Some(TaskLifecycleState::Blocked)
        );
        assert_eq!(lifecycle_state_for_legacy_status("unknown"), None);
    }

    #[test]
    fn merge_states_require_gate_cause_and_normal_edges_are_finite() {
        assert!(allowed_transition(
            TaskLifecycleState::Active,
            TaskLifecycleState::ReadyToMerge
        ));
        assert!(allowed_transition(
            TaskLifecycleState::ReadyToMerge,
            TaskLifecycleState::Merging
        ));
        assert!(!allowed_transition(
            TaskLifecycleState::Active,
            TaskLifecycleState::Done
        ));
        assert!(!allowed_transition(
            TaskLifecycleState::Backlog,
            TaskLifecycleState::Merging
        ));
    }

    #[tokio::test]
    async fn block_records_aggregate_lifecycle_and_keeps_legacy_annotation_out_of_authority() {
        let (db, event_bus, task) = seeded_ready_task().await;
        let service = TaskLifecycleService::new(Arc::clone(&db), event_bus);
        let blocked = service
            .block(
                &task.id,
                LifecycleCause::System(SystemComponent::TaskDispatcher),
                "executor_unavailable",
                "execution:unavailable-1",
                "executor-unavailable:execution:unavailable-1",
            )
            .await
            .expect("block transition commits");
        assert_eq!(blocked.lifecycle.state, TaskLifecycleState::Blocked);
        assert_eq!(
            blocked.lifecycle.reason_kind.as_deref(),
            Some("executor_unavailable")
        );
        assert_eq!(
            blocked.lifecycle.reason_ref.as_deref(),
            Some("execution:unavailable-1")
        );
        assert!(blocked.transition.is_some());

        let replay = service
            .block(
                &task.id,
                LifecycleCause::System(SystemComponent::TaskDispatcher),
                "executor_unavailable",
                "execution:unavailable-1",
                "executor-unavailable:execution:unavailable-1",
            )
            .await
            .expect("repeated block is stable");
        assert_eq!(replay.lifecycle.version, blocked.lifecycle.version);
        assert!(replay.transition.is_none());
    }

    #[tokio::test]
    async fn lifecycle_transition_fences_versions_and_replays_one_exact_receipt() {
        let (db, event_bus, task) = seeded_ready_task().await;
        let service = TaskLifecycleService::new(Arc::clone(&db), event_bus);
        let input = TransitionLifecycleInput {
            task_id: task.id.clone(),
            expected_task_version: task.version,
            to_state: TaskLifecycleState::Active,
            cause: LifecycleCause::Actor(Actor::user(api_types::UserActionSource::Test)),
            reason_kind: Some("test_start".to_owned()),
            reason_ref: Some("start once".to_owned()),
            idempotency_key: "start:one:exact-request".to_owned(),
        };
        let first = service.transition(input.clone()).await.expect("transition");
        assert_eq!(first.lifecycle.version, 2);
        assert_eq!(first.task.status, "in_progress");
        assert!(!first.transition.as_ref().unwrap().replayed);

        let replay = service.transition(input.clone()).await.expect("replay");
        assert!(replay.transition.as_ref().unwrap().replayed);
        assert_eq!(
            replay.transition.as_ref().unwrap().transition_id,
            first.transition.as_ref().unwrap().transition_id
        );

        let mut conflicting = input.clone();
        conflicting.cause = LifecycleCause::Actor(Actor::user(api_types::UserActionSource::Api));
        assert!(matches!(
            service.transition(conflicting).await,
            Err(ServiceError::Db(db::DbError::IdempotencyConflict))
        ));

        let mut stale = input;
        stale.idempotency_key = "start:stale:request".to_owned();
        stale.to_state = TaskLifecycleState::Blocked;
        assert!(matches!(
            service.transition(stale).await,
            Err(ServiceError::Db(db::DbError::TaskVersionConflict { .. }))
        ));

        assert!(TaskIntegrationOperationRepo::begin(
            &*db,
            CreateTaskIntegrationOperation {
                id: new_uuid_v4(),
                task_id: task.id.clone(),
                kind: TaskIntegrationOperationKind::TaskMerge,
                owner_id: "no-gate-admission".to_owned(),
                gate_evaluation_id: None,
                created_at: now_rfc3339(),
            },
        )
        .await
        .is_err());
        assert!(service
            .transition(TransitionLifecycleInput {
                task_id: task.id,
                expected_task_version: first.task.version,
                to_state: TaskLifecycleState::ReadyToMerge,
                cause: LifecycleCause::Actor(Actor::user(api_types::UserActionSource::Test)),
                reason_kind: Some("test_bypass".to_owned()),
                reason_ref: Some("no GateEvaluation".to_owned()),
                idempotency_key: "merge-readiness:no-gate".to_owned(),
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn concurrent_lifecycle_transitions_commit_one_version_fenced_winner() {
        let (db, event_bus, task) = seeded_ready_task().await;
        let service = TaskLifecycleService::new(Arc::clone(&db), event_bus);
        let input = |idempotency_key: &str| TransitionLifecycleInput {
            task_id: task.id.clone(),
            expected_task_version: task.version,
            to_state: TaskLifecycleState::Active,
            cause: LifecycleCause::Actor(Actor::user(api_types::UserActionSource::Test)),
            reason_kind: Some("concurrent_start".to_owned()),
            reason_ref: Some(idempotency_key.to_owned()),
            idempotency_key: idempotency_key.to_owned(),
        };
        let (left, right) = tokio::join!(
            service.transition(input("concurrent-transition:left")),
            service.transition(input("concurrent-transition:right")),
        );
        assert_ne!(left.is_ok(), right.is_ok());
        let loser = if left.is_err() { left } else { right };
        assert!(matches!(
            loser,
            Err(ServiceError::Db(
                db::DbError::TaskVersionConflict { .. } | db::DbError::VersionConflict
            ))
        ));
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.state, TaskLifecycleState::Active);
        assert_eq!(lifecycle.version, 2);
        let transition_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_lifecycle_transition WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .expect("lifecycle transition count");
        assert_eq!(transition_count, 1);
    }

    #[tokio::test]
    async fn gate_evaluations_are_deterministic_immutable_and_revision_scoped() {
        let (db, event_bus, task) = seeded_ready_task().await;
        let engine = GateEngine::new(Arc::clone(&db), event_bus);
        let gate = engine
            .create_gate(&task.id, "validation", GateScopeKind::Task, &task.id)
            .await
            .expect("Gate");
        let requirement = ValidationRequirement {
            validation_run_id: "missing-run".to_owned(),
            evidence_id: "missing-evidence".to_owned(),
            evidence_digest: "a".repeat(64),
            check_identity: "cargo-test".to_owned(),
            config_digest: "b".repeat(64),
            workspace_id: "workspace-exact".to_owned(),
            commit_sha: "c".repeat(40),
            workspace_snapshot_digest: "d".repeat(64),
            required_outcome: ValidationRunStatus::Passed,
        };
        let policy = |config_digest: &str| GatePolicyDocument {
            schema_version: 1,
            scope_requirement: None,
            review: None,
            validations: vec![ValidationRequirement {
                config_digest: config_digest.to_owned(),
                ..requirement.clone()
            }],
            decisions: Vec::new(),
            work_units: Vec::new(),
        };
        let first_policy = engine
            .revise_policy(&gate.id, None, policy(&requirement.config_digest))
            .await
            .expect("first immutable policy revision");
        let (first, concurrent) = tokio::join!(
            engine.evaluate_active(&gate.id),
            engine.evaluate_active(&gate.id)
        );
        let first = first.expect("evaluation with missing exact validation");
        let concurrent = concurrent.expect("concurrent evaluation deduplicates");
        assert_eq!(first.evaluation.outcome, GateEvaluationOutcome::Unsatisfied);
        assert_eq!(first.evaluation.id, concurrent.evaluation.id);
        assert_ne!(first.event.is_some(), concurrent.event.is_some());
        let replay = engine
            .evaluate_active(&gate.id)
            .await
            .expect("deduplicated replay");
        assert_eq!(replay.evaluation.id, first.evaluation.id);
        assert!(replay.event.is_none());

        let revised = engine
            .revise_policy(&gate.id, Some(1), policy(&"e".repeat(64)))
            .await
            .expect("second policy revision");
        assert_eq!(revised.revision, 2);
        let second = engine
            .evaluate_active(&gate.id)
            .await
            .expect("evaluation under new revision");
        assert_ne!(second.evaluation.id, first.evaluation.id);
        assert_eq!(second.evaluation.policy_revision, 2);

        let policy_update = sqlx::query(
            "UPDATE gate_policy_revision SET policy_json = '{}' WHERE gate_id = ? AND revision = ?",
        )
        .bind(&gate.id)
        .bind(first_policy.revision)
        .execute(db.pool())
        .await;
        assert!(policy_update.is_err());
        let evaluation_update =
            sqlx::query("UPDATE gate_evaluation SET outcome = 'satisfied' WHERE id = ?")
                .bind(&first.evaluation.id)
                .execute(db.pool())
                .await;
        assert!(evaluation_update.is_err());
    }

    #[tokio::test]
    async fn exact_validation_gate_moves_to_merge_ready_and_new_policy_revokes_stale_readiness() {
        let (db, event_bus, task) = seeded_ready_task().await;
        let lifecycle = TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus));
        let _active = lifecycle
            .transition(TransitionLifecycleInput {
                task_id: task.id.clone(),
                expected_task_version: task.version,
                to_state: TaskLifecycleState::Active,
                cause: LifecycleCause::Actor(Actor::user(api_types::UserActionSource::Test)),
                reason_kind: Some("test_start".to_owned()),
                reason_ref: Some("begin deterministic validation".to_owned()),
                idempotency_key: "validation-gate:start-active".to_owned(),
            })
            .await
            .expect("Task becomes active");

        let workspace_id = new_uuid_v4();
        let now = now_rfc3339();
        WorkspaceRepo::create(
            &*db,
            CreateWorkspace {
                id: workspace_id.clone(),
                task_id: task.id.clone(),
                repo_id: task.repo_id.clone().expect("Task repository"),
                worktree_path: "/tmp/pr9-gate-workspace".to_owned(),
                branch: "task/pr9".to_owned(),
                status: WorkspaceStatus::Ready,
                before_sha: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Workspace");

        let run_id = new_uuid_v4();
        let evidence_id = new_uuid_v4();
        let summary = "{}".to_owned();
        let config_digest = sha256(summary.as_bytes());
        let commit_sha = "c".repeat(40);
        let snapshot_digest = "d".repeat(64);
        let check_identity = "cargo test -p services".to_owned();
        let run_event = event(
            "validation_run.started",
            "validation_run",
            &run_id,
            &task.id,
            "validation-run-started:test",
            &now,
        );
        let started = ValidationRunRepo::start_validation_run(
            &*db,
            CreateValidationRun {
                id: run_id.clone(),
                task_id: task.id.clone(),
                work_unit_id: None,
                caused_by_execution_id: None,
                check_identity: check_identity.clone(),
                command: "cargo test -p services".to_owned(),
                config_summary_json: summary,
                config_digest: config_digest.clone(),
                workspace_id: workspace_id.clone(),
                commit_sha: commit_sha.clone(),
                workspace_snapshot_digest: snapshot_digest.clone(),
                idempotency_key: "validation-run:pr9-exact-pass".to_owned(),
                started_at: now.clone(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            run_event,
        )
        .await
        .expect("ValidationRun starts");
        assert!(ValidationRunRepo::claim_validation_run(
            &*db,
            &started.validation_run.id,
            "validation-test-owner",
            &now,
            "2099-01-01T00:00:00Z",
        )
        .await
        .expect("ValidationRun claim"));

        let finished_at = now_rfc3339();
        let evidence_content = serde_json::json!({
            "validation_run_id": run_id,
            "task_id": task.id,
            "check_identity": check_identity,
            "command": "cargo test -p services",
            "config_digest": config_digest,
            "workspace_id": workspace_id,
            "commit_sha": commit_sha,
            "workspace_snapshot_digest": snapshot_digest,
            "status": "passed",
            "exit_code": 0,
            "started_at": started.validation_run.started_at,
            "finished_at": finished_at,
        })
        .to_string();
        let evidence_digest = sha256(evidence_content.as_bytes());
        let _completion = ValidationRunRepo::finish_validation_run(
            &*db,
            FinishValidationRun {
                id: run_id.clone(),
                claim_owner: "validation-test-owner".to_owned(),
                status: ValidationRunStatus::Passed,
                exit_code: Some(0),
                finished_at: finished_at.clone(),
                logs_ref: format!("validation-evidence://{evidence_id}"),
                evidence: vec![CreateEvidence {
                    id: evidence_id.clone(),
                    task_id: task.id.clone(),
                    validation_run_id: run_id.clone(),
                    evidence_key: "check-output".to_owned(),
                    kind: "deterministic_check_output".to_owned(),
                    content_json: evidence_content,
                    digest: evidence_digest.clone(),
                    created_at: finished_at.clone(),
                }],
                validation_report: None,
                events: vec![event(
                    "validation_run.completed",
                    "validation_run",
                    &run_id,
                    &task.id,
                    "validation-run-terminal:test",
                    &finished_at,
                )],
            },
        )
        .await
        .expect("ValidationRun completes");

        let gate_engine = GateEngine::new(Arc::clone(&db), Arc::clone(&event_bus));
        let gate = gate_engine
            .create_gate(&task.id, "merge_readiness", GateScopeKind::Task, &task.id)
            .await
            .expect("merge-readiness Gate");
        let policy = |policy_config_digest: &str| GatePolicyDocument {
            schema_version: 1,
            scope_requirement: None,
            review: None,
            validations: vec![ValidationRequirement {
                validation_run_id: run_id.clone(),
                evidence_id: evidence_id.clone(),
                evidence_digest: evidence_digest.clone(),
                check_identity: check_identity.clone(),
                config_digest: policy_config_digest.to_owned(),
                workspace_id: workspace_id.clone(),
                commit_sha: commit_sha.clone(),
                workspace_snapshot_digest: snapshot_digest.clone(),
                required_outcome: ValidationRunStatus::Passed,
            }],
            decisions: Vec::new(),
            work_units: Vec::new(),
        };
        gate_engine
            .revise_policy(&gate.id, None, policy(&config_digest))
            .await
            .expect("exact validation policy");
        let direct_evaluation = gate_engine
            .evaluate_active(&gate.id)
            .await
            .expect("explicit exact evaluation");
        assert_eq!(
            direct_evaluation.evaluation.outcome,
            GateEvaluationOutcome::Satisfied
        );
        let evaluation_event = direct_evaluation
            .event
            .as_ref()
            .expect("evaluation event is durable")
            .clone();
        assert_eq!(
            gate_engine
                .process_domain_event(&evaluation_event)
                .await
                .expect("replay applies exact GateEvaluation lifecycle effect"),
            0
        );
        let ready = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("lifecycle lookup")
            .expect("lifecycle");
        assert_eq!(ready.state, TaskLifecycleState::ReadyToMerge);
        let exact_evaluation_id = ready.reason_ref.clone().expect("GateEvaluation binding");
        assert_eq!(
            GateRepo::get_gate_evaluation(&*db, &exact_evaluation_id)
                .await
                .expect("evaluation lookup")
                .expect("evaluation")
                .outcome,
            GateEvaluationOutcome::Satisfied
        );

        let ready_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("Task lookup")
            .expect("Task");
        let same_evaluation_demotion = lifecycle
            .transition(TransitionLifecycleInput {
                task_id: task.id.clone(),
                expected_task_version: ready_task.version,
                to_state: TaskLifecycleState::Active,
                cause: LifecycleCause::GateEvaluation(exact_evaluation_id.clone()),
                reason_kind: Some("merge_readiness_rechecked".to_owned()),
                reason_ref: Some(exact_evaluation_id.clone()),
                idempotency_key: "gate-readiness:reject-same-evaluation-demotion".to_owned(),
            })
            .await;
        assert!(matches!(
            same_evaluation_demotion,
            Err(ServiceError::InvalidOperation { .. })
        ));

        let actor_demotion = lifecycle
            .transition(TransitionLifecycleInput {
                task_id: task.id.clone(),
                expected_task_version: ready_task.version,
                to_state: TaskLifecycleState::Active,
                cause: LifecycleCause::Actor(Actor::user(api_types::UserActionSource::Test)),
                reason_kind: Some("rework_requested".to_owned()),
                reason_ref: Some("review changed after readiness".to_owned()),
                idempotency_key: "gate-readiness:reject-actor-demotion".to_owned(),
            })
            .await;
        assert!(matches!(
            actor_demotion,
            Err(ServiceError::InvalidOperation { .. })
        ));

        let merge_operation_id = new_uuid_v4();
        let merge_operation = TaskIntegrationOperationRepo::begin(
            &*db,
            CreateTaskIntegrationOperation {
                id: merge_operation_id.clone(),
                task_id: task.id.clone(),
                kind: TaskIntegrationOperationKind::TaskMerge,
                owner_id: "gate-scope-test".to_owned(),
                gate_evaluation_id: Some(exact_evaluation_id.clone()),
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("exact merge admission");
        let merge_scope_policy = GatePolicyDocument {
            schema_version: 1,
            scope_requirement: Some(GateScopeRequirement::MergeOperation {
                operation_id: merge_operation.id.clone(),
                version: merge_operation.version,
                expected_status: merge_operation.status.to_string(),
                gate_evaluation_id: exact_evaluation_id.clone(),
            }),
            review: None,
            validations: Vec::new(),
            decisions: Vec::new(),
            work_units: Vec::new(),
        };
        let (merge_operation_gate, _) = gate_engine
            .create_gate_with_initial_policy(
                &task.id,
                "merge_operation_audit",
                GateScopeKind::MergeOperation,
                &merge_operation.id,
                merge_scope_policy,
            )
            .await
            .expect("merge-operation-scoped Gate");
        let merge_scope_evaluation = gate_engine
            .evaluate_active(&merge_operation_gate.id)
            .await
            .expect("merge operation Gate evaluation");
        assert_eq!(
            merge_scope_evaluation.evaluation.outcome,
            GateEvaluationOutcome::Satisfied
        );
        assert_eq!(merge_scope_evaluation.inputs.len(), 1);
        assert_eq!(
            merge_scope_evaluation.inputs[0].input_kind,
            "merge_operation"
        );
        assert_eq!(
            merge_scope_evaluation.inputs[0].input_id,
            merge_operation.id
        );
        assert!(GateRepo::gate_evaluation_inputs_are_current(
            &*db,
            &merge_scope_evaluation.evaluation.id
        )
        .await
        .expect("merge operation input currentness"));

        TaskIntegrationOperationRepo::finish(
            &*db,
            FinishTaskIntegrationOperation {
                id: merge_operation_id.clone(),
                expected_version: merge_operation.version,
                status: TaskIntegrationOperationStatus::Failed,
                updated_at: now_rfc3339(),
                finished_at: now_rfc3339(),
            },
        )
        .await
        .expect("merge operation failure is durable");
        assert!(!GateRepo::gate_evaluation_inputs_are_current(
            &*db,
            &merge_scope_evaluation.evaluation.id
        )
        .await
        .expect("finished merge makes the running input stale"));
        let merge_finished_event = db::DomainEventRepo::get_event_by_dedupe(
            &*db,
            &format!("task-merge-terminal:{merge_operation_id}"),
        )
        .await
        .expect("merge terminal event query")
        .expect("durable merge terminal event");
        assert_eq!(
            gate_engine
                .process_domain_event(&merge_finished_event)
                .await
                .expect("operation fact reevaluation"),
            1
        );
        let failed_scope_evaluation = GateRepo::get_gate_evaluation_for_cause(
            &*db,
            &merge_operation_gate.id,
            &merge_finished_event.id,
        )
        .await
        .expect("replayed exact operation-scoped evaluation")
        .expect("new evaluation for terminal operation state");
        assert_eq!(
            failed_scope_evaluation.outcome,
            GateEvaluationOutcome::Unsatisfied
        );

        let ready_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("Task lookup")
            .expect("Task");
        TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .transition(TransitionLifecycleInput {
                task_id: task.id.clone(),
                expected_task_version: ready_task.version,
                to_state: TaskLifecycleState::Active,
                cause: LifecycleCause::Actor(api_types::Actor::user(
                    api_types::UserActionSource::Test,
                )),
                reason_kind: Some("reopen_after_readiness".to_owned()),
                reason_ref: Some("exercise exact evaluation replay".to_owned()),
                idempotency_key: "test:reopen-after-readiness".to_owned(),
            })
            .await
            .expect("explicit re-entry to active lifecycle");
        gate_engine
            .process_domain_event(&evaluation_event)
            .await
            .expect("replaying an already-applied GateEvaluation is a no-op");
        assert_eq!(
            TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
                .await
                .expect("lifecycle lookup")
                .expect("lifecycle")
                .state,
            TaskLifecycleState::Active,
            "replay must not reapply a previously committed lifecycle transition"
        );
        gate_engine
            .revise_policy(&gate.id, Some(1), policy(&"e".repeat(64)))
            .await
            .expect("new policy revision");
        let policy_event = db::DomainEventRepo::get_event_by_dedupe(
            &*db,
            &format!("gate.policy_revised:{}:policy:2", gate.id),
        )
        .await
        .expect("policy event query")
        .expect("policy event");
        gate_engine
            .process_domain_event(&policy_event)
            .await
            .expect("new policy evaluation");
        let revoked = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("lifecycle lookup")
            .expect("lifecycle");
        assert_eq!(revoked.state, TaskLifecycleState::Active);
        assert_ne!(
            revoked.reason_ref.as_deref(),
            Some(exact_evaluation_id.as_str())
        );

        let exact_validation = ValidationRequirement {
            validation_run_id: run_id.clone(),
            evidence_id: evidence_id.clone(),
            evidence_digest: evidence_digest.clone(),
            check_identity: check_identity.clone(),
            config_digest: config_digest.clone(),
            workspace_id: workspace_id.clone(),
            commit_sha: commit_sha.clone(),
            workspace_snapshot_digest: snapshot_digest.clone(),
            required_outcome: ValidationRunStatus::Passed,
        };
        let validation_policy = |requirement| GatePolicyDocument {
            schema_version: 1,
            scope_requirement: None,
            review: None,
            validations: vec![requirement],
            decisions: Vec::new(),
            work_units: Vec::new(),
        };
        let (validation_gate, _) = gate_engine
            .create_gate_with_initial_policy(
                &task.id,
                "validation_exactness_probe",
                GateScopeKind::Task,
                &task.id,
                validation_policy(exact_validation.clone()),
            )
            .await
            .expect("exact validation Gate");
        let exact = gate_engine
            .evaluate_active(&validation_gate.id)
            .await
            .expect("exact PASS validation and Evidence");
        assert_eq!(exact.evaluation.outcome, GateEvaluationOutcome::Satisfied);

        let mut mismatches = Vec::new();
        let mut wrong_check = exact_validation.clone();
        wrong_check.check_identity.push_str(" changed");
        mismatches.push(wrong_check);
        let mut wrong_config = exact_validation.clone();
        wrong_config.config_digest = "f".repeat(64);
        mismatches.push(wrong_config);
        let mut wrong_workspace = exact_validation.clone();
        wrong_workspace.workspace_id.push_str("-other");
        mismatches.push(wrong_workspace);
        let mut stale_commit = exact_validation.clone();
        stale_commit.commit_sha = "e".repeat(40);
        mismatches.push(stale_commit);
        let mut stale_snapshot = exact_validation.clone();
        stale_snapshot.workspace_snapshot_digest = "f".repeat(64);
        mismatches.push(stale_snapshot);
        let mut wrong_outcome = exact_validation;
        wrong_outcome.required_outcome = ValidationRunStatus::Failed;
        mismatches.push(wrong_outcome);

        let mut policy_revision = 1;
        for requirement in mismatches {
            gate_engine
                .revise_policy(
                    &validation_gate.id,
                    Some(policy_revision),
                    validation_policy(requirement),
                )
                .await
                .expect("append immutable mismatch policy");
            policy_revision += 1;
            let evaluation = gate_engine
                .evaluate_active(&validation_gate.id)
                .await
                .expect("mismatched exact validation remains unsatisfied");
            assert_eq!(
                evaluation.evaluation.outcome,
                GateEvaluationOutcome::Unsatisfied
            );
        }

        let supplemental_run_id = new_uuid_v4();
        let supplemental_evidence_id = new_uuid_v4();
        let supplemental_check = "cargo fmt --all -- --check".to_owned();
        let supplemental_summary = r#"{"source":"lifecycle rework regression"}"#.to_owned();
        let supplemental_config_digest = sha256(supplemental_summary.as_bytes());
        let supplemental_now = now_rfc3339();
        let supplemental_started = ValidationRunRepo::start_validation_run(
            &*db,
            CreateValidationRun {
                id: supplemental_run_id.clone(),
                task_id: task.id.clone(),
                work_unit_id: None,
                caused_by_execution_id: None,
                check_identity: supplemental_check.clone(),
                command: supplemental_check.clone(),
                config_summary_json: supplemental_summary,
                config_digest: supplemental_config_digest.clone(),
                workspace_id: workspace_id.clone(),
                commit_sha: commit_sha.clone(),
                workspace_snapshot_digest: snapshot_digest.clone(),
                idempotency_key: "validation-run:lifecycle-rework-regression".to_owned(),
                started_at: supplemental_now.clone(),
                created_at: supplemental_now.clone(),
                updated_at: supplemental_now.clone(),
            },
            event(
                "validation_run.started",
                "validation_run",
                &supplemental_run_id,
                &task.id,
                "validation-run-started:lifecycle-rework-regression",
                &supplemental_now,
            ),
        )
        .await
        .expect("supplemental exact ValidationRun starts");
        assert!(ValidationRunRepo::claim_validation_run(
            &*db,
            &supplemental_run_id,
            "lifecycle-rework-owner",
            &supplemental_now,
            "2099-01-01T00:00:00Z",
        )
        .await
        .expect("supplemental ValidationRun claim"));
        let supplemental_finished = now_rfc3339();
        let supplemental_content = serde_json::json!({
            "validation_run_id": supplemental_run_id,
            "task_id": task.id,
            "check_identity": supplemental_check,
            "command": supplemental_check,
            "config_digest": supplemental_config_digest,
            "workspace_id": workspace_id,
            "commit_sha": commit_sha,
            "workspace_snapshot_digest": snapshot_digest,
            "status": "passed",
            "exit_code": 0,
            "started_at": supplemental_started.validation_run.started_at,
            "finished_at": supplemental_finished,
        })
        .to_string();
        let supplemental_evidence_digest = sha256(supplemental_content.as_bytes());
        ValidationRunRepo::finish_validation_run(
            &*db,
            FinishValidationRun {
                id: supplemental_run_id.clone(),
                claim_owner: "lifecycle-rework-owner".to_owned(),
                status: ValidationRunStatus::Passed,
                exit_code: Some(0),
                finished_at: supplemental_finished.clone(),
                logs_ref: format!("validation-evidence://{supplemental_evidence_id}"),
                evidence: vec![CreateEvidence {
                    id: supplemental_evidence_id.clone(),
                    task_id: task.id.clone(),
                    validation_run_id: supplemental_run_id.clone(),
                    evidence_key: "check-output".to_owned(),
                    kind: "deterministic_check_output".to_owned(),
                    content_json: supplemental_content,
                    digest: supplemental_evidence_digest.clone(),
                    created_at: supplemental_finished.clone(),
                }],
                validation_report: None,
                events: vec![event(
                    "validation_run.completed",
                    "validation_run",
                    &supplemental_run_id,
                    &task.id,
                    "validation-run-terminal:lifecycle-rework-regression",
                    &supplemental_finished,
                )],
            },
        )
        .await
        .expect("supplemental exact ValidationRun passes");
        let mut rework_policy = policy(&config_digest);
        rework_policy.validations.push(ValidationRequirement {
            validation_run_id: supplemental_run_id,
            evidence_id: supplemental_evidence_id,
            evidence_digest: supplemental_evidence_digest,
            check_identity: "cargo fmt --all -- --check".to_owned(),
            config_digest: sha256(br#"{"source":"lifecycle rework regression"}"#),
            workspace_id: workspace_id.clone(),
            commit_sha: commit_sha.clone(),
            workspace_snapshot_digest: snapshot_digest.clone(),
            required_outcome: ValidationRunStatus::Passed,
        });
        gate_engine
            .revise_policy(&gate.id, Some(2), rework_policy)
            .await
            .expect("new exact policy revalidates merge readiness");
        let readiness_policy_event = db::DomainEventRepo::get_event_by_dedupe(
            &*db,
            &format!("gate.policy_revised:{}:policy:3", gate.id),
        )
        .await
        .expect("policy event lookup")
        .expect("exact merge-readiness policy revision");
        assert_eq!(
            gate_engine
                .process_domain_event(&readiness_policy_event)
                .await
                .expect("new merge-readiness evaluation"),
            1
        );
        let readiness_evaluation =
            GateRepo::get_gate_evaluation_for_cause(&*db, &gate.id, &readiness_policy_event.id)
                .await
                .expect("GateEvaluation lookup")
                .expect("new exact satisfied evaluation");
        let readiness_evaluation_event_id: String = sqlx::query_scalar(
            "SELECT id FROM domain_event
             WHERE event_type = 'gate.evaluated' AND entity_id = ?",
        )
        .bind(&readiness_evaluation.id)
        .fetch_one(db.pool())
        .await
        .expect("durable GateEvaluation event");
        let readiness_evaluation_event =
            db::DomainEventRepo::get_event(&*db, &readiness_evaluation_event_id)
                .await
                .expect("GateEvaluation event lookup")
                .expect("GateEvaluation event");
        gate_engine
            .process_domain_event(&readiness_evaluation_event)
            .await
            .expect("exact satisfied GateEvaluation moves lifecycle to merge-ready");
        assert_eq!(
            TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
                .await
                .expect("lifecycle lookup")
                .expect("lifecycle")
                .state,
            TaskLifecycleState::ReadyToMerge
        );

        let failed_execution_id = new_uuid_v4();
        let (_, failed_execution_event) = ExecutionRepo::create_with_event(
            &*db,
            db::CreateExecution {
                id: failed_execution_id.clone(),
                task_id: task.id.clone(),
                agent_id: None,
                actor_ref: None,
                role: "implementer".to_owned(),
                purpose: Some(db::ExecutionPurpose::Implement),
                status: db::ExecutionStatus::Failed,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
                harness_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: Some("exact post-readiness execution failure".to_owned()),
                executor_config_snapshot_json: None,
                workspace_id: None,
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
            event(
                "execution.failed",
                "execution",
                &failed_execution_id,
                &task.id,
                "test:post-readiness-execution-failed",
                &now_rfc3339(),
            ),
        )
        .await
        .expect("exact failed Execution and source event");
        assert_eq!(
            crate::task_failure_retry::TaskFailureRetryService::new(
                Arc::clone(&db),
                Arc::clone(&event_bus),
            )
            .process_domain_event(&failed_execution_event)
            .await
            .expect("exact retry receipt reopens merge-ready Task"),
            1
        );
        let reworked = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("reworked lifecycle lookup")
            .expect("reworked lifecycle");
        assert_eq!(reworked.state, TaskLifecycleState::Active);
        assert_eq!(reworked.reason_kind.as_deref(), Some("failure_rework"));
        assert_eq!(
            reworked.reason_ref.as_deref(),
            Some(failed_execution_id.as_str())
        );
    }

    #[tokio::test]
    async fn lifecycle_operation_gate_pins_the_exact_transition_fact() {
        let (db, event_bus, task) = seeded_ready_task().await;
        let transition = TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .transition(TransitionLifecycleInput {
                task_id: task.id.clone(),
                expected_task_version: task.version,
                to_state: TaskLifecycleState::Active,
                cause: LifecycleCause::Actor(Actor::user(api_types::UserActionSource::Test)),
                reason_kind: Some("test_start".to_owned()),
                reason_ref: Some("operation-scoped Gate fixture".to_owned()),
                idempotency_key: "lifecycle-operation-gate:start".to_owned(),
            })
            .await
            .expect("Task becomes active");
        let transition_id = transition
            .transition
            .expect("durable transition receipt")
            .transition_id;
        let fact = TaskLifecycleRepo::get_task_lifecycle_transition_fact(&*db, &transition_id)
            .await
            .expect("transition fact lookup")
            .expect("transition fact");
        let policy = GatePolicyDocument {
            schema_version: 1,
            scope_requirement: Some(GateScopeRequirement::LifecycleOperation {
                transition_id: fact.id.clone(),
                from_state: fact.from_state.to_string(),
                to_state: fact.to_state.to_string(),
                from_version: fact.from_version,
                to_version: fact.to_version,
                cause_kind: fact.cause_kind.clone(),
                cause_ref: fact.cause_ref.clone(),
                gate_evaluation_id: fact.gate_evaluation_id.clone(),
            }),
            review: None,
            validations: Vec::new(),
            decisions: Vec::new(),
            work_units: Vec::new(),
        };
        let engine = GateEngine::new(Arc::clone(&db), event_bus);
        let (gate, _) = engine
            .create_gate_with_initial_policy(
                &task.id,
                "lifecycle_operation_audit",
                GateScopeKind::LifecycleOperation,
                &transition_id,
                policy,
            )
            .await
            .expect("lifecycle-operation-scoped Gate");
        let evaluation = engine
            .evaluate_active(&gate.id)
            .await
            .expect("lifecycle operation evaluation");
        assert_eq!(
            evaluation.evaluation.outcome,
            GateEvaluationOutcome::Satisfied
        );
        assert_eq!(evaluation.inputs.len(), 1);
        assert_eq!(evaluation.inputs[0].input_kind, "lifecycle_operation");
        assert_eq!(evaluation.inputs[0].input_id, transition_id);
        assert_eq!(evaluation.inputs[0].input_version, fact.to_version);
        assert!(
            GateRepo::gate_evaluation_inputs_are_current(&*db, &evaluation.evaluation.id)
                .await
                .expect("lifecycle operation input currentness")
        );
    }

    #[tokio::test]
    async fn source_event_replay_after_policy_revision_keeps_its_original_evaluation() {
        let (db, event_bus, task) = seeded_ready_task().await;
        let now = now_rfc3339();
        let requirement = ValidationRequirement {
            validation_run_id: "missing-validation-run".to_owned(),
            evidence_id: "missing-evidence".to_owned(),
            evidence_digest: sha256(b"missing evidence"),
            check_identity: "cargo test -p services".to_owned(),
            config_digest: sha256(b"config one"),
            workspace_id: new_uuid_v4(),
            commit_sha: "a".repeat(40),
            workspace_snapshot_digest: sha256(b"workspace snapshot"),
            required_outcome: ValidationRunStatus::Passed,
        };
        let policy = |config_digest: &str| GatePolicyDocument {
            schema_version: 1,
            scope_requirement: None,
            review: None,
            validations: vec![ValidationRequirement {
                config_digest: config_digest.to_owned(),
                ..requirement.clone()
            }],
            decisions: Vec::new(),
            work_units: Vec::new(),
        };
        let engine = GateEngine::new(Arc::clone(&db), Arc::clone(&event_bus));
        let (gate, _) = engine
            .create_gate_with_initial_policy(
                &task.id,
                "validation_gate",
                GateScopeKind::Task,
                &task.id,
                policy(&requirement.config_digest),
            )
            .await
            .expect("Gate with exact missing validation ref");
        let source_event = db::DomainEventRepo::append_event(
            &*db,
            event(
                "validation_run.completed",
                "validation_run",
                "missing-validation-run",
                &task.id,
                "test:validation-source-event",
                &now,
            ),
        )
        .await
        .expect("durable source fact event");
        assert_eq!(
            engine
                .process_domain_event(&source_event)
                .await
                .expect("first exact evaluation"),
            1
        );
        let first = GateRepo::get_gate_evaluation_for_cause(&*db, &gate.id, &source_event.id)
            .await
            .expect("source evaluation lookup")
            .expect("source event has exact E1");
        assert_eq!(first.policy_revision, 1);

        engine
            .revise_policy(&gate.id, Some(1), policy(&sha256(b"config two")))
            .await
            .expect("new immutable policy revision");
        assert_eq!(
            engine
                .process_domain_event(&source_event)
                .await
                .expect("old source replay does not reevaluate under P2"),
            0
        );
        let policy_event = db::DomainEventRepo::get_event_by_dedupe(
            &*db,
            &format!("gate.policy_revised:{}:policy:2", gate.id),
        )
        .await
        .expect("policy event lookup")
        .expect("P2 event");
        assert_eq!(
            engine
                .process_domain_event(&policy_event)
                .await
                .expect("P2 event creates its own evaluation"),
            1
        );
        let second = GateRepo::get_gate_evaluation_for_cause(&*db, &gate.id, &policy_event.id)
            .await
            .expect("P2 evaluation lookup")
            .expect("P2 event has exact E2");
        assert_eq!(second.policy_revision, 2);
        assert_ne!(first.id, second.id);
    }

    fn sha256(input: &[u8]) -> String {
        hex::encode(sha2::Sha256::digest(input))
    }

    fn event(
        event_type: &str,
        entity_type: &str,
        entity_id: &str,
        task_id: &str,
        dedupe_key: &str,
        now: &str,
    ) -> CreateDomainEvent {
        CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: event_type.to_owned(),
            entity_type: entity_type.to_owned(),
            entity_id: entity_id.to_owned(),
            actor_type: "system".to_owned(),
            actor_id: None,
            scope_type: "task".to_owned(),
            scope_id: task_id.to_owned(),
            correlation_id: entity_id.to_owned(),
            causation_id: None,
            causation_depth: 1,
            dedupe_key: Some(dedupe_key.to_owned()),
            payload_json: serde_json::json!({"task_id": task_id, "entity_id": entity_id})
                .to_string(),
            created_at: now.to_owned(),
        }
    }
}
