use super::*;
use crate::{MaterializedWorkUnit, TaskCoordinationMode, UpdateTaskQualityRequestState};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn deterministic_review_line_id(reviewed_work_unit_id: &str) -> String {
    let digest = Sha256::digest(
        [
            b"forge.task-coordination.review-line.v1".as_slice(),
            reviewed_work_unit_id.as_bytes(),
        ]
        .concat(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn deterministic_review_gate_id(reviewed_work_unit_id: &str) -> String {
    let digest = Sha256::digest(
        [
            b"forge.task-coordination.review-gate.v1".as_slice(),
            reviewed_work_unit_id.as_bytes(),
        ]
        .concat(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

struct MaterializedReviewControl {
    reviewed_work_unit_id: String,
    line_id: String,
    gate_id: String,
    gate_kind: String,
}

async fn authorize_task_coordination_write(
    db: &SqliteDb,
    input: AuthorizeTaskCoordinationInput,
    expected_task_version: i64,
    batch_binding: Option<ProjectPlanBatchAuthorizationBinding>,
) -> Result<TaskCoordinationWrite> {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    if let Some(existing) = sqlx::query(
        "SELECT * FROM task_coordination_receipt
         WHERE task_id = ? AND idempotency_key = ?",
    )
    .bind(&input.task_id)
    .bind(&input.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    .map(map_task_coordination_receipt)
    .transpose()?
    {
        if existing.request_digest != input.request_digest
            || existing.operation != TaskCoordinationOperation::Authorize
        {
            return Err(DbError::IdempotencyConflict);
        }
        if let Some(binding) = batch_binding.as_ref() {
            let bound: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM project_plan_batch_coordination_authorization a
                    JOIN project_plan_batch b ON b.id=a.batch_id
                    JOIN project_plan p ON p.project_id=b.project_id
                    WHERE a.batch_id=? AND a.project_id=? AND a.task_id=?
                      AND a.authorization_version=? AND a.authorized_by=?
                      AND a.expected_batch_version=? AND a.expected_graph_version=?
                      AND b.authorized_by=? AND b.version=?
                      AND b.graph_version=? AND p.version=b.graph_version
                      AND ((b.state IN ('paused','running') AND b.preparation_complete=0)
                        OR (b.state='running' AND b.preparation_complete=1
                            AND EXISTS(SELECT 1 FROM project_plan_batch_task bt
                                WHERE bt.batch_id=b.id AND bt.task_id=a.task_id
                                  AND bt.state='blocked'
                                  AND bt.blocker='lifecycle:blocked')))
                )",
            )
            .bind(&binding.batch_id)
            .bind(&binding.project_id)
            .bind(&input.task_id)
            .bind(existing.authorization_version)
            .bind(&binding.authorized_by)
            .bind(binding.expected_batch_version)
            .bind(binding.expected_graph_version)
            .bind(&binding.authorized_by)
            .bind(binding.expected_batch_version)
            .bind(binding.expected_graph_version)
            .fetch_one(&mut *tx)
            .await?;
            if !bound {
                return Err(DbError::ProjectPlanBatchInactive);
            }
        }
        let coordination = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
            .bind(&input.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        return Ok(TaskCoordinationWrite {
            coordination,
            receipt: existing,
            replayed: true,
            event: None,
        });
    }
    let task = sqlx::query(
        "SELECT id,project_id,version,title,description,repo_id,parent_task_id
         FROM task WHERE id=? AND deleted_at IS NULL",
    )
    .bind(&input.task_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let actual_task_version: i64 = task.try_get("version")?;
    if actual_task_version != expected_task_version {
        return Err(DbError::TaskVersionConflict {
            expected: expected_task_version,
            actual: actual_task_version,
        });
    }
    let (actor_kind, actor_id) = coordination_actor_columns(input.coordinator_actor.as_ref());
    if let Some(binding) = batch_binding.as_ref() {
        let project_id: String = task.try_get("project_id")?;
        if binding.project_id != project_id
            || binding.authorized_by != input.requested_by
            || binding.expected_team_generation != input.expected_team_generation
            || input.mode != TaskCoordinationMode::PrepareAndExecute
            || !matches!(input.coordinator_actor.as_ref(), Some(ActorRef::Agent(_)))
        {
            return Err(DbError::ProjectPlanBatchInactive);
        }
        let eligible: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM project_plan_batch b
                JOIN project_plan p ON p.project_id=b.project_id
                JOIN project_plan_batch_task bt ON bt.batch_id=b.id AND bt.task_id=?
                JOIN task t ON t.id=bt.task_id AND t.project_id=b.project_id
                  AND t.deleted_at IS NULL AND t.version=?
                JOIN task_coordination c ON c.task_id=t.id
                JOIN project_member m ON m.project_id=b.project_id
                  AND m.user_id=? AND m.role IN ('owner','admin')
                JOIN project owner_project ON owner_project.id=b.project_id
                JOIN project_member owner_membership ON owner_membership.project_id=b.project_id
                  AND owner_membership.user_id=owner_project.owner_id
                  AND owner_membership.role='owner'
                WHERE b.id=? AND b.project_id=? AND b.authorized_by=?
                  AND b.version=? AND b.graph_version=? AND p.version=b.graph_version
                  AND ((b.state IN ('paused','running') AND b.preparation_complete=0
                        AND json_extract(bt.authorization_baseline_json,'$.task_version')=?
                        AND json_extract(bt.authorization_baseline_json,'$.lifecycle_state') IN ('backlog','ready'))
                    OR (b.state='running' AND b.preparation_complete=1
                        AND bt.state='blocked' AND bt.blocker='lifecycle:blocked'
                        AND json_extract(bt.authorization_baseline_json,'$.lifecycle_state')='blocked'
                        AND json_extract(bt.authorization_baseline_json,'$.task_version')+1=?))
                  AND json_extract(bt.authorization_baseline_json,'$.authorization_version')=?
                  AND json_extract(bt.authorization_baseline_json,'$.team_generation')=?
                  AND json_extract(bt.authorization_baseline_json,'$.unconfigured_manual_default')=?
                  AND (json_extract(bt.authorization_baseline_json,'$.team_digest') IS ?
                    OR (?=1
                        AND json_extract(bt.authorization_baseline_json,'$.unconfigured_manual_default')=1
                        AND json_extract(bt.authorization_baseline_json,'$.mode')='manual'
                        AND json_extract(bt.authorization_baseline_json,'$.requested_by') IS NULL
                        AND json_extract(bt.authorization_baseline_json,'$.revoked_at') IS NULL))
                  AND (json_extract(bt.authorization_baseline_json,'$.envelope_digest') IS ?
                    OR (?=1
                        AND json_extract(bt.authorization_baseline_json,'$.unconfigured_manual_default')=1
                        AND json_extract(bt.authorization_baseline_json,'$.mode')='manual'
                        AND json_extract(bt.authorization_baseline_json,'$.requested_by') IS NULL
                        AND json_extract(bt.authorization_baseline_json,'$.revoked_at') IS NULL))
                  AND json_extract(bt.authorization_baseline_json,'$.role_id') IS ?
                  AND json_extract(bt.authorization_baseline_json,'$.actor_kind') IS ?
                  AND json_extract(bt.authorization_baseline_json,'$.actor_id') IS ?
                  AND json_extract(bt.authorization_baseline_json,'$.task_objective_digest') IS NOT NULL
                  AND c.authorization_version=json_extract(bt.authorization_baseline_json,'$.authorization_version')
                  AND c.team_generation=json_extract(bt.authorization_baseline_json,'$.team_generation')
                  AND c.team_digest IS json_extract(bt.authorization_baseline_json,'$.team_digest')
                  AND c.envelope_digest IS json_extract(bt.authorization_baseline_json,'$.envelope_digest')
                  AND c.mode IS json_extract(bt.authorization_baseline_json,'$.mode')
                  AND c.coordinator_task_role_id IS json_extract(bt.authorization_baseline_json,'$.role_id')
                  AND c.coordinator_actor_kind IS json_extract(bt.authorization_baseline_json,'$.actor_kind')
                  AND c.coordinator_actor_id IS json_extract(bt.authorization_baseline_json,'$.actor_id')
                  AND c.requested_by IS json_extract(bt.authorization_baseline_json,'$.requested_by')
                  AND c.revoked_at IS json_extract(bt.authorization_baseline_json,'$.revoked_at')
                  AND c.team_stale=json_extract(bt.authorization_baseline_json,'$.team_stale')
                  AND NOT EXISTS(SELECT 1 FROM work_unit w
                                 WHERE w.task_id=t.id AND w.status!='cancelled')
                  AND NOT EXISTS(SELECT 1 FROM task_coordination_plan plan
                                 WHERE plan.task_id=t.id
                                   AND plan.state IN ('approved','materialized'))
            )",
        )
        .bind(&input.task_id)
        .bind(expected_task_version)
        .bind(&binding.authorized_by)
        .bind(&binding.batch_id)
        .bind(&binding.project_id)
        .bind(&binding.authorized_by)
        .bind(binding.expected_batch_version)
        .bind(binding.expected_graph_version)
        .bind(expected_task_version)
        .bind(expected_task_version)
        .bind(input.expected_coordination_version)
        .bind(input.expected_team_generation)
        .bind(input.unconfigured_default_refresh)
        .bind(&input.team_digest)
        .bind(input.unconfigured_default_refresh)
        .bind(&input.envelope_digest)
        .bind(input.unconfigured_default_refresh)
        .bind(input.coordinator_task_role_id.as_deref())
        .bind(actor_kind.as_deref())
        .bind(actor_id.as_deref())
        .fetch_one(&mut *tx)
        .await?;
        if !eligible {
            return Err(DbError::ProjectPlanBatchInactive);
        }
    }
    let current = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
        .bind(&input.task_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_task_coordination)
        .transpose()?
        .ok_or(DbError::NotFound)?;
    if current.authorization_version != input.expected_coordination_version
        || current.team_generation != input.expected_team_generation
    {
        return Err(DbError::VersionConflict);
    }
    if batch_binding.is_none() {
        let active_batch_origin: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM project_plan_batch_coordination_authorization origin
                JOIN project_plan_batch batch ON batch.id=origin.batch_id
                 AND batch.project_id=origin.project_id
                WHERE origin.task_id=? AND origin.authorization_version=?
                  AND batch.state IN ('running','paused')
            )",
        )
        .bind(&input.task_id)
        .bind(current.authorization_version)
        .fetch_one(&mut *tx)
        .await?;
        if active_batch_origin {
            return Err(DbError::ProjectPlanBatchInactive);
        }
    }
    sqlx::query(
        "INSERT INTO task_coordination_receipt (
            id, task_id, operation, idempotency_key, request_digest,
            authorization_version, created_at
         ) VALUES (?, ?, 'authorize', ?, ?, ?, ?)",
    )
    .bind(&input.receipt_id)
    .bind(&input.task_id)
    .bind(&input.idempotency_key)
    .bind(&input.request_digest)
    .bind(current.authorization_version + 1)
    .bind(&input.created_at)
    .execute(&mut *tx)
    .await
    .map_err(check_error)?;
    sqlx::query(
        "UPDATE task_coordination
         SET mode = ?, coordinator_task_role_id = ?, coordinator_actor_kind = ?,
             coordinator_actor_id = ?, authorization_version = authorization_version + 1,
             team_snapshot_json = ?, team_digest = ?, envelope_json = ?,
             envelope_digest = ?, requested_by = ?, updated_at = ?,
             revoked_at = NULL, revoke_reason = NULL, team_stale = 0
         WHERE task_id = ? AND authorization_version = ?",
    )
    .bind(input.mode.to_string())
    .bind(input.coordinator_task_role_id.as_deref())
    .bind(actor_kind.as_deref())
    .bind(actor_id.as_deref())
    .bind(&input.team_snapshot_json)
    .bind(&input.team_digest)
    .bind(&input.envelope_json)
    .bind(&input.envelope_digest)
    .bind(&input.requested_by)
    .bind(&input.created_at)
    .bind(&input.task_id)
    .bind(input.expected_coordination_version)
    .execute(&mut *tx)
    .await
    .map_err(check_error)?;
    let coordination = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
        .bind(&input.task_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(DbError::from)
        .and_then(map_task_coordination)?;
    if let Some(binding) = batch_binding {
        let task_version: i64 = task.try_get("version")?;
        let task_title: String = task.try_get("title")?;
        let task_description: Option<String> = task.try_get("description")?;
        let task_repo_id: Option<String> = task.try_get("repo_id")?;
        let task_parent_task_id: Option<String> = task.try_get("parent_task_id")?;
        sqlx::query(
            "INSERT INTO project_plan_batch_coordination_authorization(
                batch_id,project_id,task_id,authorization_version,authorized_by,
                expected_batch_version,expected_graph_version,task_version,
                task_objective_digest,task_title,task_description,task_repo_id,
                task_parent_task_id,team_generation,team_digest,envelope_digest,
                unconfigured_default_refresh,mode,role_id,actor_kind,actor_id,created_at
             ) VALUES(?,?,?,?,?,?,?,?,json_extract((SELECT authorization_baseline_json
                 FROM project_plan_batch_task WHERE batch_id=? AND task_id=?),
                 '$.task_objective_digest'),?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&binding.batch_id)
        .bind(&binding.project_id)
        .bind(&input.task_id)
        .bind(coordination.authorization_version)
        .bind(&binding.authorized_by)
        .bind(binding.expected_batch_version)
        .bind(binding.expected_graph_version)
        .bind(task_version)
        .bind(&binding.batch_id)
        .bind(&input.task_id)
        .bind(&task_title)
        .bind(task_description.as_deref())
        .bind(task_repo_id.as_deref())
        .bind(task_parent_task_id.as_deref())
        .bind(coordination.team_generation)
        .bind(&coordination.team_digest)
        .bind(&coordination.envelope_digest)
        .bind(input.unconfigured_default_refresh)
        .bind(coordination.mode.to_string())
        .bind(coordination.coordinator_task_role_id.as_deref())
        .bind(
            coordination
                .coordinator_actor
                .as_ref()
                .map(ActorRef::kind)
                .map(|v| v.to_string()),
        )
        .bind(coordination.coordinator_actor.as_ref().map(ActorRef::id))
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
    }
    let event = DomainEventRepo::append_event_in_tx(db, &mut tx, &input.event).await?;
    tx.commit().await?;
    Ok(TaskCoordinationWrite {
        coordination,
        receipt: TaskCoordinationReceipt {
            id: input.receipt_id,
            task_id: input.task_id,
            operation: TaskCoordinationOperation::Authorize,
            idempotency_key: input.idempotency_key,
            request_digest: input.request_digest,
            authorization_version: input.expected_coordination_version + 1,
            created_at: input.created_at,
        },
        replayed: false,
        event: Some(event),
    })
}

async fn create_task_coordination_request_write(
    db: &SqliteDb,
    input: CreateTaskCoordinationRequestInput,
    batch_binding: Option<ProjectPlanBatchRequestBinding>,
) -> Result<(TaskCoordinationRequest, Option<DomainEvent>)> {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    if let Some(existing) = sqlx::query(
        "SELECT * FROM task_coordination_request
         WHERE task_id=? AND idempotency_key=?",
    )
    .bind(&input.task_id)
    .bind(&input.idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    .map(map_task_coordination_request)
    .transpose()?
    {
        if existing.request_digest != input.request_digest
            || existing.kind != input.kind
            || existing.review_report_artifact_id != input.review_report_artifact_id
            || existing.review_report_digest != input.review_report_digest
            || existing.review_producer_execution_id != input.review_producer_execution_id
        {
            return Err(DbError::IdempotencyConflict);
        }
        let existing_is_batch_bound: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM project_plan_batch_coordination_request
                           WHERE request_id=?)",
        )
        .bind(&existing.id)
        .fetch_one(&mut *tx)
        .await?;
        match batch_binding.as_ref() {
            None if existing_is_batch_bound => return Err(DbError::ProjectPlanBatchInactive),
            Some(binding) => {
                let expected_origin = match &binding.origin {
                    ProjectPlanBatchRequestOrigin::Scheduler { claim_token } => {
                        ("scheduler", Some(claim_token.as_str()), None, None, None)
                    }
                    ProjectPlanBatchRequestOrigin::QualityReplan {
                        source_artifact_id,
                        producer_execution_id,
                    } => (
                        "quality_replan",
                        None,
                        Some(source_artifact_id.as_str()),
                        Some(producer_execution_id.as_str()),
                        None,
                    ),
                    ProjectPlanBatchRequestOrigin::ReviewTriage {
                        source_artifact_id,
                        producer_execution_id,
                    } => (
                        "review_triage",
                        None,
                        Some(source_artifact_id.as_str()),
                        Some(producer_execution_id.as_str()),
                        None,
                    ),
                    ProjectPlanBatchRequestOrigin::HumanReplan {
                        approved_plan_artifact_id,
                    } => (
                        "human_replan",
                        None,
                        None,
                        None,
                        Some(approved_plan_artifact_id.as_str()),
                    ),
                };
                let matching: bool = sqlx::query_scalar(
                    "SELECT EXISTS(
                        SELECT 1 FROM project_plan_batch_coordination_request scope
                        JOIN project_plan_batch b ON b.id=scope.batch_id
                        JOIN project_plan p ON p.project_id=b.project_id
                        WHERE scope.request_id=? AND scope.project_id=?
                          AND scope.batch_id=? AND scope.task_id=?
                          AND scope.authorization_version=? AND scope.authorized_by=?
                          AND scope.expected_batch_version=?
                          AND scope.expected_graph_version=?
                          AND scope.expected_task_version=?
                          AND scope.coordination_snapshot_digest=?
                          AND scope.origin_kind=? AND scope.claim_token IS ?
                          AND scope.source_artifact_id IS ?
                          AND scope.producer_execution_id IS ?
                          AND scope.approved_plan_artifact_id IS ?
                          AND b.project_id=scope.project_id AND b.authorized_by=scope.authorized_by
                          AND b.state='running' AND b.version=scope.expected_batch_version
                          AND b.graph_version=scope.expected_graph_version
                          AND p.version=b.graph_version
                    )",
                )
                .bind(&existing.id)
                .bind(&binding.project_id)
                .bind(&binding.batch_id)
                .bind(&input.task_id)
                .bind(binding.expected_coordination_version)
                .bind(&binding.authorized_by)
                .bind(binding.expected_batch_version)
                .bind(binding.expected_graph_version)
                .bind(binding.expected_task_version)
                .bind(&binding.coordination_snapshot_digest)
                .bind(expected_origin.0)
                .bind(expected_origin.1)
                .bind(expected_origin.2)
                .bind(expected_origin.3)
                .bind(expected_origin.4)
                .fetch_one(&mut *tx)
                .await?;
                if !matching {
                    return Err(DbError::ProjectPlanBatchInactive);
                }
            }
            None => {}
        }
        tx.commit().await?;
        return Ok((existing, None));
    }

    let task: Option<(i64, String)> =
        sqlx::query_as("SELECT version,project_id FROM task WHERE id=? AND deleted_at IS NULL")
            .bind(&input.task_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((actual_task_version, project_id)) = task else {
        return Err(DbError::NotFound);
    };
    if actual_task_version != input.expected_task_version {
        return Err(DbError::TaskVersionConflict {
            expected: input.expected_task_version,
            actual: actual_task_version,
        });
    }
    let current = sqlx::query("SELECT * FROM task_coordination WHERE task_id=?")
        .bind(&input.task_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_task_coordination)
        .transpose()?
        .ok_or(DbError::NotFound)?;
    if current.authorization_version != input.authorization_version
        || current.mode != input.mode
        || current.requested_by.as_deref() != Some(input.requested_by.as_str())
        || current.revoked_at.is_some()
        || current.team_stale
    {
        return Err(DbError::VersionConflict);
    }

    if let Some(binding) = batch_binding.as_ref() {
        if binding.project_id != project_id
            || binding.authorized_by != input.requested_by
            || binding.expected_task_version != input.expected_task_version
            || binding.expected_coordination_version != input.authorization_version
            || binding.expected_coordination_version != current.authorization_version
            || binding.coordination_snapshot_digest.trim().is_empty()
            || binding.coordination_snapshot_digest.len() != 64
            || input.mode != TaskCoordinationMode::PrepareAndExecute
        {
            return Err(DbError::ProjectPlanBatchInactive);
        }
        validate_project_batch_request_binding_in_tx(&mut tx, &input, binding).await?;
        let origin = match &binding.origin {
            ProjectPlanBatchRequestOrigin::Scheduler { claim_token } => {
                ("scheduler", Some(claim_token.as_str()), None, None, None)
            }
            ProjectPlanBatchRequestOrigin::QualityReplan {
                source_artifact_id,
                producer_execution_id,
            } => (
                "quality_replan",
                None,
                Some(source_artifact_id.as_str()),
                Some(producer_execution_id.as_str()),
                None,
            ),
            ProjectPlanBatchRequestOrigin::ReviewTriage {
                source_artifact_id,
                producer_execution_id,
            } => (
                "review_triage",
                None,
                Some(source_artifact_id.as_str()),
                Some(producer_execution_id.as_str()),
                None,
            ),
            ProjectPlanBatchRequestOrigin::HumanReplan {
                approved_plan_artifact_id,
            } => (
                "human_replan",
                None,
                None,
                None,
                Some(approved_plan_artifact_id.as_str()),
            ),
        };
        sqlx::query(
            "INSERT INTO project_plan_batch_coordination_request(
                request_id,project_id,batch_id,task_id,authorization_version,authorized_by,
                expected_batch_version,expected_graph_version,expected_task_version,
                coordination_snapshot_digest,origin_kind,claim_token,source_artifact_id,
                producer_execution_id,approved_plan_artifact_id,created_at
             ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&input.id)
        .bind(&binding.project_id)
        .bind(&binding.batch_id)
        .bind(&input.task_id)
        .bind(binding.expected_coordination_version)
        .bind(&binding.authorized_by)
        .bind(binding.expected_batch_version)
        .bind(binding.expected_graph_version)
        .bind(binding.expected_task_version)
        .bind(&binding.coordination_snapshot_digest)
        .bind(origin.0)
        .bind(origin.1)
        .bind(origin.2)
        .bind(origin.3)
        .bind(origin.4)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
    } else {
        let batch_scoped: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM project_plan_batch_coordination_authorization a
                WHERE a.task_id=? AND a.authorization_version=?
            ) OR EXISTS(
                SELECT 1 FROM project_plan_batch_task bt
                JOIN project_plan_batch b ON b.id=bt.batch_id
                WHERE bt.task_id=? AND b.state IN ('running','paused')
            )",
        )
        .bind(&input.task_id)
        .bind(input.authorization_version)
        .bind(&input.task_id)
        .fetch_one(&mut *tx)
        .await?;
        if batch_scoped {
            return Err(DbError::ProjectPlanBatchInactive);
        }
    }

    let budget = sqlx::query(
        "UPDATE task_coordination SET turn_count=turn_count+1,updated_at=?
         WHERE task_id=? AND authorization_version=? AND mode=? AND requested_by=?
          AND revoked_at IS NULL AND team_stale=0 AND turn_count<max_turns",
    )
    .bind(&input.created_at)
    .bind(&input.task_id)
    .bind(input.authorization_version)
    .bind(input.mode.to_string())
    .bind(&input.requested_by)
    .execute(&mut *tx)
    .await?;
    if budget.rows_affected() != 1 {
        let current: Option<(i64, i64)> =
            sqlx::query_as("SELECT turn_count,max_turns FROM task_coordination WHERE task_id=?")
                .bind(&input.task_id)
                .fetch_optional(&mut *tx)
                .await?;
        if current.is_some_and(|(turn_count, max_turns)| turn_count >= max_turns) {
            return Err(DbError::CoordinationTurnBudgetExhausted);
        }
        return Err(DbError::VersionConflict);
    }
    sqlx::query(
        "INSERT INTO task_coordination_request (
            id, task_id, authorization_version, mode, brief_json,
            objective_digest, request_digest, idempotency_key, state,
            version, created_at, updated_at, kind, review_report_artifact_id,
            review_report_digest, review_producer_execution_id
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'requested', 1, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&input.id)
    .bind(&input.task_id)
    .bind(input.authorization_version)
    .bind(input.mode.to_string())
    .bind(&input.brief_json)
    .bind(&input.objective_digest)
    .bind(&input.request_digest)
    .bind(&input.idempotency_key)
    .bind(&input.created_at)
    .bind(&input.created_at)
    .bind(input.kind.to_string())
    .bind(&input.review_report_artifact_id)
    .bind(&input.review_report_digest)
    .bind(&input.review_producer_execution_id)
    .execute(&mut *tx)
    .await
    .map_err(check_error)?;
    if let Some(binding) = batch_binding.as_ref() {
        let update = match &binding.origin {
            ProjectPlanBatchRequestOrigin::Scheduler { claim_token } => {
                sqlx::query(
                    "UPDATE project_plan_batch_task SET state='started',
                        coordination_request_id=COALESCE(coordination_request_id,?),
                        claim_expires_at=NULL,claim_token=NULL,blocker=NULL
                     WHERE batch_id=? AND task_id=? AND state='claimed' AND claim_token=?",
                )
                .bind(&input.id)
                .bind(&binding.batch_id)
                .bind(&input.task_id)
                .bind(claim_token)
                .execute(&mut *tx)
                .await?
            }
            ProjectPlanBatchRequestOrigin::QualityReplan { .. }
            | ProjectPlanBatchRequestOrigin::HumanReplan { .. }
            | ProjectPlanBatchRequestOrigin::ReviewTriage { .. } => {
                sqlx::query(
                    "UPDATE project_plan_batch_task SET
                        coordination_request_id=COALESCE(coordination_request_id,?)
                     WHERE batch_id=? AND task_id=? AND state='started'",
                )
                .bind(&input.id)
                .bind(&binding.batch_id)
                .bind(&input.task_id)
                .execute(&mut *tx)
                .await?
            }
        };
        if update.rows_affected() != 1 {
            return Err(DbError::ProjectPlanBatchInactive);
        }
    }
    let event = DomainEventRepo::append_event_in_tx(db, &mut tx, &input.event).await?;
    let request = sqlx::query("SELECT * FROM task_coordination_request WHERE id=?")
        .bind(&input.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(DbError::from)
        .and_then(map_task_coordination_request)?;
    tx.commit().await?;
    Ok((request, Some(event)))
}

async fn validate_project_batch_request_binding_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    input: &CreateTaskCoordinationRequestInput,
    binding: &ProjectPlanBatchRequestBinding,
) -> Result<()> {
    let origin_ok: bool = match &binding.origin {
        ProjectPlanBatchRequestOrigin::Scheduler { claim_token } => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM project_plan_batch_task bt
             WHERE bt.batch_id=? AND bt.task_id=? AND bt.state='claimed'
              AND bt.claim_token=? AND bt.claim_expires_at IS NOT NULL
              AND bt.claim_expires_at>=strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
            )
            .bind(&binding.batch_id)
            .bind(&input.task_id)
            .bind(claim_token)
            .fetch_one(&mut **tx)
            .await?
        }
        ProjectPlanBatchRequestOrigin::QualityReplan {
            source_artifact_id,
            producer_execution_id,
        }
        | ProjectPlanBatchRequestOrigin::ReviewTriage {
            source_artifact_id,
            producer_execution_id,
        } => {
            sqlx::query_scalar(
                "SELECT EXISTS(
                SELECT 1 FROM project_plan_batch_task bt
                WHERE bt.batch_id=? AND bt.task_id=? AND bt.state='started'
                  AND bt.coordination_snapshot_digest=?
            ) AND EXISTS(
                SELECT 1 FROM artifact_execution_producer producer
                JOIN project_plan_batch_execution exec_link
                  ON exec_link.execution_id=producer.execution_id
                 AND exec_link.batch_id=? AND exec_link.task_id=producer.task_id
                WHERE producer.artifact_id=? AND producer.execution_id=?
                  AND producer.task_id=? AND exec_link.authorization_version=?
            )",
            )
            .bind(&binding.batch_id)
            .bind(&input.task_id)
            .bind(&binding.coordination_snapshot_digest)
            .bind(&binding.batch_id)
            .bind(source_artifact_id)
            .bind(producer_execution_id)
            .bind(&input.task_id)
            .bind(binding.expected_coordination_version)
            .fetch_one(&mut **tx)
            .await?
        }
        ProjectPlanBatchRequestOrigin::HumanReplan {
            approved_plan_artifact_id,
        } => {
            sqlx::query_scalar(
                "SELECT EXISTS(
                SELECT 1 FROM project_plan_batch_task bt
                WHERE bt.batch_id=? AND bt.task_id=? AND bt.state='started'
                  AND bt.coordination_snapshot_digest=?
            ) AND EXISTS(
                SELECT 1 FROM task_coordination_plan plan
                JOIN project_plan_batch_coordination_request previous
                  ON previous.request_id=plan.request_id AND previous.batch_id=?
                 AND previous.task_id=plan.task_id
                 AND previous.authorization_version=?
                JOIN artifact plan_artifact
                  ON plan_artifact.id=plan.artifact_id AND plan_artifact.task_id=plan.task_id
                   AND plan_artifact.digest=plan.plan_digest
                WHERE plan.artifact_id=? AND plan.task_id=?
                  AND plan.authorization_version=previous.authorization_version
                  AND ((plan.state='approved' AND NOT EXISTS(
                        SELECT 1 FROM task_coordination_materialization receipt
                        WHERE receipt.plan_artifact_id=plan.artifact_id))
                    OR (plan.state='materialized' AND EXISTS(
                        SELECT 1 FROM task_coordination_materialization receipt
                        WHERE receipt.plan_artifact_id=plan.artifact_id
                          AND receipt.task_id=plan.task_id
                          AND receipt.request_id=plan.request_id
                          AND receipt.plan_digest=plan.plan_digest
                          AND receipt.authorization_version=plan.authorization_version)))
            )",
            )
            .bind(&binding.batch_id)
            .bind(&input.task_id)
            .bind(&binding.coordination_snapshot_digest)
            .bind(&binding.batch_id)
            .bind(binding.expected_coordination_version)
            .bind(approved_plan_artifact_id)
            .bind(&input.task_id)
            .fetch_one(&mut **tx)
            .await?
        }
    };
    if !origin_ok {
        return Err(DbError::ProjectPlanBatchInactive);
    }
    let current: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM project_plan_batch b
            JOIN project_plan p ON p.project_id=b.project_id
            JOIN project_plan_batch_task bt ON bt.batch_id=b.id AND bt.task_id=?
            JOIN task t ON t.id=bt.task_id AND t.project_id=b.project_id
              AND t.deleted_at IS NULL AND t.version=?
            JOIN task_coordination c ON c.task_id=t.id
            JOIN project_plan_batch_coordination_authorization a
              ON a.batch_id=b.id AND a.task_id=t.id
               AND a.authorization_version=c.authorization_version
            JOIN project_member m ON m.project_id=b.project_id
              AND m.user_id=b.authorized_by AND m.role IN ('owner','admin')
            JOIN project owner_project ON owner_project.id=b.project_id
            JOIN project_member owner_membership ON owner_membership.project_id=b.project_id
              AND owner_membership.user_id=owner_project.owner_id
              AND owner_membership.role='owner'
            WHERE b.id=? AND b.project_id=? AND b.authorized_by=?
              AND b.state='running' AND b.preparation_complete=1
              AND b.version=? AND b.graph_version=? AND p.version=b.graph_version
              AND bt.coordination_snapshot_digest=?
              AND c.authorization_version=? AND c.mode=? AND c.requested_by=?
              AND c.revoked_at IS NULL AND c.team_stale=0
              AND c.team_generation=a.team_generation AND c.team_digest=a.team_digest
              AND c.envelope_digest=a.envelope_digest
              AND c.coordinator_task_role_id=a.role_id
              AND c.coordinator_actor_kind=a.actor_kind AND c.coordinator_actor_id=a.actor_id
              AND a.project_id=b.project_id AND a.authorized_by=b.authorized_by
              AND a.expected_graph_version=b.graph_version
              AND a.mode=c.mode AND a.task_title=t.title
              AND a.task_description IS t.description AND a.task_repo_id IS t.repo_id
              AND a.task_parent_task_id IS t.parent_task_id
              AND EXISTS(SELECT 1 FROM task_lifecycle l WHERE l.task_id=t.id
                         AND l.state NOT IN ('done','cancelled','merging'))
              AND NOT EXISTS(SELECT 1 FROM task_dispatch d
                             WHERE d.task_id=t.id AND d.state='cancelled')
        )",
    )
    .bind(&input.task_id)
    .bind(binding.expected_task_version)
    .bind(&binding.batch_id)
    .bind(&binding.project_id)
    .bind(&binding.authorized_by)
    .bind(binding.expected_batch_version)
    .bind(binding.expected_graph_version)
    .bind(&binding.coordination_snapshot_digest)
    .bind(binding.expected_coordination_version)
    .bind(input.mode.to_string())
    .bind(&input.requested_by)
    .fetch_one(&mut **tx)
    .await?;
    if !current {
        return Err(DbError::ProjectPlanBatchInactive);
    }
    Ok(())
}

async fn insert_review_control(
    db: &SqliteDb,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &MaterializeTaskCoordinationPlanInput,
    control: &MaterializedReviewControl,
) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO work_unit_review_line (
            id, task_id, root_work_unit_id, current_work_unit_id,
            current_work_unit_version, current_result_execution_id,
            current_result_artifact_id, current_commit_sha,
            current_workspace_id, current_snapshot_digest, current_manifest_id,
            latest_report_artifact_id, latest_verdict, revision,
            created_at, updated_at
         ) VALUES (?, ?, ?, ?, 1, NULL, NULL, NULL, NULL, NULL, NULL,
                   NULL, NULL, 1, ?, ?)",
    )
    .bind(&control.line_id)
    .bind(&input.task_id)
    .bind(&control.reviewed_work_unit_id)
    .bind(&control.reviewed_work_unit_id)
    .bind(&input.created_at)
    .bind(&input.created_at)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;
    let gate_inserted = sqlx::query(
        "INSERT OR IGNORE INTO gate (
            id, task_id, gate_kind, scope_kind, scope_id,
            active_policy_revision, created_at
         ) VALUES (?, ?, ?, 'work_unit', ?, NULL, ?)",
    )
    .bind(&control.gate_id)
    .bind(&input.task_id)
    .bind(&control.gate_kind)
    .bind(&control.reviewed_work_unit_id)
    .bind(&input.created_at)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?
    .rows_affected()
        == 1;
    if !gate_inserted {
        return Ok(());
    }
    let policy_json = serde_json::json!({
        "schema_version": 1,
        "review": {
            "mode": "one_acceptable",
            "required_count": 1,
            "human_required": false,
            "review_line_id": control.line_id,
        }
    })
    .to_string();
    let policy_digest = hex::encode(Sha256::digest(policy_json.as_bytes()));
    sqlx::query(
        "INSERT INTO gate_policy_revision (
            gate_id, revision, schema_version, policy_json,
            policy_digest, created_at
         ) VALUES (?, 1, 1, ?, ?, ?)",
    )
    .bind(&control.gate_id)
    .bind(&policy_json)
    .bind(&policy_digest)
    .bind(&input.created_at)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;
    sqlx::query(
        "UPDATE gate SET active_policy_revision = 1
         WHERE id = ? AND active_policy_revision IS NULL",
    )
    .bind(&control.gate_id)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;
    for event in [
        CreateDomainEvent {
            id: crate::new_uuid_v4(),
            event_type: "gate.created".to_owned(),
            entity_type: "gate".to_owned(),
            entity_id: control.gate_id.clone(),
            actor_type: "human".to_owned(),
            actor_id: Some(input.created_by.clone()),
            scope_type: "task".to_owned(),
            scope_id: input.task_id.clone(),
            correlation_id: crate::new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("gate.created:{}", control.gate_id)),
            payload_json: serde_json::json!({
                "gate_id": control.gate_id,
                "gate_kind": control.gate_kind,
                "scope_kind": "work_unit",
                "scope_id": control.reviewed_work_unit_id,
            })
            .to_string(),
            created_at: input.created_at.clone(),
        },
        CreateDomainEvent {
            id: crate::new_uuid_v4(),
            event_type: "gate.policy_revised".to_owned(),
            entity_type: "gate".to_owned(),
            entity_id: format!("{}:policy:1", control.gate_id),
            actor_type: "human".to_owned(),
            actor_id: Some(input.created_by.clone()),
            scope_type: "task".to_owned(),
            scope_id: input.task_id.clone(),
            correlation_id: crate::new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("gate.policy_revised:{}:1", control.gate_id)),
            payload_json: serde_json::json!({
                "gate_id": control.gate_id,
                "revision": 1,
                "policy_digest": policy_digest,
            })
            .to_string(),
            created_at: input.created_at.clone(),
        },
    ] {
        DomainEventRepo::append_event_in_tx(db, tx, &event).await?;
    }
    Ok(())
}

async fn insert_materialized_work_unit(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    input: &MaterializeTaskCoordinationPlanInput,
    unit: &MaterializedWorkUnit,
    review_gate_id: Option<&str>,
    review_line_id: Option<&str>,
) -> Result<()> {
    let (actor_kind, actor_id) = coordination_actor_columns(unit.assigned_actor.as_ref());
    sqlx::query(
        "INSERT INTO work_unit (
            id, task_id, parent_work_unit_id, title, scope, status, role,
            assigned_actor_kind, assigned_actor_id, requires_integration,
            provenance_kind, provenance_id, provenance_actor_kind,
            review_approval_gate_id, review_approval_line_id,
            created_by_kind, created_by_id, version, created_at, updated_at,
            execution_instructions
         ) VALUES (?, ?, ?, ?, ?, 'open', ?, ?, ?, ?, 'artifact', ?, NULL,
                   ?, ?, 'human', ?, 1, ?, ?, ?)",
    )
    .bind(&unit.id)
    .bind(&input.task_id)
    .bind(unit.parent_work_unit_id.as_deref())
    .bind(&unit.title)
    .bind(&unit.scope)
    .bind(&unit.role)
    .bind(actor_kind.as_deref())
    .bind(actor_id.as_deref())
    .bind(i64::from(unit.requires_integration))
    .bind(&input.plan_artifact_id)
    .bind(review_gate_id)
    .bind(review_line_id)
    .bind(&input.created_by)
    .bind(&input.created_at)
    .bind(&input.created_at)
    .bind(unit.execution_instructions.as_deref())
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;
    for artifact in &unit.artifact_inputs {
        sqlx::query(
            "INSERT INTO work_unit_artifact_input (
                work_unit_id, task_id, artifact_id, artifact_digest,
                producer_execution_id, created_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&unit.id)
        .bind(&input.task_id)
        .bind(&artifact.artifact_id)
        .bind(&artifact.digest)
        .bind(&artifact.producer_execution_id)
        .bind(&input.created_at)
        .execute(&mut **tx)
        .await
        .map_err(check_error)?;
    }
    Ok(())
}

fn coordination_actor_columns(actor: Option<&ActorRef>) -> (Option<String>, Option<String>) {
    actor
        .map(|actor| (Some(actor.kind().to_string()), Some(actor.id().to_owned())))
        .unwrap_or((None, None))
}

fn map_task_coordination(row: SqliteRow) -> Result<TaskCoordination> {
    let actor_kind: Option<String> = row.try_get("coordinator_actor_kind")?;
    let actor_id: Option<String> = row.try_get("coordinator_actor_id")?;
    let coordinator_actor = match (actor_kind, actor_id) {
        (None, None) => None,
        (Some(kind), Some(id)) => Some(match kind.as_str() {
            "human" => ActorRef::Human(id),
            "agent" => ActorRef::Agent(id),
            _ => {
                return Err(DbError::Check(
                    "coordination Actor kind is invalid".to_owned(),
                ))
            }
        }),
        _ => {
            return Err(DbError::Check(
                "coordination Actor columns are incomplete".to_owned(),
            ))
        }
    };
    Ok(TaskCoordination {
        task_id: row.try_get("task_id")?,
        mode: parse_enum(row.try_get("mode")?)?,
        coordinator_task_role_id: row.try_get("coordinator_task_role_id")?,
        coordinator_actor,
        authorization_version: row.try_get("authorization_version")?,
        team_snapshot_json: row.try_get("team_snapshot_json")?,
        team_digest: row.try_get("team_digest")?,
        team_stale: row.try_get::<i64, _>("team_stale")? != 0,
        team_generation: row.try_get("team_generation")?,
        envelope_json: row.try_get("envelope_json")?,
        envelope_digest: row.try_get("envelope_digest")?,
        requested_by: row.try_get("requested_by")?,
        turn_count: row.try_get("turn_count")?,
        quality_request_count: row.try_get("quality_request_count")?,
        max_turns: row.try_get("max_turns")?,
        max_quality_requests: row.try_get("max_quality_requests")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        revoked_at: row.try_get("revoked_at")?,
        revoke_reason: row.try_get("revoke_reason")?,
    })
}

fn map_task_coordination_receipt(row: SqliteRow) -> Result<TaskCoordinationReceipt> {
    Ok(TaskCoordinationReceipt {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        operation: parse_enum(row.try_get("operation")?)?,
        idempotency_key: row.try_get("idempotency_key")?,
        request_digest: row.try_get("request_digest")?,
        authorization_version: row.try_get("authorization_version")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_task_coordination_request(row: SqliteRow) -> Result<TaskCoordinationRequest> {
    Ok(TaskCoordinationRequest {
        id: row.try_get("id")?,
        kind: parse_enum(row.try_get("kind")?)?,
        review_report_artifact_id: row.try_get("review_report_artifact_id")?,
        review_report_digest: row.try_get("review_report_digest")?,
        review_producer_execution_id: row.try_get("review_producer_execution_id")?,
        task_id: row.try_get("task_id")?,
        authorization_version: row.try_get("authorization_version")?,
        mode: parse_enum(row.try_get("mode")?)?,
        brief_json: row.try_get("brief_json")?,
        objective_digest: row.try_get("objective_digest")?,
        request_digest: row.try_get("request_digest")?,
        idempotency_key: row.try_get("idempotency_key")?,
        state: parse_enum(row.try_get("state")?)?,
        wake_id: row.try_get("wake_id")?,
        execution_id: row.try_get("execution_id")?,
        plan_artifact_id: row.try_get("plan_artifact_id")?,
        failure_code: row.try_get("failure_code")?,
        failure_message: row.try_get("failure_message")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn map_task_coordination_plan(row: SqliteRow) -> Result<TaskCoordinationPlan> {
    Ok(TaskCoordinationPlan {
        artifact_id: row.try_get("artifact_id")?,
        task_id: row.try_get("task_id")?,
        request_id: row.try_get("request_id")?,
        authorization_version: row.try_get("authorization_version")?,
        plan_digest: row.try_get("plan_digest")?,
        state: parse_enum(row.try_get("state")?)?,
        proposal_id: row.try_get("proposal_id")?,
        decision_id: row.try_get("decision_id")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_task_coordination_materialization(
    row: SqliteRow,
) -> Result<TaskCoordinationMaterialization> {
    Ok(TaskCoordinationMaterialization {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        request_id: row.try_get("request_id")?,
        plan_artifact_id: row.try_get("plan_artifact_id")?,
        plan_digest: row.try_get("plan_digest")?,
        authorization_version: row.try_get("authorization_version")?,
        task_version_before: row.try_get("task_version_before")?,
        task_version_after: row.try_get("task_version_after")?,
        work_unit_ids_json: row.try_get("work_unit_ids_json")?,
        receipt_digest: row.try_get("receipt_digest")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_quality_request(row: SqliteRow) -> Result<TaskQualityRequest> {
    let actor_kind: Option<String> = row.try_get("target_actor_kind")?;
    let actor_id: Option<String> = row.try_get("target_actor_id")?;
    let target_actor = match (actor_kind, actor_id) {
        (None, None) => None,
        (Some(kind), Some(id)) => Some(match kind.as_str() {
            "human" => ActorRef::Human(id),
            "agent" => ActorRef::Agent(id),
            _ => return Err(DbError::Check("quality target kind is invalid".to_owned())),
        }),
        _ => {
            return Err(DbError::Check(
                "quality target Actor columns are incomplete".to_owned(),
            ))
        }
    };
    let created_by_kind: String = row.try_get("created_by_kind")?;
    let created_by_id: String = row.try_get("created_by_id")?;
    let created_by = match created_by_kind.as_str() {
        "human" => ActorRef::Human(created_by_id),
        "agent" => ActorRef::Agent(created_by_id),
        _ => return Err(DbError::Check("quality creator kind is invalid".to_owned())),
    };
    Ok(TaskQualityRequest {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        source_kind: row.try_get("source_kind")?,
        source_id: row.try_get("source_id")?,
        category: parse_enum(row.try_get("category")?)?,
        target_kind: parse_enum(row.try_get("target_kind")?)?,
        target_task_role_id: row.try_get("target_task_role_id")?,
        target_actor,
        summary: row.try_get("summary")?,
        findings_json: row.try_get("findings_json")?,
        resolution_condition: row.try_get("resolution_condition")?,
        state: parse_enum(row.try_get("state")?)?,
        handoff_id: row.try_get("handoff_id")?,
        work_unit_id: row.try_get("work_unit_id")?,
        coordination_request_id: row.try_get("coordination_request_id")?,
        budget_round: row.try_get("budget_round")?,
        source_digest: row.try_get("source_digest")?,
        request_digest: row.try_get("request_digest")?,
        idempotency_key: row.try_get("idempotency_key")?,
        created_by,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        resolved_at: row.try_get("resolved_at")?,
        resolution_kind: row.try_get("resolution_kind")?,
        resolution_ref: row.try_get("resolution_ref")?,
        resolution_digest: row.try_get("resolution_digest")?,
        route_id: row.try_get("route_id")?,
        route_round: row.try_get("route_round")?,
        route_limit: row.try_get("route_limit")?,
        source_task_role_id: row.try_get("source_task_role_id")?,
        requires_human_reason_code: row.try_get("requires_human_reason_code")?,
        requires_human_reason: row.try_get("requires_human_reason")?,
        cycle_request_ids_json: row.try_get("cycle_request_ids_json")?,
        cycle_task_role_ids_json: row.try_get("cycle_task_role_ids_json")?,
    })
}

fn map_intervention(row: SqliteRow) -> Result<TaskIntervention> {
    Ok(TaskIntervention {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        kind: parse_enum(row.try_get("kind")?)?,
        state: parse_enum(row.try_get("state")?)?,
        source_kind: row.try_get("source_kind")?,
        source_id: row.try_get("source_id")?,
        payload_json: row.try_get("payload_json")?,
        request_digest: row.try_get("request_digest")?,
        idempotency_key: row.try_get("idempotency_key")?,
        expected_task_version: row.try_get("expected_task_version")?,
        expected_coordination_version: row.try_get("expected_coordination_version")?,
        proposal_id: row.try_get("proposal_id")?,
        decision_id: row.try_get("decision_id")?,
        coordination_request_id: row.try_get("coordination_request_id")?,
        failure_code: row.try_get("failure_code")?,
        failure_message: row.try_get("failure_message")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn append_optional_event(
    db: &SqliteDb,
    transaction: &mut Transaction<'_, Sqlite>,
    event: Option<&CreateDomainEvent>,
) -> Result<Option<DomainEvent>> {
    match event {
        Some(event) => Ok(Some(
            DomainEventRepo::append_event_in_tx(db, transaction, event).await?,
        )),
        None => Ok(None),
    }
}

fn check_error(error: sqlx::Error) -> DbError {
    if let Some(mapped) = project_plan_batch_write_error(&error) {
        return mapped;
    }
    if let sqlx::Error::Database(database_error) = &error {
        let message = database_error.message().to_ascii_lowercase();
        if message.contains("unique constraint failed") {
            return DbError::IdempotencyConflict;
        }
        if message.contains("foreign key constraint failed") {
            return DbError::NotFound;
        }
        if message.contains("constraint failed") || message.contains("invalid") {
            return DbError::Check(database_error.message().to_owned());
        }
    }
    DbError::Sqlx(error)
}

#[async_trait]
impl TaskCoordinationRepo for SqliteDb {
    async fn get_task_coordination(&self, task_id: &str) -> Result<Option<TaskCoordination>> {
        sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination)
            .transpose()
    }

    async fn get_task_coordination_receipt(
        &self,
        task_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<TaskCoordinationReceipt>> {
        sqlx::query(
            "SELECT * FROM task_coordination_receipt
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(task_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        .map(map_task_coordination_receipt)
        .transpose()
    }

    async fn authorize_task_coordination(
        &self,
        input: AuthorizeTaskCoordinationInput,
        expected_task_version: i64,
    ) -> Result<TaskCoordinationWrite> {
        authorize_task_coordination_write(self, input, expected_task_version, None).await
    }

    async fn authorize_project_plan_batch_task_coordination(
        &self,
        input: AuthorizeTaskCoordinationInput,
        expected_task_version: i64,
        binding: ProjectPlanBatchAuthorizationBinding,
    ) -> Result<TaskCoordinationWrite> {
        authorize_task_coordination_write(self, input, expected_task_version, Some(binding)).await
    }

    async fn has_project_plan_batch_coordination_authorization(
        &self,
        batch_id: &str,
        task_id: &str,
        authorization_version: i64,
        expected_batch_version: i64,
        expected_graph_version: i64,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM project_plan_batch_coordination_authorization
                WHERE batch_id=? AND task_id=? AND authorization_version=?
                  AND expected_batch_version=? AND expected_graph_version=?
            )",
        )
        .bind(batch_id)
        .bind(task_id)
        .bind(authorization_version)
        .bind(expected_batch_version)
        .bind(expected_graph_version)
        .fetch_one(&self.pool)
        .await?)
    }

    async fn has_current_project_plan_batch_coordination_authorization(
        &self,
        task_id: &str,
    ) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM task_coordination coordination
                JOIN project_plan_batch_coordination_authorization origin
                  ON origin.task_id=coordination.task_id
                 AND origin.authorization_version=coordination.authorization_version
                WHERE coordination.task_id=?
            )",
        )
        .bind(task_id)
        .fetch_one(&self.pool)
        .await?)
    }

    async fn revoke_task_coordination(
        &self,
        input: RevokeTaskCoordinationInput,
        expected_task_version: i64,
    ) -> Result<TaskCoordinationWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM task_coordination_receipt
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_task_coordination_receipt)
        .transpose()?
        {
            if existing.request_digest != input.request_digest
                || existing.operation != TaskCoordinationOperation::Revoke
            {
                return Err(DbError::IdempotencyConflict);
            }
            let coordination = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
                .bind(&input.task_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(map_task_coordination)
                .transpose()?
                .ok_or(DbError::NotFound)?;
            tx.commit().await?;
            return Ok(TaskCoordinationWrite {
                coordination,
                receipt: existing,
                replayed: true,
                event: None,
            });
        }
        let task = sqlx::query("SELECT version FROM task WHERE id = ? AND deleted_at IS NULL")
            .bind(&input.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(DbError::NotFound)?;
        let actual_task_version: i64 = task.try_get("version")?;
        if actual_task_version != expected_task_version {
            return Err(DbError::TaskVersionConflict {
                expected: expected_task_version,
                actual: actual_task_version,
            });
        }
        let current = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
            .bind(&input.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if current.authorization_version != input.expected_coordination_version {
            return Err(DbError::VersionConflict);
        }
        sqlx::query(
            "INSERT INTO task_coordination_receipt (
                id, task_id, operation, idempotency_key, request_digest,
                authorization_version, created_at
             ) VALUES (?, ?, 'revoke', ?, ?, ?, ?)",
        )
        .bind(&input.receipt_id)
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .bind(&input.request_digest)
        .bind(current.authorization_version + 1)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "UPDATE task_coordination
             SET authorization_version = authorization_version + 1,
                 revoked_at = ?, revoke_reason = ?, requested_by = ?, updated_at = ?
             WHERE task_id = ? AND authorization_version = ?",
        )
        .bind(&input.created_at)
        .bind(input.reason.as_deref())
        .bind(&input.requested_by)
        .bind(&input.created_at)
        .bind(&input.task_id)
        .bind(input.expected_coordination_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let coordination = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
            .bind(&input.task_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination)?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &input.event).await?;
        tx.commit().await?;
        Ok(TaskCoordinationWrite {
            coordination,
            receipt: TaskCoordinationReceipt {
                id: input.receipt_id,
                task_id: input.task_id,
                operation: TaskCoordinationOperation::Revoke,
                idempotency_key: input.idempotency_key,
                request_digest: input.request_digest,
                authorization_version: input.expected_coordination_version + 1,
                created_at: input.created_at,
            },
            replayed: false,
            event: Some(event),
        })
    }

    async fn consume_coordination_turn_budget(&self, task_id: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE task_coordination
             SET turn_count = turn_count + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE task_id = ? AND turn_count < max_turns",
        )
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn refund_coordination_turn_budget(&self, task_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE task_coordination
             SET turn_count = max(turn_count - 1, 0),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE task_id = ?",
        )
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn create_task_coordination_request(
        &self,
        input: CreateTaskCoordinationRequestInput,
    ) -> Result<(TaskCoordinationRequest, Option<DomainEvent>)> {
        create_task_coordination_request_write(self, input, None).await
    }

    async fn create_project_plan_batch_coordination_request(
        &self,
        input: CreateTaskCoordinationRequestInput,
        binding: ProjectPlanBatchRequestBinding,
    ) -> Result<(TaskCoordinationRequest, Option<DomainEvent>)> {
        create_task_coordination_request_write(self, input, Some(binding)).await
    }

    async fn get_task_coordination_request(
        &self,
        id: &str,
    ) -> Result<Option<TaskCoordinationRequest>> {
        sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination_request)
            .transpose()
    }

    async fn get_task_coordination_request_by_key(
        &self,
        task_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<TaskCoordinationRequest>> {
        sqlx::query(
            "SELECT * FROM task_coordination_request
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(task_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        .map(map_task_coordination_request)
        .transpose()
    }

    async fn is_project_plan_batch_coordination_request(&self, request_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM project_plan_batch_coordination_request
                           WHERE request_id=?)",
        )
        .bind(request_id)
        .fetch_one(&self.pool)
        .await?)
    }

    async fn project_plan_batch_coordination_request_state(
        &self,
        request_id: &str,
    ) -> Result<Option<String>> {
        let Some(scope) = sqlx::query(
            "SELECT scope.batch_id,scope.project_id,scope.task_id,scope.authorization_version,
                    scope.authorized_by,scope.expected_graph_version,
                    scope.coordination_snapshot_digest,b.state
             FROM project_plan_batch_coordination_request scope
             JOIN project_plan_batch b ON b.id=scope.batch_id
             WHERE scope.request_id=?",
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };
        let batch_id: String = scope.try_get("batch_id")?;
        let project_id: String = scope.try_get("project_id")?;
        let task_id: String = scope.try_get("task_id")?;
        let authorization_version: i64 = scope.try_get("authorization_version")?;
        let authorized_by: String = scope.try_get("authorized_by")?;
        let expected_graph_version: i64 = scope.try_get("expected_graph_version")?;
        let snapshot_digest: String = scope.try_get("coordination_snapshot_digest")?;
        let batch_state: String = scope.try_get("state")?;
        let current: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM project_plan_batch b
                JOIN project_plan p ON p.project_id=b.project_id
                JOIN project_plan_batch_task bt ON bt.batch_id=b.id AND bt.task_id=?
                JOIN task t ON t.id=bt.task_id AND t.project_id=b.project_id AND t.deleted_at IS NULL
                JOIN task_coordination c ON c.task_id=t.id
                JOIN project_plan_batch_coordination_authorization a
                  ON a.batch_id=b.id AND a.task_id=t.id
                   AND a.authorization_version=?
                JOIN project_member m ON m.project_id=b.project_id
                  AND m.user_id=b.authorized_by AND m.role IN ('owner','admin')
                JOIN project owner_project ON owner_project.id=b.project_id
                JOIN project_member owner_membership ON owner_membership.project_id=b.project_id
                  AND owner_membership.user_id=owner_project.owner_id
                  AND owner_membership.role='owner'
                WHERE b.id=? AND b.project_id=? AND b.authorized_by=?
                  AND b.state IN ('running','paused') AND b.preparation_complete=1
                  AND b.graph_version=? AND p.version=b.graph_version
                  AND bt.state='started' AND bt.coordination_snapshot_digest=?
                  AND c.authorization_version=? AND c.mode='prepare_and_execute'
                  AND c.requested_by=b.authorized_by AND c.revoked_at IS NULL
                  AND c.team_stale=0 AND c.team_generation=a.team_generation
                  AND c.team_digest=a.team_digest AND c.envelope_digest=a.envelope_digest
                  AND c.coordinator_task_role_id=a.role_id
                  AND c.coordinator_actor_kind=a.actor_kind
                  AND c.coordinator_actor_id=a.actor_id
                  AND a.project_id=b.project_id AND a.authorized_by=b.authorized_by
                  AND a.expected_graph_version=b.graph_version AND a.mode=c.mode
                  AND a.task_title=t.title AND a.task_description IS t.description
                  AND a.task_repo_id IS t.repo_id AND a.task_parent_task_id IS t.parent_task_id
                  AND EXISTS(SELECT 1 FROM project_plan_batch_coordination_request request
                             WHERE request.request_id=? AND request.batch_id=b.id
                              AND request.task_id=t.id
                              AND request.authorization_version=c.authorization_version
                              AND request.coordination_snapshot_digest=bt.coordination_snapshot_digest)
            )",
        )
        .bind(&task_id)
        .bind(authorization_version)
        .bind(&batch_id)
        .bind(&project_id)
        .bind(&authorized_by)
        .bind(expected_graph_version)
        .bind(&snapshot_digest)
        .bind(authorization_version)
        .bind(request_id)
        .fetch_one(&self.pool)
        .await?;
        if !current {
            return Ok(Some("inactive".to_owned()));
        }
        Ok(Some(if batch_state == "paused" {
            "paused".to_owned()
        } else {
            "running".to_owned()
        }))
    }

    async fn link_task_coordination_request_wake(
        &self,
        request_id: &str,
        task_id: &str,
        wake_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE task_coordination_request
             SET wake_id = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ? AND task_id = ? AND wake_id IS NULL",
        )
        .bind(wake_id)
        .bind(request_id)
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_task_coordination_requests(
        &self,
        task_id: &str,
        limit: i64,
    ) -> Result<Vec<TaskCoordinationRequest>> {
        sqlx::query(
            "SELECT * FROM task_coordination_request
             WHERE task_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_task_coordination_request)
        .collect()
    }

    async fn get_task_coordination_request_by_wake(
        &self,
        wake_id: &str,
    ) -> Result<Option<TaskCoordinationRequest>> {
        sqlx::query("SELECT * FROM task_coordination_request WHERE wake_id = ?")
            .bind(wake_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination_request)
            .transpose()
    }

    async fn get_task_coordination_request_by_execution(
        &self,
        execution_id: &str,
    ) -> Result<Option<TaskCoordinationRequest>> {
        sqlx::query("SELECT * FROM task_coordination_request WHERE execution_id = ?")
            .bind(execution_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination_request)
            .transpose()
    }

    async fn update_task_coordination_request(
        &self,
        input: UpdateTaskCoordinationRequestState,
    ) -> Result<TaskCoordinationRequest> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut query =
            sqlx::QueryBuilder::<Sqlite>::new("UPDATE task_coordination_request SET state = ");
        query.push_bind(input.state.to_string());
        query
            .push(", version = version + 1, updated_at = ")
            .push_bind(&input.updated_at);
        if let Some(wake_id) = input.wake_id.as_deref() {
            query.push(", wake_id = ").push_bind(wake_id);
        }
        if let Some(execution_id) = input.execution_id.as_deref() {
            query.push(", execution_id = ").push_bind(execution_id);
        }
        if let Some(plan_artifact_id) = input.plan_artifact_id.as_deref() {
            query
                .push(", plan_artifact_id = ")
                .push_bind(plan_artifact_id);
        }
        if let Some(failure_code) = input.failure_code.as_ref() {
            query
                .push(", failure_code = ")
                .push_bind(failure_code.as_deref());
        }
        if let Some(failure_message) = input.failure_message.as_ref() {
            query
                .push(", failure_message = ")
                .push_bind(failure_message.as_deref());
        }
        if let Some(completed_at) = input.completed_at.as_deref() {
            query.push(", completed_at = ").push_bind(completed_at);
        }
        query
            .push(" WHERE id = ")
            .push_bind(&input.id)
            .push(" AND version = ")
            .push_bind(input.expected_version);
        let result = query.build().execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        append_optional_event(self, &mut tx, input.event.as_ref()).await?;
        let request = sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination_request)?;
        tx.commit().await?;
        Ok(request)
    }

    async fn complete_task_coordination_turn(
        &self,
        input: CompleteTaskCoordinationTurnInput,
    ) -> Result<TaskCoordinationPlanWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let request = sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(&input.request_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination_request)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if request.task_id != input.task_id
            || request.authorization_version != input.authorization_version
        {
            return Err(DbError::VersionConflict);
        }
        if matches!(
            request.state,
            TaskCoordinationRequestState::Cancelled | TaskCoordinationRequestState::Failed
        ) {
            return Err(DbError::InvalidTransition);
        }
        let existing_artifact: Option<String> = sqlx::query_scalar(
            "SELECT artifact_id FROM task_coordination_plan WHERE artifact_id = ?",
        )
        .bind(&input.artifact_id)
        .fetch_optional(&mut *tx)
        .await?;
        if existing_artifact.is_some() {
            return Err(DbError::IdempotencyConflict);
        }
        sqlx::query(
            "INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
             VALUES (?, ?, ?)",
        )
        .bind(&input.artifact_id)
        .bind(&input.producer_execution_id)
        .bind(&input.task_id)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "INSERT INTO artifact (
                id, task_id, kind, storage_kind, content, content_ref,
                metadata_json, digest, created_at
             ) VALUES (?, ?, 'work_unit_plan', 'inline', ?, NULL, ?, ?, ?)",
        )
        .bind(&input.artifact_id)
        .bind(&input.task_id)
        .bind(&input.artifact_content)
        .bind(&input.artifact_metadata_json)
        .bind(&input.plan_digest)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "INSERT INTO execution_artifact_output (
                execution_id, artifact_id, task_id, kind, digest, created_at
             ) VALUES (?, ?, ?, 'work_unit_plan', ?, ?)",
        )
        .bind(&input.producer_execution_id)
        .bind(&input.artifact_id)
        .bind(&input.task_id)
        .bind(&input.plan_digest)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "INSERT INTO task_coordination_plan (
                artifact_id, task_id, request_id, authorization_version,
                plan_digest, state, proposal_id, decision_id, version,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, 1, ?, ?)",
        )
        .bind(&input.artifact_id)
        .bind(&input.task_id)
        .bind(&input.request_id)
        .bind(input.authorization_version)
        .bind(&input.plan_digest)
        .bind(input.plan_state.to_string())
        .bind(input.proposal_id.as_deref())
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "UPDATE task_coordination_request
             SET state = ?, plan_artifact_id = ?, version = version + 1,
                 updated_at = ?, completed_at = ?,
                 failure_code = NULL, failure_message = NULL
             WHERE id = ? AND version = ?",
        )
        .bind(input.request_state.to_string())
        .bind(&input.artifact_id)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .bind(&input.request_id)
        .bind(request.version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        DomainEventRepo::append_event_in_tx(self, &mut tx, &input.artifact_event).await?;
        DomainEventRepo::append_event_in_tx(self, &mut tx, &input.plan_event).await?;
        let plan = sqlx::query("SELECT * FROM task_coordination_plan WHERE artifact_id = ?")
            .bind(&input.artifact_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination_plan)?;
        let request = sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(&input.request_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination_request)?;
        tx.commit().await?;
        Ok(TaskCoordinationPlanWrite { plan, request })
    }

    async fn get_task_coordination_plan(
        &self,
        artifact_id: &str,
    ) -> Result<Option<TaskCoordinationPlan>> {
        sqlx::query("SELECT * FROM task_coordination_plan WHERE artifact_id = ?")
            .bind(artifact_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination_plan)
            .transpose()
    }

    async fn list_task_coordination_plans(
        &self,
        task_id: &str,
        limit: i64,
    ) -> Result<Vec<TaskCoordinationPlan>> {
        sqlx::query(
            "SELECT * FROM task_coordination_plan
             WHERE task_id = ? ORDER BY created_at DESC, artifact_id DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_task_coordination_plan)
        .collect()
    }

    async fn update_task_coordination_plan_state(
        &self,
        artifact_id: &str,
        expected_version: i64,
        state: TaskCoordinationPlanState,
        proposal_id: Option<Option<String>>,
        decision_id: Option<Option<String>>,
        updated_at: &str,
        event: Option<CreateDomainEvent>,
    ) -> Result<TaskCoordinationPlan> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut query =
            sqlx::QueryBuilder::<Sqlite>::new("UPDATE task_coordination_plan SET state = ");
        query.push_bind(state.to_string());
        query
            .push(", version = version + 1, updated_at = ")
            .push_bind(updated_at);
        if let Some(proposal_id) = proposal_id.as_ref() {
            query
                .push(", proposal_id = ")
                .push_bind(proposal_id.as_deref());
        }
        if let Some(decision_id) = decision_id.as_ref() {
            query
                .push(", decision_id = ")
                .push_bind(decision_id.as_deref());
        }
        query
            .push(" WHERE artifact_id = ")
            .push_bind(artifact_id)
            .push(" AND version = ")
            .push_bind(expected_version);
        let result = query.build().execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        append_optional_event(self, &mut tx, event.as_ref()).await?;
        let plan = sqlx::query("SELECT * FROM task_coordination_plan WHERE artifact_id = ?")
            .bind(artifact_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination_plan)?;
        tx.commit().await?;
        Ok(plan)
    }

    async fn materialize_task_coordination_plan(
        &self,
        input: MaterializeTaskCoordinationPlanInput,
    ) -> Result<TaskCoordinationMaterialization> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM task_coordination_materialization WHERE plan_artifact_id = ?",
        )
        .bind(&input.plan_artifact_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_task_coordination_materialization)
        .transpose()?
        {
            if existing.plan_digest != input.plan_digest {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(existing);
        }
        let plan = sqlx::query("SELECT * FROM task_coordination_plan WHERE artifact_id = ?")
            .bind(&input.plan_artifact_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination_plan)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if plan.task_id != input.task_id
            || plan.request_id != input.request_id
            || plan.plan_digest != input.plan_digest
            || plan.authorization_version != input.authorization_version
            || plan.version != input.expected_plan_version
            || plan.state != TaskCoordinationPlanState::Approved
        {
            return Err(DbError::VersionConflict);
        }
        let request = sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(&input.request_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination_request)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if request.task_id != input.task_id
            || request.version != input.expected_request_version
            || request.authorization_version != input.authorization_version
            || request.plan_artifact_id.as_deref() != Some(input.plan_artifact_id.as_str())
            || request.state == TaskCoordinationRequestState::Cancelled
        {
            return Err(DbError::VersionConflict);
        }
        let task_version: i64 =
            sqlx::query_scalar("SELECT version FROM task WHERE id = ? AND deleted_at IS NULL")
                .bind(&input.task_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(DbError::NotFound)?;
        if task_version != input.task_version_before {
            return Err(DbError::TaskVersionConflict {
                expected: input.task_version_before,
                actual: task_version,
            });
        }
        // Revalidate the authorization that protects this exact effect. The
        // service preflight is not sufficient: another consumer may revoke,
        // re-authorize, or mutate the team between that read and this
        // BEGIN IMMEDIATE. The caller's Project write authority is rechecked
        // under the same lock as the WorkUnit inserts.
        let coordination = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
            .bind(&input.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if coordination.revoked_at.is_some()
            || coordination.mode == crate::TaskCoordinationMode::Manual
            || coordination.authorization_version != input.authorization_version
            || coordination.team_digest != input.expected_team_digest
            || coordination.team_stale
            || coordination.coordinator_task_role_id.is_none()
            || coordination.coordinator_actor.is_none()
        {
            return Err(DbError::VersionConflict);
        }
        let lifecycle_state: Option<String> =
            sqlx::query_scalar("SELECT state FROM task_lifecycle WHERE task_id = ?")
                .bind(&input.task_id)
                .fetch_optional(&mut *tx)
                .await?;
        let lifecycle_state = lifecycle_state.ok_or(DbError::NotFound)?;
        if matches!(lifecycle_state.as_str(), "cancelled" | "done" | "merging") {
            return Err(DbError::InvalidTransition);
        }
        let dispatch_state: Option<String> =
            sqlx::query_scalar("SELECT state FROM task_dispatch WHERE task_id = ?")
                .bind(&input.task_id)
                .fetch_optional(&mut *tx)
                .await?;
        let dispatch_state = dispatch_state.ok_or(DbError::NotFound)?;
        if dispatch_state == "paused" {
            return Err(DbError::TaskDispatchPaused);
        }
        if dispatch_state == "cancelled" {
            return Err(DbError::TaskDispatchCancelled);
        }
        let caller_can_write: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1
                FROM task current_task
                JOIN project_member member
                  ON member.project_id = current_task.project_id
                 AND member.user_id = ?
                 AND member.role IN ('owner', 'admin', 'member')
                WHERE current_task.id = ? AND current_task.deleted_at IS NULL
            )",
        )
        .bind(&input.created_by)
        .bind(&input.task_id)
        .fetch_one(&mut *tx)
        .await?;
        if !caller_can_write {
            return Err(DbError::ProjectWriteRequired);
        }
        // A formal review WorkUnit materializes its durable review line and a
        // WorkUnit-scoped approval Gate. Any WU that directly depends on the
        // review consumes that Gate, so it cannot start merely because the
        // reviewer ran: the latest review-line verdict must pass.
        let mut review_controls: BTreeMap<String, MaterializedReviewControl> = BTreeMap::new();
        for unit in &input.work_units {
            let Some(reviewed_work_unit_id) = unit.review_subject_work_unit_id.as_deref() else {
                continue;
            };
            let line_id: Option<String> = sqlx::query_scalar(
                "SELECT id FROM work_unit_review_line
                 WHERE task_id = ? AND root_work_unit_id = ?",
            )
            .bind(&input.task_id)
            .bind(reviewed_work_unit_id)
            .fetch_optional(&mut *tx)
            .await?;
            let gate_kind = format!("review_approval:{reviewed_work_unit_id}");
            let gate_id: Option<String> = sqlx::query_scalar(
                "SELECT id FROM gate
                 WHERE task_id = ? AND gate_kind = ?
                   AND scope_kind = 'work_unit' AND scope_id = ?",
            )
            .bind(&input.task_id)
            .bind(&gate_kind)
            .bind(reviewed_work_unit_id)
            .fetch_optional(&mut *tx)
            .await?;
            review_controls.insert(
                unit.id.clone(),
                MaterializedReviewControl {
                    reviewed_work_unit_id: reviewed_work_unit_id.to_owned(),
                    line_id: line_id
                        .unwrap_or_else(|| deterministic_review_line_id(reviewed_work_unit_id)),
                    gate_id: gate_id
                        .unwrap_or_else(|| deterministic_review_gate_id(reviewed_work_unit_id)),
                    gate_kind,
                },
            );
        }
        let mut consumer_controls: BTreeMap<String, (String, String, String)> = BTreeMap::new();
        for unit in &input.work_units {
            let mut controls = unit
                .depends_on_work_unit_ids
                .iter()
                .filter_map(|dependency| review_controls.get(dependency))
                .map(|control| {
                    (
                        control.reviewed_work_unit_id.clone(),
                        control.line_id.clone(),
                        control.gate_id.clone(),
                    )
                })
                .collect::<Vec<_>>();
            controls.sort();
            controls.dedup();
            if controls.len() > 1 {
                return Err(DbError::Check(
                    "a WorkUnit cannot depend on multiple review approval lines".to_owned(),
                ));
            }
            if let Some(control) = controls.first() {
                consumer_controls.insert(unit.id.clone(), control.clone());
            }
        }
        // Materialization has two edges per review: the reviewed WorkUnit must
        // exist before its review line/Gate, and that control must exist before
        // a consumer is inserted. Consecutive reviews (I1 -> R1 -> I2 -> R2)
        // are a valid DAG and must be inserted in that dependency order.
        let mut inserted: BTreeSet<String> = BTreeSet::new();
        let mut created_controls: BTreeSet<String> = BTreeSet::new();
        while inserted.len() < input.work_units.len() {
            let mut progress = false;
            for unit in &input.work_units {
                if inserted.contains(&unit.id) {
                    continue;
                }
                if let Some(parent_id) = unit.parent_work_unit_id.as_deref() {
                    if !inserted.contains(parent_id) {
                        continue;
                    }
                }
                if let Some((reviewed_id, _, _)) = consumer_controls.get(&unit.id) {
                    if !inserted.contains(reviewed_id) {
                        continue;
                    }
                }
                let (review_line_id, review_gate_id) = consumer_controls
                    .get(&unit.id)
                    .map(|(_, line, gate)| (Some(line.as_str()), Some(gate.as_str())))
                    .unwrap_or((None, None));
                insert_materialized_work_unit(
                    &mut tx,
                    &input,
                    unit,
                    review_gate_id,
                    review_line_id,
                )
                .await?;
                inserted.insert(unit.id.clone());
                progress = true;
                if !created_controls.contains(&unit.id) {
                    for control in review_controls
                        .values()
                        .filter(|control| control.reviewed_work_unit_id == unit.id)
                    {
                        insert_review_control(self, &mut tx, &input, control).await?;
                    }
                    created_controls.insert(unit.id.clone());
                }
            }
            if !progress {
                return Err(DbError::Check(
                    "review approval controls are cyclic or inconsistent".to_owned(),
                ));
            }
        }
        for unit in &input.work_units {
            for dependency in &unit.depends_on_work_unit_ids {
                sqlx::query(
                    "INSERT INTO work_unit_dependency (
                        task_id, work_unit_id, depends_on_work_unit_id,
                        created_by_kind, created_by_id, created_at
                     ) VALUES (?, ?, ?, 'human', ?, ?)",
                )
                .bind(&input.task_id)
                .bind(&unit.id)
                .bind(dependency)
                .bind(&input.created_by)
                .bind(&input.created_at)
                .execute(&mut *tx)
                .await
                .map_err(check_error)?;
            }
        }
        for unit in &input.work_units {
            if let Some(reviewed_work_unit_id) = unit.review_subject_work_unit_id.as_deref() {
                sqlx::query(
                    "INSERT INTO work_unit_review_subject (
                        review_work_unit_id, task_id, reviewed_work_unit_id,
                        reviewed_work_unit_version, result_execution_id,
                        result_artifact_id, result_commit_sha, result_workspace_id,
                        workspace_snapshot_digest, created_at
                     ) VALUES (?, ?, ?, NULL, NULL, NULL, NULL, NULL, NULL, ?)",
                )
                .bind(&unit.id)
                .bind(&input.task_id)
                .bind(reviewed_work_unit_id)
                .bind(&input.created_at)
                .execute(&mut *tx)
                .await
                .map_err(check_error)?;
            }
        }
        let work_unit_ids = input
            .work_units
            .iter()
            .map(|unit| unit.id.clone())
            .collect::<Vec<_>>();
        let work_unit_ids_json = serde_json::to_string(&work_unit_ids)
            .map_err(|error| DbError::Check(format!("serialize WorkUnit IDs: {error}")))?;
        sqlx::query(
            "INSERT INTO task_coordination_materialization (
                id, task_id, request_id, plan_artifact_id, plan_digest,
                authorization_version, task_version_before, task_version_after,
                work_unit_ids_json, receipt_digest, created_by, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.receipt_id)
        .bind(&input.task_id)
        .bind(&input.request_id)
        .bind(&input.plan_artifact_id)
        .bind(&input.plan_digest)
        .bind(input.authorization_version)
        .bind(input.task_version_before)
        .bind(input.task_version_before)
        .bind(&work_unit_ids_json)
        .bind(&input.receipt_digest)
        .bind(&input.created_by)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let plan_update = sqlx::query(
            "UPDATE task_coordination_plan
             SET state = 'materialized', version = version + 1, updated_at = ?
             WHERE artifact_id = ? AND version = ? AND state = 'approved'",
        )
        .bind(&input.created_at)
        .bind(&input.plan_artifact_id)
        .bind(input.expected_plan_version)
        .execute(&mut *tx)
        .await?;
        if plan_update.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        let request_update = sqlx::query(
            "UPDATE task_coordination_request
             SET state = 'materialized', version = version + 1, updated_at = ?,
                 completed_at = ?, failure_code = NULL, failure_message = NULL
             WHERE id = ? AND version = ? AND state != 'cancelled'",
        )
        .bind(&input.created_at)
        .bind(&input.created_at)
        .bind(&input.request_id)
        .bind(input.expected_request_version)
        .execute(&mut *tx)
        .await?;
        if request_update.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        for event in &input.events {
            DomainEventRepo::append_event_in_tx(self, &mut tx, event).await?;
        }
        let receipt = sqlx::query(
            "SELECT * FROM task_coordination_materialization WHERE plan_artifact_id = ?",
        )
        .bind(&input.plan_artifact_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(DbError::from)
        .and_then(map_task_coordination_materialization)?;
        tx.commit().await?;
        Ok(receipt)
    }

    async fn get_task_coordination_materialization(
        &self,
        plan_artifact_id: &str,
    ) -> Result<Option<TaskCoordinationMaterialization>> {
        sqlx::query("SELECT * FROM task_coordination_materialization WHERE plan_artifact_id = ?")
            .bind(plan_artifact_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination_materialization)
            .transpose()
    }

    async fn create_task_quality_request(
        &self,
        input: CreateTaskQualityRequestInput,
    ) -> Result<TaskQualityWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM task_quality_request WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_quality_request)
        .transpose()?
        {
            if existing.request_digest != input.request_digest {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(TaskQualityWrite {
                request: existing,
                event: None,
                replayed: true,
            });
        }
        let route_identity = sqlx::query(
            "SELECT route_kind, route_ref, request_count, budget_used, max_requests
             FROM task_quality_route WHERE task_id = ? AND route_id = ?",
        )
        .bind(&input.task_id)
        .bind(&input.route_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(route) = route_identity.as_ref() {
            let route_kind: String = route.try_get("route_kind")?;
            let route_ref: String = route.try_get("route_ref")?;
            if route_kind != input.route_kind || route_ref != input.route_ref {
                return Err(DbError::Check(
                    "quality route identity conflicts with its durable source".to_owned(),
                ));
            }
        } else {
            sqlx::query(
                "INSERT INTO task_quality_route (
                    task_id, route_id, route_kind, route_ref, request_count,
                    budget_used, max_requests, created_at, updated_at
                 ) VALUES (?, ?, ?, ?, 0, 0,
                    COALESCE((SELECT max_route_requests FROM task_autonomy_limits WHERE task_id=?), 4), ?, ?)",
            )
            .bind(&input.task_id)
            .bind(&input.route_id)
            .bind(&input.route_kind)
            .bind(&input.route_ref)
            .bind(&input.task_id)
            .bind(&input.created_at)
            .bind(&input.created_at)
            .execute(&mut *tx)
            .await
            .map_err(check_error)?;
        }
        let route = sqlx::query(
            "SELECT request_count, budget_used, max_requests
             FROM task_quality_route WHERE task_id = ? AND route_id = ?",
        )
        .bind(&input.task_id)
        .bind(&input.route_id)
        .fetch_one(&mut *tx)
        .await?;
        let route_request_count: i64 = route.try_get("request_count")?;
        let route_budget_used: i64 = route.try_get("budget_used")?;
        let route_limit: i64 = route.try_get("max_requests")?;
        let (global_count, global_limit): (i64, i64) = sqlx::query_as(
            "SELECT quality_request_count, max_quality_requests
             FROM task_coordination WHERE task_id = ?",
        )
        .bind(&input.task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let global_available = global_count < global_limit;
        let route_available = route_budget_used < route_limit;
        let route_round = route_request_count + 1;
        let budget_round = (global_count + if global_available { 1 } else { 0 }).max(1);

        let pending_human_request: Option<String> = sqlx::query_scalar(
            "SELECT id FROM task_quality_request
             WHERE task_id = ? AND route_id = ? AND state = 'requires_human'
             ORDER BY route_round, id LIMIT 1",
        )
        .bind(&input.task_id)
        .bind(&input.route_id)
        .fetch_optional(&mut *tx)
        .await?;
        let previous_roles: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, target_task_role_id FROM task_quality_request
             WHERE task_id = ? AND route_id = ? AND state IN (
                 'requested', 'notified', 'work_created', 'running'
             ) AND target_task_role_id IS NOT NULL
             ORDER BY route_round DESC, id DESC LIMIT 2",
        )
        .bind(&input.task_id)
        .bind(&input.route_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut cycle_request_ids = Vec::new();
        let mut cycle_task_role_ids = Vec::new();
        if let (Some(target_role_id), [newest, oldest]) = (
            input.target_task_role_id.as_deref(),
            previous_roles.as_slice(),
        ) {
            if let (Some(oldest_role), Some(newest_role)) =
                (oldest.1.as_deref(), newest.1.as_deref())
            {
                if oldest_role != newest_role && oldest_role == target_role_id {
                    cycle_request_ids = vec![oldest.0.clone(), newest.0.clone(), input.id.clone()];
                    cycle_task_role_ids = vec![
                        oldest_role.to_owned(),
                        newest_role.to_owned(),
                        target_role_id.to_owned(),
                    ];
                }
            }
        }

        let (reason_code, reason) = match (
            input.initial_requires_human_reason_code.as_deref(),
            input.initial_requires_human_reason.as_deref(),
        ) {
            (Some(code), Some(reason)) => (Some(code.to_owned()), Some(reason.to_owned())),
            _ if input.state == TaskQualityState::RequiresHuman => (
                Some("coordination.requires_human".to_owned()),
                Some("this quality request requires a Human decision".to_owned()),
            ),
            _ if !global_available => (
                Some("coordination.budget_exhausted".to_owned()),
                Some(format!(
                    "Task quality request budget exhausted at {global_count} of {global_limit}; Human routing is required"
                )),
            ),
            _ if !route_available => (
                Some("coordination.quality_route_budget_exhausted".to_owned()),
                Some(format!(
                    "quality route budget exhausted at {route_budget_used} of {route_limit}; Human routing is required"
                )),
            ),
            _ if pending_human_request.is_some() => (
                Some("coordination.quality_route_human_pending".to_owned()),
                Some(format!(
                    "quality route is waiting for Human resolution of request {}",
                    pending_human_request.as_deref().unwrap_or_default()
                )),
            ),
            _ if !cycle_request_ids.is_empty() => (
                Some("coordination.quality_route_ping_pong".to_owned()),
                Some(format!(
                    "pending quality targets alternate between TaskRoles {} and {} and return to the first role",
                    cycle_task_role_ids.first().map(String::as_str).unwrap_or_default(),
                    cycle_task_role_ids.get(1).map(String::as_str).unwrap_or_default()
                )),
            ),
            _ => (None, None),
        };
        let state = if reason_code.is_some() {
            TaskQualityState::RequiresHuman
        } else {
            input.state
        };

        sqlx::query(
            "UPDATE task_quality_route
             SET request_count = request_count + 1,
                 budget_used = budget_used + CASE WHEN budget_used < max_requests THEN 1 ELSE 0 END,
                 updated_at = ?
             WHERE task_id = ? AND route_id = ?",
        )
        .bind(&input.created_at)
        .bind(&input.task_id)
        .bind(&input.route_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE task_coordination
             SET quality_request_count = quality_request_count + CASE
                     WHEN quality_request_count < max_quality_requests THEN 1 ELSE 0 END,
                 updated_at = ?
             WHERE task_id = ?",
        )
        .bind(&input.created_at)
        .bind(&input.task_id)
        .execute(&mut *tx)
        .await?;
        let (actor_kind, actor_id) = coordination_actor_columns(input.target_actor.as_ref());
        sqlx::query(
            "INSERT INTO task_quality_request (
                id, task_id, source_kind, source_id, category, target_kind,
                target_task_role_id, target_actor_kind, target_actor_id, summary,
                findings_json, resolution_condition, state, handoff_id, work_unit_id,
                budget_round, source_digest, request_digest, idempotency_key,
                created_by_kind, created_by_id, version, created_at, updated_at,
                route_id, route_round, route_limit, source_task_role_id,
                requires_human_reason_code, requires_human_reason,
                cycle_request_ids_json, cycle_task_role_ids_json
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.source_kind)
        .bind(&input.source_id)
        .bind(input.category.to_string())
        .bind(input.target_kind.to_string())
        .bind(input.target_task_role_id.as_deref())
        .bind(actor_kind.as_deref())
        .bind(actor_id.as_deref())
        .bind(&input.summary)
        .bind(&input.findings_json)
        .bind(&input.resolution_condition)
        .bind(state.to_string())
        .bind(input.handoff_id.as_deref())
        .bind(input.work_unit_id.as_deref())
        .bind(budget_round)
        .bind(&input.source_digest)
        .bind(&input.request_digest)
        .bind(&input.idempotency_key)
        .bind(input.created_by.kind().to_string())
        .bind(input.created_by.id())
        .bind(&input.created_at)
        .bind(&input.created_at)
        .bind(&input.route_id)
        .bind(route_round)
        .bind(route_limit)
        .bind(input.source_task_role_id.as_deref())
        .bind(reason_code.as_deref())
        .bind(reason.as_deref())
        .bind(serde_json::to_string(&cycle_request_ids).map_err(|error| {
            DbError::Check(format!("quality route cycle requests cannot serialize: {error}"))
        })?)
        .bind(serde_json::to_string(&cycle_task_role_ids).map_err(|error| {
            DbError::Check(format!("quality route cycle roles cannot serialize: {error}"))
        })?)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let mut event_input = input.event;
        let mut event_payload: serde_json::Value = serde_json::from_str(&event_input.payload_json)
            .map_err(|error| {
                DbError::Check(format!("quality event payload is invalid: {error}"))
            })?;
        event_payload["route_id"] = serde_json::json!(input.route_id);
        event_payload["route_round"] = serde_json::json!(route_round);
        event_payload["route_limit"] = serde_json::json!(route_limit);
        event_payload["state"] = serde_json::json!(state.to_string());
        event_payload["requires_human_reason_code"] = serde_json::json!(reason_code);
        event_payload["cycle_request_ids"] = serde_json::json!(cycle_request_ids);
        event_payload["cycle_task_role_ids"] = serde_json::json!(cycle_task_role_ids);
        event_input.payload_json = event_payload.to_string();
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event_input).await?;
        let request = sqlx::query("SELECT * FROM task_quality_request WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_quality_request)?;
        tx.commit().await?;
        Ok(TaskQualityWrite {
            request,
            event: Some(event),
            replayed: false,
        })
    }

    async fn get_task_quality_request(&self, id: &str) -> Result<Option<TaskQualityRequest>> {
        sqlx::query("SELECT * FROM task_quality_request WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_quality_request)
            .transpose()
    }

    async fn get_task_quality_request_by_key(
        &self,
        task_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<TaskQualityRequest>> {
        sqlx::query(
            "SELECT * FROM task_quality_request
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(task_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        .map(map_quality_request)
        .transpose()
    }

    async fn list_task_quality_requests(
        &self,
        task_id: &str,
        limit: i64,
    ) -> Result<Vec<TaskQualityRequest>> {
        sqlx::query(
            "SELECT * FROM task_quality_request
             WHERE task_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_quality_request)
        .collect()
    }

    async fn update_task_quality_request_state(
        &self,
        input: UpdateTaskQualityRequestState,
    ) -> Result<TaskQualityRequest> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut query =
            sqlx::QueryBuilder::<Sqlite>::new("UPDATE task_quality_request SET state = ");
        query.push_bind(input.state.to_string());
        query
            .push(", version = version + 1, updated_at = ")
            .push_bind(&input.updated_at);
        if input.state == TaskQualityState::Resolved {
            query.push(", resolved_at = ").push_bind(&input.updated_at);
        }
        if let Some(handoff_id) = input.handoff_id.as_ref() {
            query
                .push(", handoff_id = ")
                .push_bind(handoff_id.as_deref());
        }
        if let Some(work_unit_id) = input.work_unit_id.as_ref() {
            query
                .push(", work_unit_id = ")
                .push_bind(work_unit_id.as_deref());
        }
        if let Some(coordination_request_id) = input.coordination_request_id.as_ref() {
            query
                .push(", coordination_request_id = ")
                .push_bind(coordination_request_id.as_deref());
        }
        if let Some(resolution_kind) = input.resolution_kind.as_ref() {
            query
                .push(", resolution_kind = ")
                .push_bind(resolution_kind.as_deref());
        }
        if let Some(resolution_ref) = input.resolution_ref.as_ref() {
            query
                .push(", resolution_ref = ")
                .push_bind(resolution_ref.as_deref());
        }
        if let Some(resolution_digest) = input.resolution_digest.as_ref() {
            query
                .push(", resolution_digest = ")
                .push_bind(resolution_digest.as_deref());
        }
        if let Some(reason_code) = input.requires_human_reason_code.as_ref() {
            query
                .push(", requires_human_reason_code = ")
                .push_bind(reason_code.as_deref());
        }
        if let Some(reason) = input.requires_human_reason.as_ref() {
            query
                .push(", requires_human_reason = ")
                .push_bind(reason.as_deref());
        }
        query
            .push(" WHERE id = ")
            .push_bind(&input.id)
            .push(" AND version = ")
            .push_bind(input.expected_version);
        let result = query.build().execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        append_optional_event(self, &mut tx, input.event.as_ref()).await?;
        let request = sqlx::query("SELECT * FROM task_quality_request WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_quality_request)?;
        tx.commit().await?;
        Ok(request)
    }

    async fn create_task_intervention(
        &self,
        input: CreateTaskInterventionInput,
    ) -> Result<TaskInterventionWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) =
            sqlx::query("SELECT * FROM task_intervention WHERE task_id = ? AND idempotency_key = ?")
                .bind(&input.task_id)
                .bind(&input.idempotency_key)
                .fetch_optional(&mut *tx)
                .await?
                .map(map_intervention)
                .transpose()?
        {
            if existing.request_digest != input.request_digest || existing.kind != input.kind {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(TaskInterventionWrite {
                intervention: existing,
                event: None,
                replayed: true,
            });
        }
        sqlx::query(
            "INSERT INTO task_intervention (
                id, task_id, kind, state, source_kind, source_id, payload_json,
                request_digest, idempotency_key, expected_task_version,
                expected_coordination_version, version, created_at, updated_at
             ) VALUES (?, ?, ?, 'recorded', ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.kind.to_string())
        .bind(&input.source_kind)
        .bind(&input.source_id)
        .bind(&input.payload_json)
        .bind(&input.request_digest)
        .bind(&input.idempotency_key)
        .bind(input.expected_task_version)
        .bind(input.expected_coordination_version)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &input.event).await?;
        let intervention = sqlx::query("SELECT * FROM task_intervention WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_intervention)?;
        tx.commit().await?;
        Ok(TaskInterventionWrite {
            intervention,
            event: Some(event),
            replayed: false,
        })
    }

    async fn get_task_intervention(&self, id: &str) -> Result<Option<TaskIntervention>> {
        sqlx::query("SELECT * FROM task_intervention WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_intervention)
            .transpose()
    }

    async fn list_task_interventions(
        &self,
        task_id: &str,
        limit: i64,
    ) -> Result<Vec<TaskIntervention>> {
        sqlx::query(
            "SELECT * FROM task_intervention
             WHERE task_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_intervention)
        .collect()
    }

    async fn update_task_intervention_state(
        &self,
        id: &str,
        expected_version: i64,
        state: TaskInterventionState,
        proposal_id: Option<Option<String>>,
        decision_id: Option<Option<String>>,
        coordination_request_id: Option<Option<String>>,
        failure_code: Option<Option<String>>,
        failure_message: Option<Option<String>>,
        updated_at: &str,
        event: Option<CreateDomainEvent>,
    ) -> Result<TaskIntervention> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut query = sqlx::QueryBuilder::<Sqlite>::new("UPDATE task_intervention SET state = ");
        query.push_bind(state.to_string());
        query
            .push(", version = version + 1, updated_at = ")
            .push_bind(updated_at);
        if let Some(proposal_id) = proposal_id.as_ref() {
            query
                .push(", proposal_id = ")
                .push_bind(proposal_id.as_deref());
        }
        if let Some(decision_id) = decision_id.as_ref() {
            query
                .push(", decision_id = ")
                .push_bind(decision_id.as_deref());
        }
        if let Some(request_id) = coordination_request_id.as_ref() {
            query
                .push(", coordination_request_id = ")
                .push_bind(request_id.as_deref());
        }
        if let Some(failure_code) = failure_code.as_ref() {
            query
                .push(", failure_code = ")
                .push_bind(failure_code.as_deref());
        }
        if let Some(failure_message) = failure_message.as_ref() {
            query
                .push(", failure_message = ")
                .push_bind(failure_message.as_deref());
        }
        query
            .push(" WHERE id = ")
            .push_bind(id)
            .push(" AND version = ")
            .push_bind(expected_version);
        let result = query.build().execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        append_optional_event(self, &mut tx, event.as_ref()).await?;
        let intervention = sqlx::query("SELECT * FROM task_intervention WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_intervention)?;
        tx.commit().await?;
        Ok(intervention)
    }
}
