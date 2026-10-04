//! Durable, replay-safe retry receipts for exact domain failures.
//!
//! Retry budget is lifecycle/orchestration policy. It is deliberately kept
//! outside GatePolicy and never inferred from workflow transitions.

use std::sync::Arc;

use db::{
    new_uuid_v4, now_rfc3339, ArtifactKind, CollaborationRepo, CreateDomainEvent, DomainEvent,
    DomainEventRepo, ExecutionPurpose, ExecutionRepo, GateEvaluationOutcome, GateRepo,
    TaskIntegrationOperationKind, TaskIntegrationOperationRepo, TaskLifecycleRepo,
    TaskLifecycleState, TaskRepo, ValidationRunRepo, ValidationRunStatus,
    WorkUnitIntegrationOutcome, WorkUnitRepo,
};
use events::EventBus;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{domain_event_service::DomainEventService, task_lifecycle::*, Result, ServiceError};

const POLICY_REF: &str = "forge.task_failure_retry";
const POLICY_VERSION: i64 = 1;
const POLICY_CANONICAL: &str =
    "forge.task_failure_retry:v1:review_request_changes=3;validation_failed=2;execution_failed=3;merge_failed=1";

#[derive(Clone)]
pub struct TaskFailureRetryService {
    db: Arc<db::SqliteDb>,
    event_bus: Arc<EventBus>,
}

#[derive(Debug, Clone)]
struct FailureFact {
    kind: &'static str,
    id: String,
}

#[derive(Debug, Clone)]
struct FailureReceipt {
    task_id: String,
    kind: String,
    failure_ref: String,
    disposition: String,
    event_id: String,
}

impl TaskFailureRetryService {
    pub fn new(db: Arc<db::SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self { db, event_bus }
    }

    pub(crate) async fn has_exhausted_retry_budget(
        db: &db::SqliteDb,
        task_id: &str,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM task_failure_retry_receipt
                 WHERE task_id = ? AND disposition = 'exhausted'
             )",
        )
        .bind(task_id)
        .fetch_one(db.pool())
        .await?)
    }

    /// A durable retry receipt replaces its originating failure event as an
    /// Orchestrator wake. The receipt remains tied to its source after its
    /// lifecycle effect; only rework receipts themselves can wake work, while
    /// exhausted receipts leave the Task blocked for external direction.
    pub(crate) async fn source_event_has_retry_receipt(
        db: &db::SqliteDb,
        source_event_id: &str,
    ) -> Result<bool> {
        let policy_digest = policy_digest();
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1
                 FROM task_failure_retry_receipt receipt
                 JOIN domain_event source ON source.id = receipt.source_event_id
                 JOIN domain_event request ON request.id = receipt.receipt_event_id
                 WHERE receipt.source_event_id = ?
                   AND receipt.disposition IN ('rework', 'exhausted')
                   AND receipt.policy_ref = ?
                   AND receipt.policy_version = ?
                   AND receipt.policy_digest = ?
                   AND source.scope_type = 'task'
                   AND source.scope_id = receipt.task_id
                   AND request.event_type = CASE receipt.disposition
                       WHEN 'rework' THEN 'task.rework_requested'
                       WHEN 'exhausted' THEN 'task.retry_budget_exhausted'
                   END
                   AND request.entity_type = 'task'
                   AND request.entity_id = receipt.task_id
                   AND request.scope_type = 'task'
                   AND request.scope_id = receipt.task_id
                   AND request.causation_id = source.id
                   AND json_valid(request.payload_json)
                   AND json_extract(request.payload_json, '$.task_id') = receipt.task_id
                   AND json_extract(request.payload_json, '$.failure_kind') = receipt.failure_kind
                   AND json_extract(request.payload_json, '$.failure_ref') = receipt.failure_ref
                   AND json_extract(request.payload_json, '$.source_event_id') = source.id
                   AND json_extract(request.payload_json, '$.attempt_number') = receipt.attempt_number
                   AND json_extract(request.payload_json, '$.retry_budget') = receipt.retry_budget
                   AND json_extract(request.payload_json, '$.disposition') = receipt.disposition
                   AND json_extract(request.payload_json, '$.policy_ref') = receipt.policy_ref
                   AND json_extract(request.payload_json, '$.policy_version') = receipt.policy_version
                   AND json_extract(request.payload_json, '$.policy_digest') = receipt.policy_digest
             )",
        )
        .bind(source_event_id)
        .bind(POLICY_REF)
        .bind(POLICY_VERSION)
        .bind(policy_digest)
        .fetch_one(db.pool())
        .await?)
    }

    /// Consume failures only after their authoritative fact is durable. Event
    /// delivery is a hint; the receipt's unique exact-failure key is the
    /// authority for budget consumption and replay.
    pub async fn process_domain_event(&self, event: &DomainEvent) -> Result<usize> {
        let failures = self.failures_for_event(event).await?;
        let mut consumed = 0;
        for failure in failures {
            let receipt = self.consume(event, &failure).await?;
            self.apply_lifecycle_effect(&receipt).await?;
            consumed += 1;
        }
        Ok(consumed)
    }

    pub(crate) async fn is_rework_request_event(
        db: &db::SqliteDb,
        event: &DomainEvent,
    ) -> Result<bool> {
        if event.event_type != "task.rework_requested"
            || event.entity_type != "task"
            || event.scope_type != "task"
            || event.entity_id != event.scope_id
        {
            return Ok(false);
        }
        let payload = parse_payload(event);
        let Some(kind) = payload.get("failure_kind").and_then(Value::as_str) else {
            return Ok(false);
        };
        let Some(failure_ref) = payload.get("failure_ref").and_then(Value::as_str) else {
            return Ok(false);
        };
        let row = sqlx::query(
            "SELECT task_id, failure_kind, failure_ref, source_event_id, attempt_number, retry_budget,
                    disposition, policy_ref, policy_version, policy_digest
             FROM task_failure_retry_receipt WHERE receipt_event_id = ?",
        )
        .bind(&event.id)
        .fetch_optional(db.pool())
        .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let task_id: String = row.try_get("task_id")?;
        let stored_kind: String = row.try_get("failure_kind")?;
        let stored_ref: String = row.try_get("failure_ref")?;
        let source_event_id: String = row.try_get("source_event_id")?;
        let attempt: i64 = row.try_get("attempt_number")?;
        let budget: i64 = row.try_get("retry_budget")?;
        let disposition: String = row.try_get("disposition")?;
        let policy_ref: String = row.try_get("policy_ref")?;
        let policy_version: i64 = row.try_get("policy_version")?;
        let stored_policy_digest: String = row.try_get("policy_digest")?;
        let Some(source) = DomainEventRepo::get_event(db, &source_event_id).await? else {
            return Ok(false);
        };
        let exact_receipt = task_id == event.scope_id
            && source.scope_type == "task"
            && source.scope_id == task_id
            && event.causation_id.as_deref() == Some(source_event_id.as_str())
            && payload.get("source_event_id").and_then(Value::as_str)
                == Some(source_event_id.as_str())
            && stored_kind == kind
            && stored_ref == failure_ref
            && payload.get("attempt_number").and_then(Value::as_i64) == Some(attempt)
            && payload.get("retry_budget").and_then(Value::as_i64) == Some(budget)
            && payload.get("disposition").and_then(Value::as_str) == Some("rework")
            && disposition == "rework"
            && policy_ref == POLICY_REF
            && policy_version == POLICY_VERSION
            && payload.get("policy_ref").and_then(Value::as_str) == Some(POLICY_REF)
            && payload.get("policy_version").and_then(Value::as_i64) == Some(POLICY_VERSION)
            && stored_policy_digest == policy_digest()
            && payload.get("policy_digest").and_then(Value::as_str)
                == Some(stored_policy_digest.as_str());
        if !exact_receipt {
            return Ok(false);
        }
        let superseded: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1
                 FROM task_lifecycle_transition transition
                 JOIN domain_event transition_event
                   ON transition_event.id = transition.domain_event_id
                 JOIN domain_event retry_event
                   ON retry_event.id = ?
                 WHERE transition.task_id = ?
                   AND transition_event.sequence > retry_event.sequence
                   AND NOT (
                       transition.cause_kind = 'domain_event'
                       AND transition.cause_ref = retry_event.id
                       AND transition.reason_kind IN ('failure_rework', 'merge_failure_rework')
                       AND transition.reason_ref = ?
                   )
             )",
        )
        .bind(&event.id)
        .bind(&task_id)
        .bind(failure_ref)
        .fetch_one(db.pool())
        .await?;
        Ok(!superseded)
    }

    async fn failures_for_event(&self, event: &DomainEvent) -> Result<Vec<FailureFact>> {
        let mut failures = Vec::new();
        if event.scope_type != "task" || event.scope_id.trim().is_empty() {
            return Ok(failures);
        }
        match event.event_type.as_str() {
            "gate.evaluated" => {
                if event.entity_type != "gate_evaluation" {
                    return Ok(failures);
                }
                let Some(evaluation) =
                    GateRepo::get_gate_evaluation(&*self.db, &event.entity_id).await?
                else {
                    return Ok(failures);
                };
                let payload = parse_payload(event);
                if evaluation.id != event.entity_id
                    || evaluation.task_id != event.scope_id
                    || payload.get("evaluation_id").and_then(Value::as_str)
                        != Some(evaluation.id.as_str())
                    || payload.get("outcome").and_then(Value::as_str)
                        != Some(evaluation.outcome.to_string().as_str())
                {
                    return Ok(failures);
                }
                if evaluation.outcome == GateEvaluationOutcome::Satisfied {
                    return Ok(failures);
                }
                let gate = GateRepo::get_gate(&*self.db, &evaluation.gate_id)
                    .await?
                    .filter(|gate| gate.task_id == evaluation.task_id);
                if gate.is_none() {
                    return Ok(failures);
                }
                for input in
                    GateRepo::list_gate_evaluation_inputs(&*self.db, &evaluation.id).await?
                {
                    if input.input_kind == "review_report" && input.status == "request_changes" {
                        let Some(artifact) =
                            CollaborationRepo::get_artifact(&*self.db, &input.input_id).await?
                        else {
                            continue;
                        };
                        let valid = artifact.task_id == evaluation.task_id
                            && artifact.kind == ArtifactKind::ReviewReport
                            && artifact.digest.as_deref() == Some(input.input_digest.as_str())
                            && artifact
                                .content
                                .as_deref()
                                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                                .and_then(|report| {
                                    report
                                        .get("verdict")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned)
                                })
                                .as_deref()
                                == Some("request_changes");
                        if valid {
                            failures.push(FailureFact {
                                kind: "review_request_changes",
                                id: artifact.id,
                            });
                        }
                    } else if input.input_kind == "validation_run"
                        && matches!(input.status.as_str(), "failed" | "error" | "stale")
                    {
                        if let Some(run) =
                            ValidationRunRepo::get_validation_run(&*self.db, &input.input_id)
                                .await?
                        {
                            if run.task_id == evaluation.task_id
                                && run.status != ValidationRunStatus::Passed
                            {
                                failures.push(FailureFact {
                                    kind: "validation_failed",
                                    id: run.id,
                                });
                            }
                        }
                    }
                }
            }
            "execution.failed" => {
                if event.entity_type != "execution" {
                    return Ok(failures);
                }
                if let Some(execution) =
                    ExecutionRepo::get_by_id(&*self.db, &event.entity_id).await?
                {
                    if execution.task_id == event.scope_id
                        && execution.status == db::ExecutionStatus::Failed
                        && execution.purpose != Some(ExecutionPurpose::Orchestrate)
                        && !execution_is_executor_unavailable(&execution)
                    {
                        failures.push(FailureFact {
                            kind: "execution_failed",
                            id: execution.id,
                        });
                    }
                }
            }
            "validation_run.failed" => {
                if event.entity_type != "validation_run" {
                    return Ok(failures);
                }
                if let Some(run) =
                    ValidationRunRepo::get_validation_run(&*self.db, &event.entity_id).await?
                {
                    if run.task_id == event.scope_id
                        && matches!(
                            run.status,
                            ValidationRunStatus::Failed
                                | ValidationRunStatus::Error
                                | ValidationRunStatus::Stale
                        )
                    {
                        failures.push(FailureFact {
                            kind: "validation_failed",
                            id: run.id,
                        });
                    }
                }
            }
            "work_unit.integration_conflicted" | "work_unit.integration_failed" => {
                if event.entity_type != "work_unit_integration" {
                    return Ok(failures);
                }
                if let Some(integration) =
                    WorkUnitRepo::get_integration_by_id(&*self.db, &event.entity_id).await?
                {
                    if integration.task_id == event.scope_id
                        && matches!(
                            integration.outcome,
                            WorkUnitIntegrationOutcome::Conflict
                                | WorkUnitIntegrationOutcome::Failed
                        )
                    {
                        failures.push(FailureFact {
                            kind: "merge_failed",
                            id: integration.id,
                        });
                    }
                }
            }
            "task.lifecycle_changed" => {
                let payload = parse_payload(event);
                if event.entity_type != "task"
                    || payload.get("cause_kind").and_then(Value::as_str) != Some("merge_operation")
                {
                    return Ok(failures);
                }
                let Some(operation_id) = payload
                    .get("cause_ref")
                    .and_then(Value::as_str)
                    .or(event.causation_id.as_deref())
                else {
                    return Ok(failures);
                };
                let Some(operation) =
                    TaskIntegrationOperationRepo::get_by_id(&*self.db, operation_id).await?
                else {
                    return Ok(failures);
                };
                let failed_status = matches!(
                    payload.get("task_merge_status").and_then(Value::as_str),
                    Some("conflict" | "failed" | "abandoned")
                );
                if operation.task_id == event.scope_id
                    && operation.kind == TaskIntegrationOperationKind::TaskMerge
                    && matches!(
                        operation.status,
                        db::TaskIntegrationOperationStatus::Conflict
                            | db::TaskIntegrationOperationStatus::Failed
                            | db::TaskIntegrationOperationStatus::Abandoned
                    )
                    && failed_status
                {
                    failures.push(FailureFact {
                        kind: "merge_failed",
                        id: operation.id,
                    });
                }
            }
            _ => {}
        }
        Ok(failures)
    }

    async fn consume(&self, source: &DomainEvent, failure: &FailureFact) -> Result<FailureReceipt> {
        let budget = retry_budget(failure.kind);
        let digest = policy_digest();
        let mut transaction = self.db.pool().begin().await?;
        if let Some(receipt) = load_receipt(
            &mut transaction,
            &source.scope_id,
            failure.kind,
            &failure.id,
        )
        .await?
        {
            transaction.commit().await?;
            return Ok(receipt);
        }
        let consumed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_failure_retry_receipt
             WHERE task_id = ? AND failure_kind = ?",
        )
        .bind(&source.scope_id)
        .bind(failure.kind)
        .fetch_one(&mut *transaction)
        .await?;
        let attempt = consumed + 1;
        let disposition = if attempt <= budget {
            "rework"
        } else {
            "exhausted"
        };
        let now = now_rfc3339();
        let event_type = if disposition == "rework" {
            "task.rework_requested"
        } else {
            "task.retry_budget_exhausted"
        };
        let dedupe_key = format!(
            "task-failure-retry:{}:{}:{}",
            source.scope_id, failure.kind, failure.id
        );
        let event_input = CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: event_type.to_owned(),
            entity_type: "task".to_owned(),
            entity_id: source.scope_id.clone(),
            actor_type: "system".to_owned(),
            actor_id: None,
            scope_type: "task".to_owned(),
            scope_id: source.scope_id.clone(),
            correlation_id: dedupe_key.clone(),
            causation_id: Some(source.id.clone()),
            causation_depth: source.causation_depth.saturating_add(1).min(16),
            dedupe_key: Some(dedupe_key),
            payload_json: json!({
                "task_id": source.scope_id,
                "failure_kind": failure.kind,
                "failure_ref": failure.id,
                "source_event_id": source.id,
                "attempt_number": attempt,
                "retry_budget": budget,
                "disposition": disposition,
                "policy_ref": POLICY_REF,
                "policy_version": POLICY_VERSION,
                "policy_digest": digest,
            })
            .to_string(),
            created_at: now.clone(),
        };
        let event =
            DomainEventRepo::append_event_in_tx(&*self.db, &mut transaction, &event_input).await?;
        sqlx::query(
            "INSERT INTO task_failure_retry_receipt (
                id, task_id, failure_kind, failure_ref, source_event_id,
                attempt_number, retry_budget, disposition, policy_ref,
                policy_version, policy_digest, receipt_event_id, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(new_uuid_v4())
        .bind(&source.scope_id)
        .bind(failure.kind)
        .bind(&failure.id)
        .bind(&source.id)
        .bind(attempt)
        .bind(budget)
        .bind(disposition)
        .bind(POLICY_REF)
        .bind(POLICY_VERSION)
        .bind(&digest)
        .bind(&event.id)
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        DomainEventService::publish_committed_hint(&self.event_bus, &event);
        Ok(FailureReceipt {
            task_id: source.scope_id.clone(),
            kind: failure.kind.to_owned(),
            failure_ref: failure.id.clone(),
            disposition: disposition.to_owned(),
            event_id: event.id,
        })
    }

    async fn apply_lifecycle_effect(&self, receipt: &FailureReceipt) -> Result<()> {
        let idempotency_key = format!(
            "task-failure-lifecycle:{}:{}:{}",
            receipt.task_id, receipt.kind, receipt.failure_ref
        );
        if TaskLifecycleRepo::has_task_lifecycle_transition(
            &*self.db,
            &receipt.task_id,
            &idempotency_key,
        )
        .await?
        {
            return Ok(());
        }
        if receipt.disposition == "rework" {
            let Some(event) = DomainEventRepo::get_event(&*self.db, &receipt.event_id).await?
            else {
                return Ok(());
            };
            if !Self::is_rework_request_event(&self.db, &event).await? {
                // A lifecycle transition after this receipt supersedes its
                // rework direction. Replaying an old failure must not reopen
                // work after a later readiness or user decision.
                return Ok(());
            }
        }
        let task = TaskRepo::get_by_id(&*self.db, &receipt.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", receipt.task_id.clone()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if matches!(
            lifecycle.state,
            TaskLifecycleState::Done | TaskLifecycleState::Cancelled
        ) {
            return Ok(());
        }
        if receipt.disposition == "rework"
            && Self::has_exhausted_retry_budget(&self.db, &task.id).await?
        {
            return Ok(());
        }

        let (target, reason_kind) = match receipt.disposition.as_str() {
            "rework" if lifecycle.state == TaskLifecycleState::Active => return Ok(()),
            "rework"
                if lifecycle.state == TaskLifecycleState::Blocked
                    && receipt.kind == "merge_failed"
                    && lifecycle.reason_kind.as_deref() == Some("task_merge_failed")
                    && lifecycle.reason_ref.as_deref() == Some(receipt.failure_ref.as_str()) =>
            {
                // A failed TaskMerge atomically moves Merging -> Blocked before
                // this retry receipt is consumed. Reopen only when that exact
                // operation is the current block cause; unrelated blocks such
                // as a closed PR remain blocked.
                (TaskLifecycleState::Active, "merge_failure_rework")
            }
            "rework"
                if matches!(
                    lifecycle.state,
                    TaskLifecycleState::Ready | TaskLifecycleState::ReadyToMerge
                ) =>
            {
                (TaskLifecycleState::Active, "failure_rework")
            }
            "rework" => return Ok(()),
            "exhausted" if lifecycle.state == TaskLifecycleState::Blocked => return Ok(()),
            "exhausted"
                if matches!(
                    lifecycle.state,
                    TaskLifecycleState::Ready
                        | TaskLifecycleState::Active
                        | TaskLifecycleState::ReadyToMerge
                ) =>
            {
                (TaskLifecycleState::Blocked, "retry_budget_exhausted")
            }
            "exhausted" => return Ok(()),
            _ => {
                return Err(ServiceError::invalid_operation(
                    "unknown retry receipt disposition",
                ))
            }
        };
        TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .transition(TransitionLifecycleInput {
                task_id: task.id,
                expected_task_version: task.version,
                to_state: target,
                cause: LifecycleCause::DomainEvent(receipt.event_id.clone()),
                reason_kind: Some(reason_kind.to_owned()),
                reason_ref: Some(receipt.failure_ref.clone()),
                idempotency_key,
            })
            .await?;
        Ok(())
    }
}

async fn load_receipt(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
    kind: &str,
    failure_ref: &str,
) -> Result<Option<FailureReceipt>> {
    let row = sqlx::query(
        "SELECT task_id, failure_kind, failure_ref, attempt_number, retry_budget,
                disposition, policy_digest, receipt_event_id
         FROM task_failure_retry_receipt
         WHERE task_id = ? AND failure_kind = ? AND failure_ref = ?",
    )
    .bind(task_id)
    .bind(kind)
    .bind(failure_ref)
    .fetch_optional(&mut **transaction)
    .await?;
    row.map(|row| {
        Ok(FailureReceipt {
            task_id: row.try_get("task_id")?,
            kind: row.try_get("failure_kind")?,
            failure_ref: row.try_get("failure_ref")?,
            disposition: row.try_get("disposition")?,
            event_id: row.try_get("receipt_event_id")?,
        })
    })
    .transpose()
}

fn retry_budget(kind: &str) -> i64 {
    match kind {
        "review_request_changes" => 3,
        "validation_failed" => 2,
        "execution_failed" => 3,
        "merge_failed" => 1,
        _ => 0,
    }
}

fn execution_is_executor_unavailable(execution: &db::Execution) -> bool {
    execution
        .executor_config_snapshot_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|snapshot| {
            snapshot
                .get(executors::ROUTING_SNAPSHOT_KEY)
                .and_then(|routing| routing.get("disposition"))
                .and_then(|disposition| disposition.get("failure_class"))
                .and_then(Value::as_str)
                .map(|failure_class| failure_class == "executor_unavailable")
        })
        .unwrap_or(false)
}

fn policy_digest() -> String {
    hex::encode(Sha256::digest(POLICY_CANONICAL.as_bytes()))
}

fn parse_payload(event: &DomainEvent) -> Value {
    serde_json::from_str(&event.payload_json).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{
        CreateExecution, CreateProject, CreateRoleMembership, CreateTask, CreateTaskRole,
        ExecutionStatus, ProjectRepo, RoleMembershipRepo, RoleMembershipStatus, TaskLifecycleRepo,
        TaskRoleRepo, UserRepo,
    };

    #[tokio::test]
    async fn exact_request_changes_report_creates_one_rework_receipt() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool");
        db::run_migrations(&pool).await.expect("migrations");
        let db = Arc::new(db::SqliteDb::new(pool));
        let event_bus = Arc::new(EventBus::new(32));
        let now = now_rfc3339();
        let project_id = new_uuid_v4();
        let task_id = new_uuid_v4();
        let user_id = new_uuid_v4();
        let role_id = new_uuid_v4();
        let membership_id = new_uuid_v4();
        let execution_id = new_uuid_v4();
        ProjectRepo::create(
            &*db,
            CreateProject {
                id: project_id.clone(),
                name: "Review rework receipt test".to_owned(),
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
        UserRepo::create_user(
            &*db,
            &db::User {
                id: user_id.clone(),
                email: "review-rework@example.invalid".to_owned(),
                password_hash: "unused".to_owned(),
                display_name: None,
                is_admin: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Human reviewer");
        TaskRepo::create(
            &*db,
            CreateTask {
                id: task_id.clone(),
                project_id: project_id.clone(),
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "Review rework Task".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "in_progress".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Task");
        TaskRoleRepo::create(
            &*db,
            CreateTaskRole {
                id: role_id.clone(),
                task_id: task_id.clone(),
                role: "reviewer".to_owned(),
                coordination_mode: None,
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("reviewer TaskRole");
        RoleMembershipRepo::add(
            &*db,
            CreateRoleMembership {
                id: membership_id,
                task_role_id: role_id,
                actor_kind: db::ActorKind::Human,
                actor_id: user_id.clone(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("active Human reviewer membership");
        let actor = db::ActorRef::Human(user_id.clone());
        ExecutionRepo::create(
            &*db,
            CreateExecution {
                id: execution_id.clone(),
                task_id: task_id.clone(),
                agent_id: None,
                actor_ref: Some(actor.clone()),
                purpose: Some(ExecutionPurpose::Review),
                role: "reviewer".to_owned(),
                status: ExecutionStatus::Running,
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
                error: None,
                executor_config_snapshot_json: None,
                workspace_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("exact Human Review Execution");
        let report_id = new_uuid_v4();
        let report_content = json!({
            "kind": "review_report",
            "verdict": "request_changes",
            "summary": "One concrete change is required.",
            "criteria": ["required contract"],
            "findings": [{"title":"Fix contract", "severity":"major", "body":"Update contract."}],
            "questions": [],
            "evidence_considered": [],
            "subject": {
                "task_id": task_id,
                "review_execution_id": execution_id,
                "workspace_id": null,
                "base_commit_sha": null,
                "head_commit_sha": null,
                "workspace_snapshot_digest": null,
            }
        })
        .to_string();
        let report_digest = hex::encode(Sha256::digest(report_content.as_bytes()));
        db::CollaborationRepo::create_execution_artifact_output(
            &*db,
            db::CreateArtifact {
                id: report_id.clone(),
                task_id: task_id.clone(),
                kind: ArtifactKind::ReviewReport,
                storage_kind: db::ArtifactStorageKind::Inline,
                content: Some(report_content),
                content_ref: None,
                metadata_json: json!({"schema_version":1}).to_string(),
                digest: Some(report_digest.clone()),
                producer_execution_id: execution_id.clone(),
                created_at: now.clone(),
            },
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "artifact.created".to_owned(),
                entity_type: "artifact".to_owned(),
                entity_id: report_id.clone(),
                actor_type: "human".to_owned(),
                actor_id: Some(user_id),
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: execution_id.clone(),
                causation_id: Some(execution_id.clone()),
                causation_depth: 1,
                dedupe_key: Some(format!("review-report:{execution_id}")),
                payload_json: json!({"kind":"review_report","execution_id":execution_id})
                    .to_string(),
                created_at: now.clone(),
            },
        )
        .await
        .expect("exact ReviewReport Artifact");
        ExecutionRepo::update(
            &*db,
            db::UpdateExecution {
                id: execution_id,
                status: Some(ExecutionStatus::Completed),
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
                error: None,
                executor_config_snapshot_json: None,
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Review Execution completes");

        let gate_engine =
            crate::gate_engine::GateEngine::new(Arc::clone(&db), Arc::clone(&event_bus));
        let (gate, _) = gate_engine
            .create_gate_with_initial_policy(
                &task_id,
                "review_acceptance",
                db::GateScopeKind::Task,
                &task_id,
                crate::gate_engine::GatePolicyDocument {
                    schema_version: 1,
                    scope_requirement: None,
                    review: Some(crate::gate_engine::ReviewSetPolicy {
                        mode: crate::gate_engine::ReviewSelectionMode::OneAcceptable,
                        required_count: 1,
                        human_required: false,
                        allow_humans: true,
                        allow_agents: true,
                        allowed_actor_refs: Vec::new(),
                        task_role_snapshot: None,
                        candidates: vec![crate::gate_engine::ReviewReportRequirement {
                            artifact_id: report_id,
                            digest: report_digest,
                            expected_actor: Some(actor),
                            required: true,
                            subject: crate::gate_engine::ReviewSubjectExpectation {
                                workspace_id: None,
                                base_commit_sha: None,
                                head_commit_sha: None,
                                workspace_snapshot_digest: None,
                            },
                        }],
                    }),
                    validations: Vec::new(),
                    decisions: Vec::new(),
                    work_units: Vec::new(),
                },
            )
            .await
            .expect("Gate with exact ReviewReport policy");
        let evaluation = gate_engine
            .evaluate_active(&gate.id)
            .await
            .expect("request_changes is a deterministic unsatisfied evaluation");
        assert_eq!(
            evaluation.evaluation.outcome,
            GateEvaluationOutcome::Unsatisfied
        );
        assert!(evaluation
            .inputs
            .iter()
            .any(|input| input.input_kind == "review_report" && input.status == "request_changes"));
        let evaluation_event = evaluation.event.expect("durable GateEvaluation event");
        let service = TaskFailureRetryService::new(Arc::clone(&db), Arc::clone(&event_bus));

        assert_eq!(
            service
                .process_domain_event(&evaluation_event)
                .await
                .expect("exact review failure consumes retry policy"),
            1
        );
        assert_eq!(
            service
                .process_domain_event(&evaluation_event)
                .await
                .expect("replay reuses its exact receipt"),
            1
        );
        let report_id = evaluation
            .inputs
            .iter()
            .find(|input| input.input_kind == "review_report")
            .expect("ReviewReport input")
            .input_id
            .clone();
        let receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_failure_retry_receipt
             WHERE task_id = ? AND failure_kind = 'review_request_changes'
               AND failure_ref = ? AND disposition = 'rework'",
        )
        .bind(&task_id)
        .bind(&report_id)
        .fetch_one(db.pool())
        .await
        .expect("exact ReviewReport receipt count");
        assert_eq!(receipts, 1, "replay cannot consume a second review retry");

        let current_task = TaskRepo::get_by_id(&*db, &task_id, false)
            .await
            .expect("Task after first failure receipt")
            .expect("Task exists");
        crate::task_lifecycle::TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .transition(crate::task_lifecycle::TransitionLifecycleInput {
                task_id: task_id.clone(),
                expected_task_version: current_task.version,
                to_state: db::TaskLifecycleState::Ready,
                cause: crate::task_lifecycle::LifecycleCause::System(
                    api_types::SystemComponent::General,
                ),
                reason_kind: Some("later_user_decision".to_owned()),
                reason_ref: Some("later-transition".to_owned()),
                idempotency_key: "later-transition-after-rework".to_owned(),
            })
            .await
            .expect("later lifecycle decision supersedes the old failure receipt");

        assert_eq!(
            service
                .process_domain_event(&evaluation_event)
                .await
                .expect("old failure replay does not reapply its lifecycle direction"),
            1
        );
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task_id)
            .await
            .expect("lifecycle lookup after stale replay")
            .expect("Task lifecycle");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Ready);
    }

    #[tokio::test]
    async fn exact_execution_failure_is_consumed_once_and_exhaustion_blocks_lifecycle() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool");
        db::run_migrations(&pool).await.expect("migrations");
        let db = Arc::new(db::SqliteDb::new(pool));
        let event_bus = Arc::new(EventBus::new(32));
        let now = now_rfc3339();
        let project_id = new_uuid_v4();
        let task_id = new_uuid_v4();
        ProjectRepo::create(
            &*db,
            CreateProject {
                id: project_id.clone(),
                name: "Retry receipt test".to_owned(),
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
        TaskRepo::create(
            &*db,
            CreateTask {
                id: task_id.clone(),
                project_id: project_id.clone(),
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "Retry receipt Task".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "in_progress".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Task");

        let service = TaskFailureRetryService::new(Arc::clone(&db), Arc::clone(&event_bus));
        let unavailable_execution_id = new_uuid_v4();
        let (_, unavailable_event) = ExecutionRepo::create_with_event(
            &*db,
            CreateExecution {
                id: unavailable_execution_id.clone(),
                task_id: task_id.clone(),
                agent_id: None,
                actor_ref: None,
                purpose: Some(ExecutionPurpose::Implement),
                harness_session_id: None,
                role: "implementer".to_owned(),
                status: ExecutionStatus::Failed,
                stop_reason: Some(db::StopReason::ExecutorFailed),
                stopped_by: None,
                resume_policy: Some(db::ResumePolicy::Manual),
                stopped_at: Some(now_rfc3339()),
                parent_execution_id: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: Some("no executor route is available".to_owned()),
                executor_config_snapshot_json: Some(
                    json!({
                        "routing": {
                            "disposition": {
                                "failure_class": "executor_unavailable",
                                "retry_at": null
                            }
                        }
                    })
                    .to_string(),
                ),
                workspace_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "execution.failed".to_owned(),
                entity_type: "execution".to_owned(),
                entity_id: unavailable_execution_id,
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: new_uuid_v4(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("test-execution-unavailable:{task_id}")),
                payload_json: json!({"task_id": task_id}).to_string(),
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("executor-unavailable Execution and event");
        assert_eq!(
            service
                .process_domain_event(&unavailable_event)
                .await
                .expect("unavailable route is not retry-budget failure"),
            0
        );
        let unavailable_receipts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_failure_retry_receipt WHERE task_id = ?")
                .bind(&task_id)
                .fetch_one(db.pool())
                .await
                .expect("retry receipt count loads");
        assert_eq!(unavailable_receipts, 0);

        let mut source_events = Vec::new();
        for index in 0..4 {
            let execution_id = new_uuid_v4();
            let created_at = now_rfc3339();
            let (execution, event) = ExecutionRepo::create_with_event(
                &*db,
                CreateExecution {
                    id: execution_id.clone(),
                    task_id: task_id.clone(),
                    agent_id: None,
                    actor_ref: None,
                    purpose: Some(ExecutionPurpose::Implement),
                    harness_session_id: None,
                    role: "implementer".to_owned(),
                    status: ExecutionStatus::Failed,
                    stop_reason: None,
                    stopped_by: None,
                    resume_policy: None,
                    stopped_at: None,
                    parent_execution_id: None,
                    agent_session_id: None,
                    agent_message_id: None,
                    last_activity_at: None,
                    summary: None,
                    logs_path: None,
                    before_sha: None,
                    after_sha: None,
                    error: Some(format!("failure {index}")),
                    executor_config_snapshot_json: None,
                    workspace_id: None,
                    created_at: created_at.clone(),
                    updated_at: created_at,
                },
                CreateDomainEvent {
                    id: new_uuid_v4(),
                    event_type: "execution.failed".to_owned(),
                    entity_type: "execution".to_owned(),
                    entity_id: execution_id,
                    actor_type: "system".to_owned(),
                    actor_id: None,
                    scope_type: "task".to_owned(),
                    scope_id: task_id.clone(),
                    correlation_id: new_uuid_v4(),
                    causation_id: None,
                    causation_depth: 0,
                    dedupe_key: Some(format!("test-execution-failed:{task_id}:{index}")),
                    payload_json: json!({"task_id": task_id, "failure_index": index}).to_string(),
                    created_at: now_rfc3339(),
                },
            )
            .await
            .expect("failed Execution and source event");
            assert_eq!(execution.status, ExecutionStatus::Failed);
            source_events.push(event);
        }

        for event in &source_events[..3] {
            assert_eq!(
                service.process_domain_event(event).await.expect("consume"),
                1
            );
            assert_eq!(
                service.process_domain_event(event).await.expect("replay"),
                1
            );
        }
        let rework_event = DomainEventRepo::get_event_by_dedupe(
            &*db,
            &format!(
                "task-failure-retry:{task_id}:execution_failed:{}",
                source_events[0].entity_id
            ),
        )
        .await
        .expect("event lookup")
        .expect("rework event");
        assert!(
            TaskFailureRetryService::is_rework_request_event(&db, &rework_event)
                .await
                .expect("exact rework receipt")
        );
        let mut mismatched_cause = rework_event.clone();
        mismatched_cause.causation_id = None;
        assert!(
            !TaskFailureRetryService::is_rework_request_event(&db, &mismatched_cause)
                .await
                .expect("mismatched retry cause is rejected")
        );
        let mut mismatched_source_payload = rework_event.clone();
        let mut payload = parse_payload(&rework_event);
        payload["source_event_id"] = Value::String("different-source".to_owned());
        mismatched_source_payload.payload_json = payload.to_string();
        assert!(
            !TaskFailureRetryService::is_rework_request_event(&db, &mismatched_source_payload)
                .await
                .expect("mismatched retry source payload is rejected")
        );
        assert!(
            TaskFailureRetryService::source_event_has_retry_receipt(&db, &source_events[0].id)
                .await
                .expect("rework source receipt lookup")
        );

        let exhausted = &source_events[3];
        assert_eq!(
            service
                .process_domain_event(exhausted)
                .await
                .expect("exhaust"),
            1
        );
        assert_eq!(
            service
                .process_domain_event(exhausted)
                .await
                .expect("replay"),
            1
        );
        assert!(
            TaskFailureRetryService::source_event_has_retry_receipt(&db, &exhausted.id)
                .await
                .expect("exhaustion source receipt lookup")
        );
        let receipt_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_failure_retry_receipt WHERE task_id = ?")
                .bind(&task_id)
                .fetch_one(db.pool())
                .await
                .expect("retry receipts");
        assert_eq!(
            receipt_count, 4,
            "failure replay does not consume a second receipt"
        );
        let attempts: Vec<i64> = sqlx::query_scalar(
            "SELECT attempt_number FROM task_failure_retry_receipt
             WHERE task_id = ? ORDER BY attempt_number",
        )
        .bind(&task_id)
        .fetch_all(db.pool())
        .await
        .expect("retry attempt order");
        assert_eq!(attempts, vec![1, 2, 3, 4]);
        assert_eq!(
            TaskLifecycleRepo::get_task_lifecycle(&*db, &task_id)
                .await
                .expect("lifecycle")
                .expect("Task lifecycle")
                .state,
            TaskLifecycleState::Blocked
        );
        assert!(
            TaskFailureRetryService::has_exhausted_retry_budget(&db, &task_id)
                .await
                .expect("retry budget lookup")
        );
        let task = TaskRepo::get_by_id(&*db, &task_id, false)
            .await
            .expect("Task lookup")
            .expect("Task exists");
        let reopen = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
        )
        .transition(crate::task_lifecycle::TransitionLifecycleInput {
            task_id: task_id.clone(),
            expected_task_version: task.version,
            to_state: TaskLifecycleState::Ready,
            cause: crate::task_lifecycle::LifecycleCause::Actor(api_types::Actor::user(
                api_types::UserActionSource::Test,
            )),
            reason_kind: Some("test_unblock".to_owned()),
            reason_ref: Some("exercise retry guard".to_owned()),
            idempotency_key: "retry-guard:test-unblock".to_owned(),
        })
        .await;
        assert!(reopen
            .expect_err("exhausted retry receipts fence runnable lifecycle states")
            .to_string()
            .contains("retry budget is exhausted"));
        let blocked_lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task_id)
            .await
            .expect("blocked lifecycle after rejected re-entry")
            .expect("Task lifecycle");
        let guarded_transition = db::TransitionTaskLifecycle {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            expected_task_version: task.version,
            expected_lifecycle_version: blocked_lifecycle.version,
            expected_state: blocked_lifecycle.state,
            to_state: TaskLifecycleState::Ready,
            cause_kind: "actor".to_owned(),
            cause_ref: Some("human:retry-guard-user".to_owned()),
            reason_kind: Some("direct_repository_reentry".to_owned()),
            reason_ref: Some("check V103 guard".to_owned()),
            gate_evaluation_id: None,
            idempotency_key: "retry-guard:direct-repository-reentry".to_owned(),
            updated_at: now_rfc3339(),
            event: CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.lifecycle_changed".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: task_id.clone(),
                actor_type: "human".to_owned(),
                actor_id: Some("retry-guard-user".to_owned()),
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: "retry-guard:direct-repository-reentry".to_owned(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some("retry-guard:direct-repository-reentry".to_owned()),
                payload_json: json!({"task_id":task_id,"from_state":"blocked","to_state":"ready"})
                    .to_string(),
                created_at: now_rfc3339(),
            },
        };
        assert!(
            TaskLifecycleRepo::transition_task_lifecycle(&*db, guarded_transition)
                .await
                .is_err(),
            "V103 must reject direct repository writes after exhaustion"
        );
        service
            .process_domain_event(exhausted)
            .await
            .expect("replaying an applied exhaustion effect is a no-op");
        assert_eq!(
            TaskLifecycleRepo::get_task_lifecycle(&*db, &task_id)
                .await
                .expect("lifecycle")
                .expect("Task lifecycle")
                .state,
            TaskLifecycleState::Blocked,
            "replaying the same failure must not apply its lifecycle effect twice"
        );

        let blocked_task_id = new_uuid_v4();
        TaskRepo::create(
            &*db,
            CreateTask {
                id: blocked_task_id.clone(),
                project_id: project_id.clone(),
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "Independently blocked Task".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "in_progress".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("second Task");
        let blocked_task = TaskRepo::get_by_id(&*db, &blocked_task_id, false)
            .await
            .expect("second Task lookup")
            .expect("second Task exists");
        crate::task_lifecycle::TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .transition(crate::task_lifecycle::TransitionLifecycleInput {
                task_id: blocked_task_id.clone(),
                expected_task_version: blocked_task.version,
                to_state: TaskLifecycleState::Blocked,
                cause: crate::task_lifecycle::LifecycleCause::Actor(api_types::Actor::user(
                    api_types::UserActionSource::Test,
                )),
                reason_kind: Some("pr_closed_without_merge".to_owned()),
                reason_ref: Some("pr-metadata-closed".to_owned()),
                idempotency_key: "test:closed-pr-block".to_owned(),
            })
            .await
            .expect("independent block reason");
        let blocked_execution_id = new_uuid_v4();
        let (_, blocked_failure_event) = ExecutionRepo::create_with_event(
            &*db,
            CreateExecution {
                id: blocked_execution_id.clone(),
                task_id: blocked_task_id.clone(),
                agent_id: None,
                actor_ref: None,
                purpose: Some(ExecutionPurpose::Implement),
                harness_session_id: None,
                role: "implementer".to_owned(),
                status: ExecutionStatus::Failed,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: Some("failure while blocked".to_owned()),
                executor_config_snapshot_json: None,
                workspace_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "execution.failed".to_owned(),
                entity_type: "execution".to_owned(),
                entity_id: blocked_execution_id,
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: blocked_task_id.clone(),
                correlation_id: new_uuid_v4(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("test-execution-failed:{blocked_task_id}")),
                payload_json: json!({"task_id": blocked_task_id}).to_string(),
                created_at: now,
            },
        )
        .await
        .expect("failed Execution and source event");
        assert_eq!(
            service
                .process_domain_event(&blocked_failure_event)
                .await
                .expect("record separate rework receipt"),
            1
        );
        let blocked = TaskLifecycleRepo::get_task_lifecycle(&*db, &blocked_task_id)
            .await
            .expect("blocked lifecycle")
            .expect("blocked Task lifecycle");
        assert_eq!(blocked.state, TaskLifecycleState::Blocked);
        assert_eq!(
            blocked.reason_kind.as_deref(),
            Some("pr_closed_without_merge")
        );

        let merge_retry_task_id = new_uuid_v4();
        TaskRepo::create(
            &*db,
            CreateTask {
                id: merge_retry_task_id.clone(),
                project_id: project_id.clone(),
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "Merge failure rework Task".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "in_progress".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("merge retry Task");
        let failed_merge_id = new_uuid_v4();
        let merge_task = TaskRepo::get_by_id(&*db, &merge_retry_task_id, false)
            .await
            .expect("merge retry task lookup")
            .expect("merge retry task");
        let lifecycle_service = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
        );
        lifecycle_service
            .transition(crate::task_lifecycle::TransitionLifecycleInput {
                task_id: merge_retry_task_id.clone(),
                expected_task_version: merge_task.version,
                to_state: TaskLifecycleState::Blocked,
                cause: crate::task_lifecycle::LifecycleCause::System(
                    api_types::SystemComponent::General,
                ),
                reason_kind: Some("task_merge_failed".to_owned()),
                reason_ref: Some(failed_merge_id.clone()),
                idempotency_key: "test:merge-failure-block".to_owned(),
            })
            .await
            .expect("exact failed merge blocks its Task");
        let now = now_rfc3339();
        let merge_failure_event = DomainEventRepo::append_event(
            &*db,
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task_merge.failed".to_owned(),
                entity_type: "task_integration_operation".to_owned(),
                entity_id: failed_merge_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: merge_retry_task_id.clone(),
                correlation_id: "test:merge-failure-source".to_owned(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some("test:merge-failure-source".to_owned()),
                payload_json: json!({"task_id":merge_retry_task_id,"operation_id":failed_merge_id})
                    .to_string(),
                created_at: now,
            },
        )
        .await
        .expect("durable merge failure source event");
        let receipt = service
            .consume(
                &merge_failure_event,
                &FailureFact {
                    kind: "merge_failed",
                    id: failed_merge_id.clone(),
                },
            )
            .await
            .expect("exact merge failure retry receipt");
        service
            .apply_lifecycle_effect(&receipt)
            .await
            .expect("exact failed merge reopens for rework");
        let merge_retry_lifecycle =
            TaskLifecycleRepo::get_task_lifecycle(&*db, &merge_retry_task_id)
                .await
                .expect("merge retry lifecycle lookup")
                .expect("merge retry lifecycle");
        assert_eq!(merge_retry_lifecycle.state, TaskLifecycleState::Active);
        assert_eq!(
            merge_retry_lifecycle.reason_kind.as_deref(),
            Some("merge_failure_rework")
        );
        assert_eq!(
            merge_retry_lifecycle.reason_ref.as_deref(),
            Some(failed_merge_id.as_str())
        );

        let task_service =
            crate::task_service::TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
        let claim = task_service
            .claim_task(
                task_id,
                crate::Assignee::User("retry-guard-user".to_owned()),
                None,
            )
            .await;
        match claim {
            Err(_) => {}
            Ok(_) => panic!("exhausted retry receipt must block another execution dispatch"),
        }
    }
}
