use std::sync::Arc;

use api_types::ActorRef;
use db::{
    new_uuid_v4, now_rfc3339, ArtifactKind, CollaborationRepo, CreateDomainEvent, DbError,
    DomainEventRepo, ExecutionRepo, SqliteDb, TaskLifecycleRepo, TaskLifecycleState, WorkUnit,
    WorkUnitRepo, WorkUnitStatus,
};
use events::EventBus;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    collaboration_service::{CollaborationActorSource, CollaborationService},
    Result, ServiceError,
};

/// Bounded correction rounds per Task. The budget is consumed atomically by
/// the rework receipt; replaying the same idempotency key consumes nothing.
pub const MAX_REWORK_ROUNDS: i64 = 8;

#[derive(Debug, Clone, Serialize)]
pub struct RequestReworkInput {
    pub task_id: String,
    pub expected_task_version: i64,
    pub expected_lifecycle_version: i64,
    pub review_report_artifact_id: String,
    pub reviewed_work_unit_id: String,
    pub reviewed_work_unit_version: i64,
    pub correction_title: String,
    pub correction_scope: String,
    pub correction_role: String,
    pub correction_actor: Option<ActorRef>,
    pub review_title: String,
    pub review_scope: String,
    pub review_actor: Option<ActorRef>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitReviewSubject {
    pub id: String,
    pub review_work_unit_id: String,
    pub task_id: String,
    pub reviewed_work_unit_id: String,
    pub reviewed_work_unit_version: Option<i64>,
    pub result_execution_id: Option<String>,
    pub result_artifact_id: Option<String>,
    pub result_commit_sha: Option<String>,
    pub result_workspace_id: Option<String>,
    pub workspace_snapshot_digest: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct RequestReworkResult {
    pub correction_work_unit: WorkUnit,
    pub review_work_unit: WorkUnit,
    pub review_subject: Option<WorkUnitReviewSubject>,
    pub rounds_remaining: i64,
    pub replayed: bool,
}

#[derive(Clone)]
pub struct TaskReworkService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
}

impl TaskReworkService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self { db, event_bus }
    }

    /// Create or replay one correction + review round. The correction consumes
    /// the reviewed result, and the new review depends on the correction.
    pub async fn request_rework(
        &self,
        source: CollaborationActorSource,
        input: RequestReworkInput,
    ) -> Result<RequestReworkResult> {
        validate_rework_input(&input)?;
        let collaboration =
            CollaborationService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let (task, actor) = collaboration
            .authorize_mutation_source(&input.task_id, &source)
            .await?;
        let request_digest = rework_request_digest(&input)?;

        if let Some(replayed) = self
            .replay_if_receipt_matches(&input, &request_digest)
            .await?
        {
            return Ok(replayed);
        }

        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if task.version != input.expected_task_version {
            return Err(DbError::TaskVersionConflict {
                expected: input.expected_task_version,
                actual: task.version,
            }
            .into());
        }
        if lifecycle.version != input.expected_lifecycle_version {
            return Err(DbError::VersionConflict.into());
        }
        if !matches!(
            lifecycle.state,
            TaskLifecycleState::Ready
                | TaskLifecycleState::Active
                | TaskLifecycleState::Blocked
                | TaskLifecycleState::ReadyToMerge
        ) {
            return Err(ServiceError::invalid_operation(format!(
                "rework is not available for TaskLifecycle {}",
                lifecycle.state
            )));
        }

        let reviewed = WorkUnitRepo::get_by_id(&*self.db, &input.reviewed_work_unit_id)
            .await?
            .filter(|unit| unit.task_id == task.id)
            .ok_or_else(|| {
                ServiceError::not_found("work_unit", input.reviewed_work_unit_id.clone())
            })?;
        if reviewed.version != input.reviewed_work_unit_version
            && !(reviewed.status == WorkUnitStatus::Completed
                && reviewed.version == input.reviewed_work_unit_version + 1)
        {
            return Err(DbError::VersionConflict.into());
        }
        let reviewed_version = input.reviewed_work_unit_version;
        let review_report =
            CollaborationRepo::get_artifact(&*self.db, &input.review_report_artifact_id)
                .await?
                .filter(|artifact| {
                    artifact.task_id == task.id
                        && artifact.kind == ArtifactKind::ReviewReport
                        && artifact.content.is_some()
                })
                .ok_or_else(|| {
                    ServiceError::not_found(
                        "review_report_artifact",
                        input.review_report_artifact_id.clone(),
                    )
                })?;
        let report: serde_json::Value = serde_json::from_str(
            review_report.content.as_deref().unwrap_or_default(),
        )
        .map_err(|error| {
            ServiceError::invalid_operation(format!(
                "ReviewReport content is not valid JSON: {error}"
            ))
        })?;
        if report.get("verdict").and_then(serde_json::Value::as_str) != Some("request_changes") {
            return Err(ServiceError::invalid_operation(
                "rework requires a ReviewReport with verdict request_changes",
            ));
        }
        let report_subject = report
            .get("subject")
            .ok_or_else(|| ServiceError::invalid_operation("ReviewReport has no exact subject"))?;
        let report_work_unit_id = report_subject
            .get("reviewed_work_unit_id")
            .and_then(serde_json::Value::as_str);
        let report_work_unit_version = report_subject
            .get("reviewed_work_unit_version")
            .and_then(serde_json::Value::as_i64);
        if report_work_unit_id != Some(reviewed.id.as_str())
            || report_work_unit_version != Some(reviewed_version)
        {
            return Err(ServiceError::invalid_operation(
                "ReviewReport subject does not match the requested WorkUnit and version",
            ));
        }
        let (producer_execution_id, _producer_actor) =
            review_report.execution_producer().ok_or_else(|| {
                ServiceError::invalid_operation(
                    "rework requires an Execution-produced ReviewReport",
                )
            })?;
        let producer = ExecutionRepo::get_by_id(&*self.db, producer_execution_id)
            .await?
            .filter(|execution| execution.task_id == task.id && execution.role == "reviewer")
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "ReviewReport producer is not an exact reviewer Execution",
                )
            })?;
        let producer_work_unit_id = producer.work_unit_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation(
                "rework requires a ReviewReport from a reviewer WorkUnit with an exact subject",
            )
        })?;
        let producer_subject = get_review_subject(&self.db, producer_work_unit_id)
            .await?
            .filter(|subject| {
                subject.reviewed_work_unit_id == reviewed.id
                    && subject.reviewed_work_unit_version == Some(reviewed_version)
            })
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "ReviewReport producer reviewed a different WorkUnit subject",
                )
            })?;
        let result_execution = latest_completed_work_unit_execution(
            &self.db,
            &task.id,
            &reviewed.id,
            reviewed_version,
        )
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation(
                "reviewed WorkUnit has no completed exact result to consume",
            )
        })?;
        if producer_subject.result_execution_id.as_deref() != Some(result_execution.id.as_str()) {
            return Err(ServiceError::invalid_operation(
                "ReviewReport producer subject does not pin the requested completed result",
            ));
        }
        let report_result = report_subject.get("reviewed_result").ok_or_else(|| {
            ServiceError::invalid_operation("ReviewReport has no frozen reviewed result")
        })?;
        if report_result
            .get("execution_id")
            .and_then(serde_json::Value::as_str)
            != Some(result_execution.id.as_str())
            || report_result
                .get("artifact_id")
                .and_then(serde_json::Value::as_str)
                != producer_subject.result_artifact_id.as_deref()
            || report_result
                .get("commit_sha")
                .and_then(serde_json::Value::as_str)
                != producer_subject.result_commit_sha.as_deref()
            || report_result
                .get("workspace_id")
                .and_then(serde_json::Value::as_str)
                != producer_subject.result_workspace_id.as_deref()
            || report_result
                .get("workspace_snapshot_digest")
                .and_then(serde_json::Value::as_str)
                != producer_subject.workspace_snapshot_digest.as_deref()
        {
            return Err(ServiceError::invalid_operation(
                "ReviewReport does not preserve the exact reviewed result identity and digest",
            ));
        }
        if reviewed.requires_integration
            && (result_execution.after_sha.is_none() || result_execution.workspace_id.is_none())
        {
            return Err(ServiceError::invalid_operation(
                "repository rework requires the reviewed Execution commit and Workspace",
            ));
        }
        if let Some(review_actor) = input.review_actor.as_ref() {
            if input.correction_actor.as_ref() == Some(review_actor) {
                return Err(ServiceError::invalid_operation(
                    "independent review cannot use the correction Actor",
                ));
            }
            if actor_contributed_to_work_unit(&self.db, &task.id, &reviewed.id, review_actor)
                .await?
            {
                return Err(ServiceError::invalid_operation(
                    "independent review cannot use an Actor that contributed to the reviewed result",
                ));
            }
        }
        let correction_role = db::canonical_task_role_name(&input.correction_role)
            .ok_or_else(|| ServiceError::invalid_operation("correction role is not a TaskRole"))?;
        let review_role = db::canonical_task_role_name("reviewer")
            .ok_or_else(|| ServiceError::invalid_operation("reviewer role is not a TaskRole"))?;
        let correction_role_exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task_role WHERE task_id = ? AND role = ?)",
        )
        .bind(&task.id)
        .bind(&correction_role)
        .fetch_one(self.db.pool())
        .await?;
        if correction_role_exists == 0 {
            return Err(ServiceError::invalid_operation(format!(
                "correction role {correction_role} does not exist on this Task"
            )));
        }
        let review_role_exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task_role WHERE task_id = ? AND role = ?)",
        )
        .bind(&task.id)
        .bind(&review_role)
        .fetch_one(self.db.pool())
        .await?;
        if review_role_exists == 0 {
            return Err(ServiceError::invalid_operation(
                "reviewer TaskRole does not exist on this Task",
            ));
        }

        let workspace_snapshot_digest = producer_subject.workspace_snapshot_digest.clone();

        let now = now_rfc3339();
        let correction_id = new_uuid_v4();
        let review_id = new_uuid_v4();
        let receipt_id = new_uuid_v4();
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        if let Some(row) = sqlx::query(
            "SELECT id, request_digest FROM task_rework_receipt
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&task.id)
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            if row.try_get::<String, _>("request_digest")? != request_digest {
                return Err(DbError::IdempotencyConflict.into());
            }
            tx.commit().await?;
            return self
                .replay_if_receipt_matches(&input, &request_digest)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "rework receipt could not be resolved after concurrent commit",
                    )
                });
        }
        let current_task_version: i64 =
            sqlx::query_scalar("SELECT version FROM task WHERE id = ? AND deleted_at IS NULL")
                .bind(&task.id)
                .fetch_one(&mut *tx)
                .await?;
        let current_lifecycle: String =
            sqlx::query_scalar("SELECT state FROM task_lifecycle WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(&mut *tx)
                .await?;
        if current_task_version != input.expected_task_version
            || current_lifecycle != lifecycle.state.to_string()
        {
            return Err(DbError::VersionConflict.into());
        }
        let round: i64 =
            sqlx::query_scalar("SELECT COUNT(*) + 1 FROM task_rework_receipt WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(&mut *tx)
                .await?;
        if round > MAX_REWORK_ROUNDS {
            return Err(ServiceError::conflict(format!(
                "rework budget exhausted after {MAX_REWORK_ROUNDS} rounds"
            )));
        }

        insert_work_unit_in_tx(
            &mut tx,
            &task.id,
            &correction_id,
            &input.correction_title,
            &input.correction_scope,
            &correction_role,
            input.correction_actor.as_ref(),
            reviewed.requires_integration,
            Some(("artifact", &review_report.id, None)),
            &actor,
            &now,
        )
        .await?;
        insert_work_unit_in_tx(
            &mut tx,
            &task.id,
            &review_id,
            &input.review_title,
            &input.review_scope,
            &review_role,
            input.review_actor.as_ref(),
            false,
            Some(("work_unit", &correction_id, None)),
            &actor,
            &now,
        )
        .await?;
        sqlx::query(
            "INSERT INTO work_unit_result_requirement (
                work_unit_id, task_id, source_work_unit_id, source_work_unit_version,
                result_execution_id, result_artifact_id, result_commit_sha,
                result_workspace_id, workspace_snapshot_digest, created_at
             ) VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?, ?)",
        )
        .bind(&correction_id)
        .bind(&task.id)
        .bind(&reviewed.id)
        .bind(reviewed_version)
        .bind(&result_execution.id)
        .bind(result_execution.after_sha.as_deref())
        .bind(result_execution.workspace_id.as_deref())
        .bind(workspace_snapshot_digest.as_deref())
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO work_unit_review_subject (
                review_work_unit_id, task_id, reviewed_work_unit_id,
                reviewed_work_unit_version, result_execution_id, result_artifact_id,
                result_commit_sha, result_workspace_id, workspace_snapshot_digest, created_at
             ) VALUES (?, ?, ?, NULL, NULL, NULL, NULL, NULL, NULL, ?)",
        )
        .bind(&review_id)
        .bind(&task.id)
        .bind(&correction_id)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO task_rework_receipt (
                id, task_id, idempotency_key, request_digest,
                review_report_artifact_id, reviewed_work_unit_id, reviewed_work_unit_version,
                correction_work_unit_id, review_work_unit_id, round, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&receipt_id)
        .bind(&task.id)
        .bind(&input.idempotency_key)
        .bind(&request_digest)
        .bind(&review_report.id)
        .bind(&reviewed.id)
        .bind(reviewed_version)
        .bind(&correction_id)
        .bind(&review_id)
        .bind(round)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        for (work_unit_id, _title, kind) in [
            (&correction_id, &input.correction_title, "correction"),
            (&review_id, &input.review_title, "review"),
        ] {
            let event = CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "work_unit.created".to_owned(),
                entity_type: "work_unit".to_owned(),
                entity_id: work_unit_id.clone(),
                actor_type: actor.kind().to_string(),
                actor_id: Some(actor.id().to_owned()),
                scope_type: "task".to_owned(),
                scope_id: task.id.clone(),
                correlation_id: receipt_id.clone(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("work_unit.created:{work_unit_id}")),
                payload_json: serde_json::json!({
                    "task_id": task.id,
                    "work_unit_id": work_unit_id,
                    "version": 1,
                    "status": "open",
                    "rework_round": round,
                    "rework_kind": kind,
                    "review_report_artifact_id": review_report.id,
                })
                .to_string(),
                created_at: now.clone(),
            };
            DomainEventRepo::append_event_in_tx(&*self.db, &mut tx, &event).await?;
        }
        let requested_event = CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "task.rework_requested".to_owned(),
            entity_type: "task".to_owned(),
            entity_id: task.id.clone(),
            actor_type: actor.kind().to_string(),
            actor_id: Some(actor.id().to_owned()),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: receipt_id.clone(),
            causation_id: Some(producer_execution_id.to_owned()),
            causation_depth: 1,
            dedupe_key: Some(format!("task.rework_requested:{receipt_id}")),
            payload_json: serde_json::json!({
                "task_id": task.id,
                "review_report_artifact_id": review_report.id,
                "reviewed_work_unit_id": reviewed.id,
                "reviewed_work_unit_version": reviewed_version,
                "correction_work_unit_id": correction_id,
                "review_work_unit_id": review_id,
                "round": round,
            })
            .to_string(),
            created_at: now,
        };
        DomainEventRepo::append_event_in_tx(&*self.db, &mut tx, &requested_event).await?;
        tx.commit().await?;

        let correction_work_unit = WorkUnitRepo::get_by_id(&*self.db, &correction_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", correction_id.clone()))?;
        let review_work_unit = WorkUnitRepo::get_by_id(&*self.db, &review_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", review_id.clone()))?;
        let review_subject = get_review_subject(&self.db, &review_id).await?;
        Ok(RequestReworkResult {
            correction_work_unit,
            review_work_unit,
            review_subject,
            rounds_remaining: MAX_REWORK_ROUNDS - round,
            replayed: false,
        })
    }

    async fn replay_if_receipt_matches(
        &self,
        input: &RequestReworkInput,
        request_digest: &str,
    ) -> Result<Option<RequestReworkResult>> {
        let Some(row) = sqlx::query(
            "SELECT id, request_digest, correction_work_unit_id, review_work_unit_id, round
             FROM task_rework_receipt WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .fetch_optional(self.db.pool())
        .await?
        else {
            return Ok(None);
        };
        if row.try_get::<String, _>("request_digest")? != request_digest {
            return Err(DbError::IdempotencyConflict.into());
        }
        let correction_id: String = row.try_get("correction_work_unit_id")?;
        let review_id: String = row.try_get("review_work_unit_id")?;
        let round: i64 = row.try_get("round")?;
        let correction_work_unit = WorkUnitRepo::get_by_id(&*self.db, &correction_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", correction_id.clone()))?;
        let review_work_unit = WorkUnitRepo::get_by_id(&*self.db, &review_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", review_id.clone()))?;
        Ok(Some(RequestReworkResult {
            correction_work_unit,
            review_work_unit,
            review_subject: get_review_subject(&self.db, &review_id).await?,
            rounds_remaining: MAX_REWORK_ROUNDS - round,
            replayed: true,
        }))
    }
}

fn validate_rework_input(input: &RequestReworkInput) -> Result<()> {
    for (field, value) in [
        ("task_id", input.task_id.as_str()),
        (
            "review_report_artifact_id",
            input.review_report_artifact_id.as_str(),
        ),
        (
            "reviewed_work_unit_id",
            input.reviewed_work_unit_id.as_str(),
        ),
        ("correction_title", input.correction_title.as_str()),
        ("correction_scope", input.correction_scope.as_str()),
        ("correction_role", input.correction_role.as_str()),
        ("review_title", input.review_title.as_str()),
        ("review_scope", input.review_scope.as_str()),
        ("idempotency_key", input.idempotency_key.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(ServiceError::invalid_operation(format!(
                "{field} must not be empty"
            )));
        }
    }
    if input.idempotency_key.len() > 256 {
        return Err(ServiceError::invalid_operation(
            "rework idempotency_key must be at most 256 characters",
        ));
    }
    Ok(())
}

fn rework_request_digest(input: &RequestReworkInput) -> Result<String> {
    let encoded = serde_json::to_vec(input)
        .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

#[allow(clippy::too_many_arguments)]
async fn insert_work_unit_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: &str,
    id: &str,
    title: &str,
    scope: &str,
    role: &str,
    assigned_actor: Option<&ActorRef>,
    requires_integration: bool,
    provenance: Option<(&str, &str, Option<&str>)>,
    created_by: &db::ActorRef,
    now: &str,
) -> Result<()> {
    let (assigned_kind, assigned_id) = match assigned_actor {
        Some(ActorRef::Human(id)) => (Some("human"), Some(id.as_str())),
        Some(ActorRef::Agent(id)) => (Some("agent"), Some(id.as_str())),
        None => (None, None),
    };
    let (provenance_kind, provenance_id, provenance_actor_kind) =
        provenance.unwrap_or(("external", "", None));
    let provenance_id = if provenance_kind == "external" {
        None
    } else {
        Some(provenance_id)
    };
    sqlx::query(
        "INSERT INTO work_unit (
            id, task_id, parent_work_unit_id, title, scope, status, role,
            assigned_actor_kind, assigned_actor_id, requires_integration,
            provenance_kind, provenance_id, provenance_actor_kind,
            created_by_kind, created_by_id, version, created_at, updated_at
         ) VALUES (?, ?, NULL, ?, ?, 'open', ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
    )
    .bind(id)
    .bind(task_id)
    .bind(title)
    .bind(scope)
    .bind(role)
    .bind(assigned_kind)
    .bind(assigned_id)
    .bind(i64::from(requires_integration))
    .bind(provenance_kind)
    .bind(provenance_id)
    .bind(provenance_actor_kind)
    .bind(created_by.kind().to_string())
    .bind(created_by.id())
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn latest_completed_work_unit_execution(
    db: &SqliteDb,
    task_id: &str,
    work_unit_id: &str,
    work_unit_version: i64,
) -> Result<Option<db::Execution>> {
    let execution_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM execution
         WHERE task_id = ? AND work_unit_id = ? AND work_unit_version = ?
           AND status = 'completed'
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(task_id)
    .bind(work_unit_id)
    .bind(work_unit_version)
    .fetch_optional(db.pool())
    .await?;
    match execution_id {
        Some(execution_id) => Ok(ExecutionRepo::get_by_id(db, &execution_id).await?),
        None => Ok(None),
    }
}

async fn actor_contributed_to_work_unit(
    db: &SqliteDb,
    task_id: &str,
    work_unit_id: &str,
    actor: &ActorRef,
) -> Result<bool> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM execution
            WHERE task_id = ? AND work_unit_id = ?
              AND actor_kind = ? AND actor_id = ?
        )",
    )
    .bind(task_id)
    .bind(work_unit_id)
    .bind(match actor {
        ActorRef::Human(_) => "human",
        ActorRef::Agent(_) => "agent",
    })
    .bind(match actor {
        ActorRef::Human(id) | ActorRef::Agent(id) => id,
    })
    .fetch_one(db.pool())
    .await?;
    if exists != 0 {
        return Ok(true);
    }
    let assigned: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT assigned_actor_kind, assigned_actor_id FROM work_unit WHERE id = ?")
            .bind(work_unit_id)
            .fetch_optional(db.pool())
            .await?;
    Ok(assigned.is_some_and(|(kind, id)| {
        kind.as_deref()
            == Some(match actor {
                ActorRef::Human(_) => "human",
                ActorRef::Agent(_) => "agent",
            })
            && id.as_deref()
                == Some(match actor {
                    ActorRef::Human(id) | ActorRef::Agent(id) => id.as_str(),
                })
    }))
}

/// Check the exact reviewer WorkUnit subject. A reviewer cannot be the Actor
/// already assigned to, or a completed contributor of, the reviewed result.
pub async fn reviewer_is_independent(
    db: &SqliteDb,
    review_work_unit_id: &str,
    actor: &ActorRef,
) -> Result<bool> {
    let Some(subject) = get_review_subject(db, review_work_unit_id).await? else {
        return Ok(true);
    };
    Ok(
        !actor_contributed_to_work_unit(
            db,
            &subject.task_id,
            &subject.reviewed_work_unit_id,
            actor,
        )
        .await?,
    )
}

/// A review WorkUnit may run only after its exact reviewed WorkUnit is
/// completed and its frozen result Execution and Workspace digest are exact.
pub async fn review_subject_ready(db: &SqliteDb, review_work_unit_id: &str) -> Result<bool> {
    let Some(subject) = get_review_subject(db, review_work_unit_id).await? else {
        return Ok(false);
    };
    let Some(reviewed) = WorkUnitRepo::get_by_id(db, &subject.reviewed_work_unit_id).await? else {
        return Ok(false);
    };
    let Some(version) = subject.reviewed_work_unit_version else {
        return Ok(false);
    };
    let Some(execution_id) = subject.result_execution_id.as_deref() else {
        return Ok(false);
    };
    if reviewed.status != WorkUnitStatus::Completed {
        return Ok(false);
    }
    let exact_completed_result = ExecutionRepo::get_by_id(db, execution_id)
        .await?
        .is_some_and(|execution| {
            execution.status == db::ExecutionStatus::Completed
                && execution.work_unit_id.as_deref() == Some(subject.reviewed_work_unit_id.as_str())
                && execution.work_unit_version == Some(version)
                && execution.task_id == subject.task_id
        });
    if !exact_completed_result {
        return Ok(false);
    }
    Ok(subject.result_workspace_id.is_none() || subject.workspace_snapshot_digest.is_some())
}

pub async fn get_review_subject(
    db: &SqliteDb,
    review_work_unit_id: &str,
) -> Result<Option<WorkUnitReviewSubject>> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT review_work_unit_id, task_id, reviewed_work_unit_id,
                reviewed_work_unit_version, result_execution_id, result_artifact_id,
                result_commit_sha, result_workspace_id, workspace_snapshot_digest, created_at
         FROM work_unit_review_subject WHERE review_work_unit_id = ?",
    )
    .bind(review_work_unit_id)
    .fetch_optional(db.pool())
    .await?;
    row.map(|row| {
        Ok(WorkUnitReviewSubject {
            review_work_unit_id: row.try_get("review_work_unit_id")?,
            task_id: row.try_get("task_id")?,
            reviewed_work_unit_id: row.try_get("reviewed_work_unit_id")?,
            id: row.try_get("review_work_unit_id")?,
            reviewed_work_unit_version: row.try_get("reviewed_work_unit_version")?,
            result_execution_id: row.try_get("result_execution_id")?,
            result_artifact_id: row.try_get("result_artifact_id")?,
            result_commit_sha: row.try_get("result_commit_sha")?,
            result_workspace_id: row.try_get("result_workspace_id")?,
            workspace_snapshot_digest: row.try_get("workspace_snapshot_digest")?,
            created_at: row.try_get("created_at")?,
        })
    })
    .transpose()
}

pub async fn result_requirement_available(db: &SqliteDb, work_unit_id: &str) -> Result<bool> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM work_unit_result_requirement r
            JOIN execution e ON e.id = r.result_execution_id
            WHERE r.work_unit_id = ? AND e.status = 'completed'
              AND (r.result_workspace_id IS NULL OR r.result_commit_sha IS NOT NULL)
        )",
    )
    .bind(work_unit_id)
    .fetch_one(db.pool())
    .await?;
    Ok(exists != 0)
}

/// Fill the review subject's frozen result from the exact completed execution
/// of the correction WorkUnit. Re-running this is idempotent.
pub async fn populate_review_subject_result(
    db: &SqliteDb,
    task_id: &str,
    completed_work_unit_id: &str,
    completed_work_unit_version: i64,
) -> Result<()> {
    let Some(execution) = latest_completed_work_unit_execution(
        db,
        task_id,
        completed_work_unit_id,
        completed_work_unit_version,
    )
    .await?
    else {
        return Ok(());
    };
    sqlx::query(
        "UPDATE work_unit_review_subject
         SET result_execution_id = ?, result_commit_sha = ?, result_workspace_id = ?
         WHERE task_id = ? AND reviewed_work_unit_id = ?
           AND reviewed_work_unit_version = ?
           AND result_execution_id IS NULL",
    )
    .bind(&execution.id)
    .bind(execution.after_sha.as_deref())
    .bind(execution.workspace_id.as_deref())
    .bind(task_id)
    .bind(completed_work_unit_id)
    .bind(completed_work_unit_version)
    .execute(db.pool())
    .await?;
    if let Some(workspace_id) = execution.workspace_id.as_deref() {
        let workspace = db::WorkspaceRepo::get_by_id(db, workspace_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
        let live_head =
            git::get_current_sha(std::path::Path::new(&workspace.worktree_path)).await?;
        if execution.after_sha.as_deref() != Some(live_head.as_str()) {
            return Err(ServiceError::invalid_operation(
                "completed result Workspace no longer matches its exact Execution commit",
            ));
        }
        let digest = crate::ValidationService::snapshot_digest(&workspace.worktree_path).await?;
        sqlx::query(
            "UPDATE work_unit_review_subject
             SET workspace_snapshot_digest = ?
             WHERE task_id = ? AND reviewed_work_unit_id = ?
               AND reviewed_work_unit_version = ? AND result_execution_id = ?
               AND result_workspace_id = ? AND workspace_snapshot_digest IS NULL",
        )
        .bind(digest)
        .bind(task_id)
        .bind(completed_work_unit_id)
        .bind(completed_work_unit_version)
        .bind(&execution.id)
        .bind(workspace_id)
        .execute(db.pool())
        .await?;
    }
    Ok(())
}

pub fn review_subject_response(
    subject: &WorkUnitReviewSubject,
) -> api_types::ReviewSubjectResponse {
    api_types::ReviewSubjectResponse {
        id: subject.id.clone(),
        review_work_unit_id: subject.review_work_unit_id.clone(),
        reviewed_work_unit_id: subject.reviewed_work_unit_id.clone(),
        reviewed_work_unit_version: subject.reviewed_work_unit_version,
        result_execution_id: subject.result_execution_id.clone(),
        result_artifact_id: subject.result_artifact_id.clone(),
        result_commit_sha: subject.result_commit_sha.clone(),
        result_workspace_id: subject.result_workspace_id.clone(),
        workspace_snapshot_digest: subject.workspace_snapshot_digest.clone(),
    }
}
