use super::*;
use crate::{
    CollaborationWrite, CreateGate, CreateGatePolicyRevision, Gate, GateEvaluation,
    GateEvaluationInput, GateEvaluationWrite, GatePolicyRevision, GateRepo, Result,
    StoreGateEvaluation, TaskLifecycle, TaskLifecycleMigrationAudit, TaskLifecycleRepo,
    TaskLifecycleState, TaskLifecycleTransitionFact, TaskLifecycleTransitionWrite,
    TransitionTaskLifecycle,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::str::FromStr;

fn parse_gate_enum<T: FromStr<Err = String>>(value: String) -> Result<T> {
    value.parse().map_err(|error: String| DbError::Check(error))
}

pub(super) fn map_task_lifecycle(row: &sqlx::sqlite::SqliteRow) -> Result<TaskLifecycle> {
    Ok(TaskLifecycle {
        task_id: row.try_get("task_id")?,
        state: parse_gate_enum(row.try_get("state")?)?,
        version: row.try_get("version")?,
        reason_kind: row.try_get("reason_kind")?,
        reason_ref: row.try_get("reason_ref")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_lifecycle_audit(row: &sqlx::sqlite::SqliteRow) -> Result<TaskLifecycleMigrationAudit> {
    Ok(TaskLifecycleMigrationAudit {
        task_id: row.try_get("task_id")?,
        legacy_state: row.try_get("legacy_state")?,
        mapped_state: parse_gate_enum(row.try_get("mapped_state")?)?,
        mapping_status: row.try_get("mapping_status")?,
        reason_kind: row.try_get("reason_kind")?,
        reason_ref: row.try_get("reason_ref")?,
        task_state_config: row.try_get("legacy_task_state_config")?,
        workflow_definition: row.try_get("workflow_definition")?,
        details_json: row.try_get("details_json")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_gate(row: &sqlx::sqlite::SqliteRow) -> Result<Gate> {
    Ok(Gate {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        gate_kind: row.try_get("gate_kind")?,
        scope_kind: parse_gate_enum(row.try_get("scope_kind")?)?,
        scope_id: row.try_get("scope_id")?,
        active_policy_revision: row.try_get("active_policy_revision")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_policy(row: &sqlx::sqlite::SqliteRow) -> Result<GatePolicyRevision> {
    Ok(GatePolicyRevision {
        gate_id: row.try_get("gate_id")?,
        revision: row.try_get("revision")?,
        schema_version: row.try_get("schema_version")?,
        policy_json: row.try_get("policy_json")?,
        policy_digest: row.try_get("policy_digest")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_evaluation(row: &sqlx::sqlite::SqliteRow) -> Result<GateEvaluation> {
    Ok(GateEvaluation {
        id: row.try_get("id")?,
        gate_id: row.try_get("gate_id")?,
        task_id: row.try_get("task_id")?,
        policy_revision: row.try_get("policy_revision")?,
        outcome: parse_gate_enum(row.try_get("outcome")?)?,
        input_digest: row.try_get("input_digest")?,
        result_json: row.try_get("result_json")?,
        evaluated_at: row.try_get("evaluated_at")?,
    })
}

fn map_evaluation_input(row: &sqlx::sqlite::SqliteRow) -> Result<GateEvaluationInput> {
    Ok(GateEvaluationInput {
        evaluation_id: row.try_get("evaluation_id")?,
        ordinal: row.try_get("ordinal")?,
        input_kind: row.try_get("input_kind")?,
        input_id: row.try_get("input_id")?,
        input_version: row.try_get("input_version")?,
        input_digest: row.try_get("input_digest")?,
        producer_ref: row.try_get("producer_ref")?,
        subject_json: row.try_get("subject_json")?,
        status: row.try_get("status")?,
    })
}

fn lifecycle_projection(state: TaskLifecycleState) -> &'static str {
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

fn lifecycle_request_digest(input: &TransitionTaskLifecycle) -> String {
    let canonical = serde_json::json!({
        "expected_task_version": input.expected_task_version,
        "expected_lifecycle_version": input.expected_lifecycle_version,
        "expected_state": input.expected_state,
        "to_state": input.to_state,
        "cause_kind": input.cause_kind,
        "cause_ref": input.cause_ref,
        "reason_kind": input.reason_kind,
        "reason_ref": input.reason_ref,
        "gate_evaluation_id": input.gate_evaluation_id,
    });
    hex::encode(Sha256::digest(canonical.to_string().as_bytes()))
}

/// Record a lifecycle transition inside a caller-owned SQLite transaction.
/// Callers update the Task's remaining compatibility fields and projection in
/// the same transaction after this returns.
pub(super) async fn record_lifecycle_transition_in_tx(
    db: &SqliteDb,
    transaction: &mut Transaction<'_, Sqlite>,
    input: &TransitionTaskLifecycle,
) -> Result<(TaskLifecycle, DomainEvent)> {
    if input.idempotency_key.trim().is_empty() || input.idempotency_key.len() > 256 {
        return Err(DbError::Check(
            "invalid Task lifecycle idempotency key".to_owned(),
        ));
    }
    let (task_version, _project_id): (i64, String) =
        sqlx::query_as("SELECT version, project_id FROM task WHERE id = ? AND deleted_at IS NULL")
            .bind(&input.task_id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(DbError::NotFound)?;
    if task_version != input.expected_task_version {
        return Err(DbError::TaskVersionConflict {
            expected: input.expected_task_version,
            actual: task_version,
        });
    }
    let row = sqlx::query(
        "SELECT task_id, state, version, reason_kind, reason_ref, created_at, updated_at
         FROM task_lifecycle WHERE task_id = ?",
    )
    .bind(&input.task_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(DbError::NotFound)?;
    let current = map_task_lifecycle(&row)?;
    if current.version != input.expected_lifecycle_version || current.state != input.expected_state
    {
        return Err(DbError::VersionConflict);
    }
    let request_digest = lifecycle_request_digest(input);
    let event = DomainEventRepo::append_event_in_tx(db, transaction, &input.event).await?;
    sqlx::query(
        "INSERT INTO task_lifecycle_transition (
            id, task_id, idempotency_key, request_digest, expected_task_version,
            from_state, to_state,
            from_version, to_version, cause_kind, cause_ref, gate_evaluation_id,
            reason_kind, reason_ref, domain_event_id, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&input.id)
    .bind(&input.task_id)
    .bind(&input.idempotency_key)
    .bind(request_digest)
    .bind(input.expected_task_version)
    .bind(current.state.to_string())
    .bind(input.to_state.to_string())
    .bind(current.version)
    .bind(current.version + 1)
    .bind(&input.cause_kind)
    .bind(input.cause_ref.as_deref())
    .bind(input.gate_evaluation_id.as_deref())
    .bind(input.reason_kind.as_deref())
    .bind(input.reason_ref.as_deref())
    .bind(&event.id)
    .bind(&input.updated_at)
    .execute(&mut **transaction)
    .await?;
    let updated = sqlx::query(
        "UPDATE task_lifecycle
         SET state = ?, version = version + 1, reason_kind = ?, reason_ref = ?, updated_at = ?
         WHERE task_id = ? AND state = ? AND version = ?",
    )
    .bind(input.to_state.to_string())
    .bind(input.reason_kind.as_deref())
    .bind(input.reason_ref.as_deref())
    .bind(&input.updated_at)
    .bind(&input.task_id)
    .bind(current.state.to_string())
    .bind(current.version)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(DbError::VersionConflict);
    }
    let row = sqlx::query(
        "SELECT task_id, state, version, reason_kind, reason_ref, created_at, updated_at
         FROM task_lifecycle WHERE task_id = ?",
    )
    .bind(&input.task_id)
    .fetch_one(&mut **transaction)
    .await?;
    Ok((map_task_lifecycle(&row)?, event))
}

#[async_trait]
impl TaskLifecycleRepo for SqliteDb {
    async fn get_task_lifecycle(&self, task_id: &str) -> Result<Option<TaskLifecycle>> {
        sqlx::query(
            "SELECT task_id, state, version, reason_kind, reason_ref, created_at, updated_at
             FROM task_lifecycle WHERE task_id = ?",
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await?
        .as_ref()
        .map(map_task_lifecycle)
        .transpose()
    }

    async fn get_task_lifecycle_transition_fact(
        &self,
        transition_id: &str,
    ) -> Result<Option<TaskLifecycleTransitionFact>> {
        let row = sqlx::query(
            "SELECT id, task_id, from_state, to_state, from_version, to_version,
                    cause_kind, cause_ref, gate_evaluation_id, reason_kind, reason_ref,
                    domain_event_id, created_at
             FROM task_lifecycle_transition WHERE id = ?",
        )
        .bind(transition_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(TaskLifecycleTransitionFact {
                id: row.try_get("id")?,
                task_id: row.try_get("task_id")?,
                from_state: parse_gate_enum(row.try_get("from_state")?)?,
                to_state: parse_gate_enum(row.try_get("to_state")?)?,
                from_version: row.try_get("from_version")?,
                to_version: row.try_get("to_version")?,
                cause_kind: row.try_get("cause_kind")?,
                cause_ref: row.try_get("cause_ref")?,
                gate_evaluation_id: row.try_get("gate_evaluation_id")?,
                reason_kind: row.try_get("reason_kind")?,
                reason_ref: row.try_get("reason_ref")?,
                domain_event_id: row.try_get("domain_event_id")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .transpose()
    }

    async fn has_task_lifecycle_transition(
        &self,
        task_id: &str,
        idempotency_key: &str,
    ) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM task_lifecycle_transition
                WHERE task_id = ? AND idempotency_key = ?
            )",
        )
        .bind(task_id)
        .bind(idempotency_key)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    async fn get_task_lifecycle_transition(
        &self,
        identity: TaskLifecycleTransitionIdentity,
    ) -> Result<Option<TaskLifecycleTransitionWrite>> {
        let row = sqlx::query(
            "SELECT id, task_id, expected_task_version, to_state, cause_kind, cause_ref,
                    reason_kind, reason_ref, gate_evaluation_id, to_version, created_at
             FROM task_lifecycle_transition WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&identity.task_id)
        .bind(&identity.idempotency_key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            if row.try_get::<i64, _>("expected_task_version")? != identity.expected_task_version
                || parse_gate_enum::<TaskLifecycleState>(row.try_get("to_state")?)?
                    != identity.to_state
                || row.try_get::<String, _>("cause_kind")? != identity.cause_kind
                || row.try_get::<Option<String>, _>("cause_ref")? != identity.cause_ref
                || row.try_get::<Option<String>, _>("reason_kind")? != identity.reason_kind
                || row.try_get::<Option<String>, _>("reason_ref")? != identity.reason_ref
                || row.try_get::<Option<String>, _>("gate_evaluation_id")?
                    != identity.gate_evaluation_id
            {
                return Err(DbError::IdempotencyConflict);
            }
            Ok(TaskLifecycleTransitionWrite {
                lifecycle: TaskLifecycle {
                    task_id: row.try_get("task_id")?,
                    state: parse_gate_enum(row.try_get("to_state")?)?,
                    version: row.try_get("to_version")?,
                    reason_kind: row.try_get("reason_kind")?,
                    reason_ref: row.try_get("reason_ref")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("created_at")?,
                },
                transition_id: row.try_get("id")?,
                gate_evaluation_id: row.try_get("gate_evaluation_id")?,
                replayed: true,
                event: None,
            })
        })
        .transpose()
    }

    async fn list_lifecycle_migration_audit(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskLifecycleMigrationAudit>> {
        sqlx::query(
            "SELECT task_id, legacy_state, mapped_state, mapping_status, reason_kind, reason_ref,
                    legacy_task_state_config, workflow_definition, details_json, created_at
             FROM task_lifecycle_migration_audit WHERE task_id = ?",
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await?
        .as_ref()
        .map(map_lifecycle_audit)
        .transpose()
    }

    async fn transition_task_lifecycle(
        &self,
        input: TransitionTaskLifecycle,
    ) -> Result<TaskLifecycleTransitionWrite> {
        if input.idempotency_key.trim().is_empty() || input.idempotency_key.len() > 256 {
            return Err(DbError::Check(
                "invalid Task lifecycle idempotency key".to_owned(),
            ));
        }
        let request_digest = lifecycle_request_digest(&input);
        let mut transaction = self.pool.begin().await?;

        if let Some(row) = sqlx::query(
            "SELECT id, request_digest, task_id, expected_task_version, to_state,
                    to_version, reason_kind, reason_ref,
                    gate_evaluation_id, created_at
             FROM task_lifecycle_transition WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *transaction)
        .await?
        {
            if row.try_get::<String, _>("request_digest")? != request_digest {
                return Err(DbError::IdempotencyConflict);
            }
            let lifecycle = TaskLifecycle {
                task_id: row.try_get("task_id")?,
                state: parse_gate_enum(row.try_get("to_state")?)?,
                version: row.try_get("to_version")?,
                reason_kind: row.try_get("reason_kind")?,
                reason_ref: row.try_get("reason_ref")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("created_at")?,
            };
            let result = TaskLifecycleTransitionWrite {
                lifecycle,
                transition_id: row.try_get("id")?,
                gate_evaluation_id: row.try_get("gate_evaluation_id")?,
                replayed: true,
                event: None,
            };
            transaction.commit().await?;
            return Ok(result);
        }

        let (task_version, project_id): (i64, String) = sqlx::query_as(
            "SELECT version, project_id FROM task WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(&input.task_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(DbError::NotFound)?;
        if task_version != input.expected_task_version {
            return Err(DbError::TaskVersionConflict {
                expected: input.expected_task_version,
                actual: task_version,
            });
        }
        let current_row = sqlx::query(
            "SELECT task_id, state, version, reason_kind, reason_ref, created_at, updated_at
             FROM task_lifecycle WHERE task_id = ?",
        )
        .bind(&input.task_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(DbError::NotFound)?;
        let current = map_task_lifecycle(&current_row)?;
        if current.version != input.expected_lifecycle_version
            || current.state != input.expected_state
        {
            return Err(DbError::VersionConflict);
        }

        let event =
            DomainEventRepo::append_event_in_tx(self, &mut transaction, &input.event).await?;
        sqlx::query(
            "INSERT INTO task_lifecycle_transition (
                id, task_id, idempotency_key, request_digest, expected_task_version,
                from_state, to_state,
                from_version, to_version, cause_kind, cause_ref, gate_evaluation_id,
                reason_kind, reason_ref, domain_event_id, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .bind(&request_digest)
        .bind(input.expected_task_version)
        .bind(current.state.to_string())
        .bind(input.to_state.to_string())
        .bind(current.version)
        .bind(current.version + 1)
        .bind(&input.cause_kind)
        .bind(input.cause_ref.as_deref())
        .bind(input.gate_evaluation_id.as_deref())
        .bind(input.reason_kind.as_deref())
        .bind(input.reason_ref.as_deref())
        .bind(&event.id)
        .bind(&input.updated_at)
        .execute(&mut *transaction)
        .await?;

        let updated_lifecycle = sqlx::query(
            "UPDATE task_lifecycle
             SET state = ?, version = version + 1, reason_kind = ?, reason_ref = ?, updated_at = ?
             WHERE task_id = ? AND state = ? AND version = ?",
        )
        .bind(input.to_state.to_string())
        .bind(input.reason_kind.as_deref())
        .bind(input.reason_ref.as_deref())
        .bind(&input.updated_at)
        .bind(&input.task_id)
        .bind(current.state.to_string())
        .bind(current.version)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if updated_lifecycle != 1 {
            return Err(DbError::VersionConflict);
        }

        let updated_task = sqlx::query(
            "UPDATE task SET status = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND project_id = ? AND version = ? AND deleted_at IS NULL",
        )
        .bind(lifecycle_projection(input.to_state))
        .bind(&input.updated_at)
        .bind(&input.task_id)
        .bind(&project_id)
        .bind(input.expected_task_version)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if updated_task != 1 {
            return Err(DbError::TaskVersionConflict {
                expected: input.expected_task_version,
                actual: task_version,
            });
        }
        let lifecycle_row = sqlx::query(
            "SELECT task_id, state, version, reason_kind, reason_ref, created_at, updated_at
             FROM task_lifecycle WHERE task_id = ?",
        )
        .bind(&input.task_id)
        .fetch_one(&mut *transaction)
        .await?;
        let lifecycle = map_task_lifecycle(&lifecycle_row)?;
        transaction.commit().await?;

        Ok(TaskLifecycleTransitionWrite {
            lifecycle,
            transition_id: input.id,
            gate_evaluation_id: input.gate_evaluation_id,
            replayed: false,
            event: Some(event),
        })
    }
}

#[async_trait]
impl GateRepo for SqliteDb {
    async fn create_gate(&self, input: CreateGate) -> Result<CollaborationWrite<Gate>> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO gate (id, task_id, gate_kind, scope_kind, scope_id, active_policy_revision, created_at)
             VALUES (?, ?, ?, ?, ?, NULL, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.gate_kind)
        .bind(input.scope_kind.to_string())
        .bind(&input.scope_id)
        .bind(&input.created_at)
        .execute(&mut *transaction)
        .await?;
        let event =
            DomainEventRepo::append_event_in_tx(self, &mut transaction, &input.event).await?;
        let row = sqlx::query(
            "SELECT id, task_id, gate_kind, scope_kind, scope_id, active_policy_revision, created_at
             FROM gate WHERE id = ?",
        )
        .bind(&input.id)
        .fetch_one(&mut *transaction)
        .await?;
        let gate = map_gate(&row)?;
        transaction.commit().await?;
        Ok(CollaborationWrite {
            record: gate,
            event,
        })
    }

    async fn create_gate_with_initial_policy(
        &self,
        gate_input: CreateGate,
        policy_input: CreateGatePolicyRevision,
    ) -> Result<(
        CollaborationWrite<Gate>,
        CollaborationWrite<GatePolicyRevision>,
    )> {
        if policy_input.gate_id != gate_input.id
            || policy_input.expected_active_revision.is_some()
            || policy_input.revision != 1
        {
            return Err(DbError::Check(
                "initial Gate policy must be revision 1 for the new Gate".to_owned(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO gate (id, task_id, gate_kind, scope_kind, scope_id, active_policy_revision, created_at)
             VALUES (?, ?, ?, ?, ?, NULL, ?)",
        )
        .bind(&gate_input.id)
        .bind(&gate_input.task_id)
        .bind(&gate_input.gate_kind)
        .bind(gate_input.scope_kind.to_string())
        .bind(&gate_input.scope_id)
        .bind(&gate_input.created_at)
        .execute(&mut *transaction)
        .await?;
        let gate_event =
            DomainEventRepo::append_event_in_tx(self, &mut transaction, &gate_input.event).await?;
        sqlx::query(
            "INSERT INTO gate_policy_revision
                (gate_id, revision, schema_version, policy_json, policy_digest, created_at)
             VALUES (?, 1, ?, ?, ?, ?)",
        )
        .bind(&policy_input.gate_id)
        .bind(policy_input.schema_version)
        .bind(&policy_input.policy_json)
        .bind(&policy_input.policy_digest)
        .bind(&policy_input.created_at)
        .execute(&mut *transaction)
        .await?;
        let advanced = sqlx::query(
            "UPDATE gate SET active_policy_revision = 1
             WHERE id = ? AND active_policy_revision IS NULL",
        )
        .bind(&gate_input.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if advanced != 1 {
            return Err(DbError::VersionConflict);
        }
        let policy_event =
            DomainEventRepo::append_event_in_tx(self, &mut transaction, &policy_input.event)
                .await?;
        let gate_row = sqlx::query(
            "SELECT id, task_id, gate_kind, scope_kind, scope_id, active_policy_revision, created_at
             FROM gate WHERE id = ?",
        )
        .bind(&gate_input.id)
        .fetch_one(&mut *transaction)
        .await?;
        let policy_row = sqlx::query(
            "SELECT gate_id, revision, schema_version, policy_json, policy_digest, created_at
             FROM gate_policy_revision WHERE gate_id = ? AND revision = 1",
        )
        .bind(&gate_input.id)
        .fetch_one(&mut *transaction)
        .await?;
        let gate = map_gate(&gate_row)?;
        let policy = map_policy(&policy_row)?;
        transaction.commit().await?;
        Ok((
            CollaborationWrite {
                record: gate,
                event: gate_event,
            },
            CollaborationWrite {
                record: policy,
                event: policy_event,
            },
        ))
    }

    async fn get_gate(&self, id: &str) -> Result<Option<Gate>> {
        sqlx::query(
            "SELECT id, task_id, gate_kind, scope_kind, scope_id, active_policy_revision, created_at
             FROM gate WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .as_ref()
        .map(map_gate)
        .transpose()
    }

    async fn list_active_gate_policies(
        &self,
        task_id: &str,
    ) -> Result<Vec<(Gate, GatePolicyRevision)>> {
        let rows = sqlx::query(
            "SELECT g.id, g.task_id, g.gate_kind, g.scope_kind, g.scope_id,
                    g.active_policy_revision, g.created_at,
                    p.gate_id AS policy_gate_id, p.revision, p.schema_version,
                    p.policy_json, p.policy_digest, p.created_at AS policy_created_at
             FROM gate g JOIN gate_policy_revision p
               ON p.gate_id = g.id AND p.revision = g.active_policy_revision
             WHERE g.task_id = ? ORDER BY g.gate_kind, g.scope_kind, g.scope_id, g.id",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let gate = map_gate(row)?;
                let policy = GatePolicyRevision {
                    gate_id: row.try_get("policy_gate_id")?,
                    revision: row.try_get("revision")?,
                    schema_version: row.try_get("schema_version")?,
                    policy_json: row.try_get("policy_json")?,
                    policy_digest: row.try_get("policy_digest")?,
                    created_at: row.try_get("policy_created_at")?,
                };
                Ok((gate, policy))
            })
            .collect()
    }

    async fn get_gate_evaluation_for_cause(
        &self,
        gate_id: &str,
        causation_event_id: &str,
    ) -> Result<Option<GateEvaluation>> {
        let row = sqlx::query(
            "SELECT e.id, e.gate_id, e.task_id, e.policy_revision, e.outcome,
                    e.input_digest, e.result_json, e.evaluated_at
             FROM gate_evaluation e
             JOIN domain_event event
               ON event.entity_type = 'gate_evaluation'
              AND event.entity_id = e.id
              AND event.event_type = 'gate.evaluated'
              AND event.causation_id = ?
             WHERE e.gate_id = ?
             ORDER BY event.sequence LIMIT 1",
        )
        .bind(causation_event_id)
        .bind(gate_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(map_evaluation).transpose()
    }

    async fn get_gate_policy_revision(
        &self,
        gate_id: &str,
        revision: i64,
    ) -> Result<Option<GatePolicyRevision>> {
        sqlx::query(
            "SELECT gate_id, revision, schema_version, policy_json, policy_digest, created_at
             FROM gate_policy_revision WHERE gate_id = ? AND revision = ?",
        )
        .bind(gate_id)
        .bind(revision)
        .fetch_optional(&self.pool)
        .await?
        .as_ref()
        .map(map_policy)
        .transpose()
    }

    async fn create_gate_policy_revision(
        &self,
        input: CreateGatePolicyRevision,
    ) -> Result<CollaborationWrite<GatePolicyRevision>> {
        let expected = input.expected_active_revision.unwrap_or(0);
        if input.revision != expected + 1 {
            return Err(DbError::VersionConflict);
        }
        let mut transaction = self.pool.begin().await?;
        let active = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT active_policy_revision FROM gate WHERE id = ?",
        )
        .bind(&input.gate_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(DbError::NotFound)?;
        if active != input.expected_active_revision {
            return Err(DbError::VersionConflict);
        }
        sqlx::query(
            "INSERT INTO gate_policy_revision
                (gate_id, revision, schema_version, policy_json, policy_digest, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.gate_id)
        .bind(input.revision)
        .bind(input.schema_version)
        .bind(&input.policy_json)
        .bind(&input.policy_digest)
        .bind(&input.created_at)
        .execute(&mut *transaction)
        .await?;
        let advanced = sqlx::query(
            "UPDATE gate SET active_policy_revision = ?
             WHERE id = ? AND active_policy_revision IS ?",
        )
        .bind(input.revision)
        .bind(&input.gate_id)
        .bind(input.expected_active_revision)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if advanced != 1 {
            return Err(DbError::VersionConflict);
        }
        let event =
            DomainEventRepo::append_event_in_tx(self, &mut transaction, &input.event).await?;
        let row = sqlx::query(
            "SELECT gate_id, revision, schema_version, policy_json, policy_digest, created_at
             FROM gate_policy_revision WHERE gate_id = ? AND revision = ?",
        )
        .bind(&input.gate_id)
        .bind(input.revision)
        .fetch_one(&mut *transaction)
        .await?;
        let policy = map_policy(&row)?;
        transaction.commit().await?;
        Ok(CollaborationWrite {
            record: policy,
            event,
        })
    }

    async fn create_gate_evaluation(
        &self,
        input: StoreGateEvaluation,
    ) -> Result<GateEvaluationWrite> {
        if input.inputs.iter().enumerate().any(|(ordinal, row)| {
            row.evaluation_id != input.evaluation.id || row.ordinal != ordinal as i64
        }) {
            return Err(DbError::Check(
                "Gate evaluation inputs are not canonically ordered".to_owned(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO gate_evaluation
                (id, gate_id, task_id, policy_revision, outcome, input_digest, result_json, evaluated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(gate_id, policy_revision, input_digest) DO NOTHING",
        )
        .bind(&input.evaluation.id)
        .bind(&input.evaluation.gate_id)
        .bind(&input.evaluation.task_id)
        .bind(input.evaluation.policy_revision)
        .bind(input.evaluation.outcome.to_string())
        .bind(&input.evaluation.input_digest)
        .bind(&input.evaluation.result_json)
        .bind(&input.evaluation.evaluated_at)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        if inserted == 0 {
            let row = sqlx::query(
                "SELECT id, gate_id, task_id, policy_revision, outcome, input_digest, result_json, evaluated_at
                 FROM gate_evaluation WHERE gate_id = ? AND policy_revision = ? AND input_digest = ?",
            )
            .bind(&input.evaluation.gate_id)
            .bind(input.evaluation.policy_revision)
            .bind(&input.evaluation.input_digest)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(DbError::NotFound)?;
            let evaluation = map_evaluation(&row)?;
            if evaluation.outcome != input.evaluation.outcome
                || evaluation.result_json != input.evaluation.result_json
            {
                return Err(DbError::IdempotencyConflict);
            }
            let rows = sqlx::query(
                "SELECT evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
                        input_digest, producer_ref, subject_json, status
                 FROM gate_evaluation_input WHERE evaluation_id = ? ORDER BY ordinal",
            )
            .bind(&evaluation.id)
            .fetch_all(&mut *transaction)
            .await?;
            let inputs = rows
                .iter()
                .map(map_evaluation_input)
                .collect::<Result<Vec<_>>>()?;
            transaction.commit().await?;
            return Ok(GateEvaluationWrite {
                evaluation,
                inputs,
                event: None,
            });
        }

        for input_row in &input.inputs {
            sqlx::query(
                "INSERT INTO gate_evaluation_input (
                    evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
                    input_digest, producer_ref, subject_json, status
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&input_row.evaluation_id)
            .bind(&input.evaluation.task_id)
            .bind(input_row.ordinal)
            .bind(&input_row.input_kind)
            .bind(&input_row.input_id)
            .bind(input_row.input_version)
            .bind(&input_row.input_digest)
            .bind(input_row.producer_ref.as_deref())
            .bind(&input_row.subject_json)
            .bind(&input_row.status)
            .execute(&mut *transaction)
            .await?;
        }
        let event =
            DomainEventRepo::append_event_in_tx(self, &mut transaction, &input.event).await?;
        transaction.commit().await?;
        Ok(GateEvaluationWrite {
            evaluation: input.evaluation,
            inputs: input.inputs,
            event: Some(event),
        })
    }

    async fn get_gate_evaluation(&self, id: &str) -> Result<Option<GateEvaluation>> {
        sqlx::query(
            "SELECT id, gate_id, task_id, policy_revision, outcome, input_digest, result_json, evaluated_at
             FROM gate_evaluation WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .as_ref()
        .map(map_evaluation)
            .transpose()
    }

    async fn is_latest_gate_evaluation(
        &self,
        gate_id: &str,
        revision: i64,
        evaluation_id: &str,
    ) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM gate_evaluation current
                WHERE current.id = ? AND current.gate_id = ?
                  AND current.policy_revision = ?
                  AND NOT EXISTS (
                      SELECT 1 FROM gate_evaluation newer
                      WHERE newer.gate_id = current.gate_id
                        AND newer.policy_revision = current.policy_revision
                        AND newer.rowid > current.rowid
                  )
             )",
        )
        .bind(evaluation_id)
        .bind(gate_id)
        .bind(revision)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    async fn gate_evaluation_inputs_are_current(&self, evaluation_id: &str) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT COALESCE((
                SELECT inputs_current FROM gate_evaluation_currentness
                WHERE evaluation_id = ?
             ), 0)",
        )
        .bind(evaluation_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    async fn list_gate_evaluation_inputs(
        &self,
        evaluation_id: &str,
    ) -> Result<Vec<GateEvaluationInput>> {
        let rows = sqlx::query(
            "SELECT evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
                    input_digest, producer_ref, subject_json, status
             FROM gate_evaluation_input WHERE evaluation_id = ? ORDER BY ordinal",
        )
        .bind(evaluation_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(map_evaluation_input).collect()
    }
}
