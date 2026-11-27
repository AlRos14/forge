//! Node RT (phase 1): immutable coordinator triage of one exact validation
//! result, and the resume of a `human` disposition after a Human answered.
//!
//! `correct` creates one correction WorkUnit for the checked WorkUnit in the
//! same transaction as the triage receipt. `recheck` and `human` effects
//! (enqueue / raise) are idempotent and performed by the caller before this
//! receipt, so a replay of the completed turn converges.

use super::*;
use crate::coordinator_outputs::MicroFixOutput;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidationFindingsDecision {
    pub validation_run_id: String,
    #[serde(default)]
    pub comparison_id: Option<String>,
    pub run_digest: String,
    pub disposition: db::ValidationTriageDisposition,
    pub rationale: String,
    pub instructions: String,
    #[serde(default)]
    pub micro_fix: Option<MicroFixOutput>,
}

pub(super) const AUTHORITY_SQL: &str = "SELECT c.task_id,c.authorization_version,e.actor_kind,e.actor_id
    FROM task_coordination c JOIN task task ON task.id=c.task_id
    JOIN project project ON project.id=task.project_id
    JOIN execution e ON e.id=? AND e.task_id=c.task_id
    JOIN task_coordination_request request ON request.id=? AND request.task_id=c.task_id
      AND request.kind=? AND request.state IN ('running','uncertain')
      AND request.execution_id=e.id AND request.authorization_version=c.authorization_version
    JOIN agent_current agent ON agent.id=e.actor_id AND e.actor_kind='agent' AND agent.owner_id=project.owner_id
    JOIN project_member owner_member ON owner_member.project_id=project.id AND owner_member.user_id=agent.owner_id AND owner_member.role IN ('owner','admin','member')
    JOIN execution_task_role_authority a ON a.execution_id=e.id
    JOIN task_role role ON role.id=a.task_role_id
    JOIN task_role_capability_configuration config ON config.task_role_id=role.id
    JOIN role_membership membership ON membership.id=a.membership_id
    JOIN orchestrator_wake_execution attempt ON attempt.execution_id=e.id
    JOIN orchestrator_wake wake ON wake.id=attempt.wake_id
    JOIN task_lifecycle lifecycle ON lifecycle.task_id=c.task_id
    JOIN task_dispatch dispatch ON dispatch.task_id=c.task_id
    WHERE c.mode!='manual' AND c.revoked_at IS NULL AND c.team_stale=0
      AND request.wake_id=wake.id
      AND (json_type(wake.task_role_policy_json,'$.max_actions_per_execution') IS NULL
           OR (json_type(wake.task_role_policy_json,'$.max_actions_per_execution')='integer'
               AND json_extract(wake.task_role_policy_json,'$.max_actions_per_execution')>=1))
      AND (json_type(wake.task_role_policy_json,'$.allowed_actions') IS NULL
           OR json_type(wake.task_role_policy_json,'$.allowed_actions')='null'
           OR (json_type(wake.task_role_policy_json,'$.allowed_actions')='array'
               AND EXISTS(SELECT 1 FROM json_each(wake.task_role_policy_json,'$.allowed_actions')
                          WHERE value='review_findings')))
      AND c.coordinator_task_role_id=a.task_role_id AND c.coordinator_actor_kind=e.actor_kind AND c.coordinator_actor_id=e.actor_id
      AND task.deleted_at IS NULL AND e.status IN ('running','completed') AND e.work_unit_id IS NULL
      AND wake.current_attempt=attempt.attempt_number AND attempt.state IN ('running','completed')
      AND wake.state IN ('leased','running','uncertain')
      AND (e.status='completed' OR wake.lease_until IS NULL OR julianday(wake.lease_until)>julianday('now'))
      AND wake.task_role_version=role.version AND wake.task_role_policy_json=role.policy_json
      AND a.task_role_version=role.version AND a.configuration_json=config.configuration_json
      AND membership.status='active' AND membership.task_role_id=role.id AND membership.actor_kind=e.actor_kind AND membership.actor_id=e.actor_id
      AND EXISTS(SELECT 1 FROM json_each(a.capabilities_json) WHERE value='orchestration')
      AND lifecycle.state NOT IN ('cancelled','done','merging') AND dispatch.state NOT IN ('paused','cancelled')";

fn bounded_text(input: &ValidationFindingsDecision) -> Result<()> {
    if input.rationale.trim().is_empty()
        || input.rationale.len() > 8192
        || input.instructions.trim().is_empty()
        || input.instructions.len() > 16384
    {
        return Err(ServiceError::invalid_operation(
            "Validation triage requires bounded rationale and actionable instructions",
        ));
    }
    Ok(())
}

fn decision_digest(input: &ValidationFindingsDecision) -> Result<String> {
    Ok(hex::encode(Sha256::digest(
        serde_json::to_vec(input).map_err(|e| ServiceError::invalid_operation(e.to_string()))?,
    )))
}

fn cut(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

impl TaskReworkService {
    async fn validation_authority_row(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        execution_id: &str,
        request_id: &str,
        kind: &str,
        label: &str,
    ) -> Result<sqlx::sqlite::SqliteRow> {
        let row = sqlx::query(AUTHORITY_SQL)
            .bind(execution_id)
            .bind(request_id)
            .bind(kind)
            .fetch_optional(&mut **tx)
            .await?;
        if let Some(row) = row {
            return Ok(row);
        }
        let paused: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM task_coordination_request request
                JOIN task_dispatch dispatch ON dispatch.task_id=request.task_id
                WHERE request.id=? AND dispatch.state='paused'
                UNION ALL
                SELECT 1 FROM project_plan_batch_coordination_request scope
                JOIN project_plan_batch batch ON batch.id=scope.batch_id
                WHERE scope.request_id=? AND batch.state='paused')",
        )
        .bind(request_id)
        .bind(request_id)
        .fetch_one(&mut **tx)
        .await?;
        if paused {
            return Err(ServiceError::Coordination {
                code: "coordination.paused",
                message: format!("{label} waits until the same Task or Project batch is resumed"),
            });
        }
        Err(ServiceError::invalid_operation(
            "The exact validation result, current coordinator, wake, team or Task authority is no longer valid",
        ))
    }

    /// Source of the checked WorkUnit for one validation run: the exact check
    /// request subject. `None` when the run has no catalog request.
    async fn validation_subject(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        run_id: &str,
    ) -> Result<Option<(String, String, i64, String)>> {
        Ok(sqlx::query_as::<_, (String, String, i64, String)>(
            "SELECT req.subject_kind, req.subject_id, req.subject_version, req.workspace_id
             FROM validation_check_request req
             WHERE req.candidate_run_id=?",
        )
        .bind(run_id)
        .fetch_optional(&mut **tx)
        .await?)
    }

    /// Spend one rework round for a correction or recheck.
    async fn consume_validation_round(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        task_id: &str,
    ) -> Result<()> {
        let consumed: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM task_rework_receipt WHERE task_id=?)
                  + (SELECT count(*) FROM review_rework_round WHERE task_id=?)
                  + (SELECT count(*) FROM task_review_triage triage WHERE triage.task_id=?
                       AND (triage.disposition='recheck' OR EXISTS (
                           SELECT 1 FROM review_finding_triage finding_triage
                           WHERE finding_triage.report_artifact_id=triage.review_report_artifact_id
                             AND finding_triage.disposition='recheck'
                       )))
                  + (SELECT count(*) FROM task_validation_triage WHERE task_id=? AND disposition IN ('correct','recheck'))
                  + (SELECT count(*) FROM task_coordination_request r
                       JOIN task_coordination_request paused
                         ON paused.id=r.resumes_request_id AND paused.task_id=r.task_id
                        AND paused.kind='validation_findings'
                     WHERE r.task_id=? AND r.kind='human_decision_resume' AND r.state='triaged')",
        )
        .bind(task_id)
        .bind(task_id)
        .bind(task_id)
        .bind(task_id)
        .bind(task_id)
        .fetch_one(&mut **tx)
        .await?;
        let max_rounds: i64 = sqlx::query_scalar(
            "SELECT max_rework_rounds FROM task_autonomy_limits WHERE task_id=?",
        )
        .bind(task_id)
        .fetch_one(&mut **tx)
        .await?;
        if consumed >= max_rounds {
            return Err(ServiceError::conflict(
                "Recheck and correction round budget exhausted; a Human may authorize extra rounds",
            ));
        }
        Ok(())
    }

    /// Insert the correction WorkUnit for the checked WorkUnit. Only a
    /// `work_unit_result` subject can be corrected.
    #[allow(clippy::too_many_arguments)]
    async fn insert_validation_correction(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        task_id: &str,
        run_id: &str,
        run_digest: &str,
        rationale: &str,
        instructions: &str,
        created_by: &db::ActorRef,
        now: &str,
    ) -> Result<String> {
        let Some((subject_kind, subject_id, subject_version, checked_workspace_id)) =
            self.validation_subject(tx, run_id).await?
        else {
            return Err(crate::task_coordination::output_not_permitted(
                "correct requires a catalog check request subject",
            ));
        };
        if subject_kind != "work_unit_result" {
            return Err(crate::task_coordination::output_not_permitted(
                "correct is available only for a WorkUnit result; use recheck or human",
            ));
        }
        let unit: (String, i64, Option<String>) = sqlx::query_as(
            "SELECT work_unit.role, work_unit.requires_integration, primary_repository.repo_id
             FROM work_unit
             LEFT JOIN work_unit_repository_set_revision repository_pin
               ON repository_pin.work_unit_id = work_unit.id
              AND repository_pin.task_id = work_unit.task_id
             LEFT JOIN task_repository_set_revision_repo primary_repository
               ON primary_repository.task_id = repository_pin.task_id
              AND primary_repository.revision = repository_pin.revision
              AND primary_repository.role = 'primary'
             WHERE work_unit.id=? AND work_unit.task_id=?",
        )
        .bind(&subject_id)
        .bind(task_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServiceError::not_found("work_unit", subject_id.clone()))?;
        let execution_id: String = sqlx::query_scalar(
            "SELECT execution.id FROM execution
             JOIN execution_repository_result result
               ON result.execution_id=execution.id
             WHERE execution.task_id=? AND execution.work_unit_id=?
               AND execution.work_unit_version=? AND execution.status='completed'
               AND result.workspace_id=?
             ORDER BY execution.created_at DESC, execution.id DESC LIMIT 1",
        )
        .bind(task_id)
        .bind(&subject_id)
        .bind(subject_version)
        .bind(&checked_workspace_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            crate::task_coordination::output_not_permitted(
                "correct requires a check pinned to an exact completed repository result",
            )
        })?;
        let result_rows = sqlx::query(
            "SELECT result.execution_id, result.task_id, result.repo_id, result.workspace_id,
                    result.before_sha, result.after_sha, result.snapshot_digest, result.created_at
             FROM execution_repository_result result
             JOIN execution ON execution.id=result.execution_id
             WHERE execution.id=? AND execution.task_id=? AND execution.work_unit_id=?
               AND execution.work_unit_version=? AND execution.status='completed'
             ORDER BY result.repo_id",
        )
        .bind(&execution_id)
        .bind(task_id)
        .bind(&subject_id)
        .bind(subject_version)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(|row| {
            Ok(db::ExecutionRepositoryResult {
                execution_id: row.try_get("execution_id")?,
                task_id: row.try_get("task_id")?,
                repo_id: row.try_get("repo_id")?,
                workspace_id: row.try_get("workspace_id")?,
                before_sha: row.try_get("before_sha")?,
                after_sha: row.try_get("after_sha")?,
                snapshot_digest: row.try_get("snapshot_digest")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        if result_rows.is_empty() {
            return Err(crate::task_coordination::output_not_permitted(
                "correct requires repository results for the exact checked WorkUnit",
            ));
        }
        if unit.2.is_none() {
            return Err(ServiceError::invalid_operation(
                "checked repository results have no pinned primary repository",
            ));
        }
        self.consume_validation_round(tx, task_id).await?;
        let contributor = sole_work_unit_contributor(&self.db, task_id, &subject_id).await?;
        let role_id: Option<String> =
            sqlx::query_scalar("SELECT id FROM task_role WHERE task_id=? AND role=?")
                .bind(task_id)
                .bind(&unit.0)
                .fetch_optional(&mut **tx)
                .await?;
        let mut assigned = None;
        if let (Some(actor), Some(role_id)) = (contributor, role_id) {
            if role_has_active_member_in_tx(tx, &role_id, &actor).await? {
                assigned = Some(actor);
            }
        }
        let id = new_uuid_v4();
        let scope = cut(
            &format!(
                "Correct the exact deterministic validation failure of WorkUnit {subject_id}. Validation run {run_id} ({run_digest}).\n\nRationale: {rationale}\nInstructions: {instructions}"
            ),
            4096,
        );
        insert_work_unit_in_tx(
            tx,
            task_id,
            &id,
            "Correct failed validation check",
            &scope,
            &unit.0,
            assigned.as_ref(),
            unit.1 != 0,
            Some(("work_unit", &subject_id, None)),
            created_by,
            now,
        )
        .await?;
        for result in &result_rows {
            sqlx::query(
                "INSERT INTO work_unit_repository(work_unit_id, task_id, repo_id, created_at)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(task_id)
            .bind(&result.repo_id)
            .bind(now)
            .execute(&mut **tx)
            .await?;
        }
        let primary_result = unit
            .2
            .as_deref()
            .and_then(|repo_id| result_rows.iter().find(|result| result.repo_id == repo_id));
        sqlx::query(
            "INSERT INTO work_unit_result_requirement (
                work_unit_id, task_id, source_work_unit_id, source_work_unit_version,
                result_execution_id, result_artifact_id, result_commit_sha,
                result_workspace_id, workspace_snapshot_digest, created_at
             ) VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(task_id)
        .bind(&subject_id)
        .bind(subject_version)
        .bind(&execution_id)
        .bind(primary_result.and_then(|result| result.after_sha.as_deref()))
        .bind(primary_result.map(|result| result.workspace_id.as_str()))
        .bind(primary_result.and_then(|result| result.snapshot_digest.as_deref()))
        .bind(now)
        .execute(&mut **tx)
        .await?;
        let requirement_repositories = result_rows
            .iter()
            .map(|result| db::WorkUnitResultRequirementRepository {
                work_unit_id: id.clone(),
                task_id: task_id.to_owned(),
                repo_id: result.repo_id.clone(),
                source_work_unit_id: subject_id.clone(),
                source_work_unit_version: subject_version,
                result_execution_id: execution_id.clone(),
                result_artifact_id: None,
                result_commit_sha: result.after_sha.clone(),
                result_workspace_id: Some(result.workspace_id.clone()),
                workspace_snapshot_digest: result.snapshot_digest.clone(),
                created_at: now.to_owned(),
            })
            .collect::<Vec<_>>();
        WorkUnitRepo::insert_result_requirement_repositories_in_tx(
            &*self.db,
            tx,
            &requirement_repositories,
        )
        .await?;
        Ok(id)
    }

    pub(crate) async fn record_validation_triage(
        &self,
        execution_id: &str,
        request_id: &str,
        input: ValidationFindingsDecision,
    ) -> Result<()> {
        bounded_text(&input)?;
        let digest = decision_digest(&input)?;
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT decision_digest FROM task_validation_triage WHERE request_id=?",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            if existing != digest {
                return Err(DbError::IdempotencyConflict.into());
            }
            tx.commit().await?;
            return Ok(());
        }
        let row = self
            .validation_authority_row(
                &mut tx,
                execution_id,
                request_id,
                "validation_findings",
                "ValidationFindings triage",
            )
            .await?;
        let task_id: String = row.try_get("task_id")?;
        let authorization_version: i64 = row.try_get("authorization_version")?;
        let actor = actor_ref_from_parts(
            &row.try_get::<String, _>("actor_kind")?,
            row.try_get::<String, _>("actor_id")?,
        )?;
        let (actor_type, actor_id) = match &actor {
            ActorRef::Agent(id) => ("agent", id.clone()),
            ActorRef::Human(id) => ("human", id.clone()),
        };
        let created_by = match &actor {
            ActorRef::Agent(id) => db::ActorRef::Agent(id.clone()),
            ActorRef::Human(id) => db::ActorRef::Human(id.clone()),
        };
        let now = now_rfc3339();
        let disposition = input.disposition.to_string();
        let mut correction_id = None;
        match input.disposition {
            db::ValidationTriageDisposition::Correct => {
                correction_id = Some(
                    self.insert_validation_correction(
                        &mut tx,
                        &task_id,
                        &input.validation_run_id,
                        &input.run_digest,
                        &input.rationale,
                        &input.instructions,
                        &created_by,
                        &now,
                    )
                    .await?,
                );
            }
            db::ValidationTriageDisposition::Recheck => {
                self.consume_validation_round(&mut tx, &task_id).await?;
            }
            db::ValidationTriageDisposition::Human => {}
        }
        sqlx::query(
            "INSERT INTO task_validation_triage(request_id,task_id,validation_run_id,comparison_id,run_digest,execution_id,authorization_version,disposition,rationale,instructions,decision_digest,review_work_unit_id,rework_ref,created_at)
             VALUES(?,?,?,?,?,?,?,?,?,?,?,NULL,?,?)",
        )
        .bind(request_id)
        .bind(&task_id)
        .bind(&input.validation_run_id)
        .bind(input.comparison_id.as_deref())
        .bind(&input.run_digest)
        .bind(execution_id)
        .bind(authorization_version)
        .bind(&disposition)
        .bind(&input.rationale)
        .bind(&input.instructions)
        .bind(&digest)
        .bind(correction_id.as_deref())
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        let changed = sqlx::query(
            "UPDATE task_coordination_request
             SET state='triaged',version=version+1,updated_at=?,completed_at=?
             WHERE id=? AND task_id=? AND kind='validation_findings'
               AND state IN ('running','uncertain') AND execution_id=? AND authorization_version=?",
        )
        .bind(&now)
        .bind(&now)
        .bind(request_id)
        .bind(&task_id)
        .bind(execution_id)
        .bind(authorization_version)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(ServiceError::conflict(
                "ValidationFindings request changed before the triage receipt was committed",
            ));
        }
        let work_event = match correction_id.as_ref() {
            Some(id) => Some(
                DomainEventRepo::append_event_in_tx(
                    &*self.db,
                    &mut tx,
                    &CreateDomainEvent {
                        id: new_uuid_v4(),
                        event_type: "work_unit.created".to_owned(),
                        entity_type: "work_unit".to_owned(),
                        entity_id: id.clone(),
                        actor_type: created_by.kind().to_string(),
                        actor_id: Some(created_by.id().to_owned()),
                        scope_type: "task".to_owned(),
                        scope_id: task_id.clone(),
                        correlation_id: task_id.clone(),
                        causation_id: None,
                        causation_depth: 0,
                        dedupe_key: Some(format!("work_unit.created:{id}")),
                        payload_json: serde_json::json!({"task_id":task_id,"work_unit_id":id,"version":1,"status":"open","validation_run_id":input.validation_run_id}).to_string(),
                        created_at: now.clone(),
                    },
                )
                .await?,
            ),
            None => None,
        };
        let state_event = DomainEventRepo::append_event_in_tx(
            &*self.db,
            &mut tx,
            &CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.coordination_request_state_changed".to_owned(),
                entity_type: "task_coordination_request".to_owned(),
                entity_id: request_id.to_owned(),
                actor_type: actor_type.to_owned(),
                actor_id: Some(actor_id),
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: task_id.clone(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("validation-findings-triaged:{request_id}")),
                payload_json: serde_json::json!({
                    "task_id": task_id,
                    "request_id": request_id,
                    "state": "triaged",
                    "validation_run_id": input.validation_run_id,
                    "disposition": disposition,
                    "rework_ref": correction_id,
                })
                .to_string(),
                created_at: now.clone(),
            },
        )
        .await?;
        tx.commit().await?;
        if let Some(event) = work_event.as_ref() {
            crate::domain_event_service::DomainEventService::publish_committed_hint(
                &self.event_bus,
                event,
            );
        }
        crate::domain_event_service::DomainEventService::publish_committed_hint(
            &self.event_bus,
            &state_event,
        );
        Ok(())
    }

    /// Conclude a `human_decision_resume` request: apply the follow-up
    /// `correct` (correction WorkUnit) and mark the resume request triaged.
    /// `recheck` is enqueued by the caller before this receipt.
    pub(crate) async fn record_validation_resume(
        &self,
        execution_id: &str,
        request_id: &str,
        input: ValidationFindingsDecision,
    ) -> Result<()> {
        bounded_text(&input)?;
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let already: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task_coordination_request
             WHERE id=? AND kind='human_decision_resume' AND state='triaged' AND execution_id=?)",
        )
        .bind(request_id)
        .bind(execution_id)
        .fetch_one(&mut *tx)
        .await?;
        if already {
            tx.commit().await?;
            return Ok(());
        }
        let row = self
            .validation_authority_row(
                &mut tx,
                execution_id,
                request_id,
                "human_decision_resume",
                "HumanDecisionResume",
            )
            .await?;
        let task_id: String = row.try_get("task_id")?;
        let authorization_version: i64 = row.try_get("authorization_version")?;
        let actor_id: String = row.try_get("actor_id")?;
        let created_by = db::ActorRef::Agent(actor_id.clone());
        let now = now_rfc3339();
        let mut correction_id = None;
        match input.disposition {
            db::ValidationTriageDisposition::Correct => {
                correction_id = Some(
                    self.insert_validation_correction(
                        &mut tx,
                        &task_id,
                        &input.validation_run_id,
                        &input.run_digest,
                        &input.rationale,
                        &input.instructions,
                        &created_by,
                        &now,
                    )
                    .await?,
                );
            }
            db::ValidationTriageDisposition::Recheck => {
                self.consume_validation_round(&mut tx, &task_id).await?;
            }
            db::ValidationTriageDisposition::Human => {
                return Err(ServiceError::invalid_operation(
                    "a resumed validation decision cannot ask a Human again",
                ));
            }
        }
        let changed = sqlx::query(
            "UPDATE task_coordination_request
             SET state='triaged',version=version+1,updated_at=?,completed_at=?
             WHERE id=? AND task_id=? AND kind='human_decision_resume'
               AND state IN ('running','uncertain') AND execution_id=? AND authorization_version=?",
        )
        .bind(&now)
        .bind(&now)
        .bind(request_id)
        .bind(&task_id)
        .bind(execution_id)
        .bind(authorization_version)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(ServiceError::conflict(
                "HumanDecisionResume request changed before its receipt was committed",
            ));
        }
        let work_event = match correction_id.as_ref() {
            Some(id) => Some(
                DomainEventRepo::append_event_in_tx(
                    &*self.db,
                    &mut tx,
                    &CreateDomainEvent {
                        id: new_uuid_v4(),
                        event_type: "work_unit.created".to_owned(),
                        entity_type: "work_unit".to_owned(),
                        entity_id: id.clone(),
                        actor_type: "agent".to_owned(),
                        actor_id: Some(actor_id.clone()),
                        scope_type: "task".to_owned(),
                        scope_id: task_id.clone(),
                        correlation_id: task_id.clone(),
                        causation_id: None,
                        causation_depth: 0,
                        dedupe_key: Some(format!("work_unit.created:{id}")),
                        payload_json: serde_json::json!({"task_id":task_id,"work_unit_id":id,"version":1,"status":"open","validation_run_id":input.validation_run_id}).to_string(),
                        created_at: now.clone(),
                    },
                )
                .await?,
            ),
            None => None,
        };
        let state_event = DomainEventRepo::append_event_in_tx(
            &*self.db,
            &mut tx,
            &CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.coordination_request_state_changed".to_owned(),
                entity_type: "task_coordination_request".to_owned(),
                entity_id: request_id.to_owned(),
                actor_type: "agent".to_owned(),
                actor_id: Some(actor_id),
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: task_id.clone(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("human-decision-resume-triaged:{request_id}")),
                payload_json: serde_json::json!({
                    "task_id": task_id,
                    "request_id": request_id,
                    "state": "triaged",
                    "disposition": input.disposition.to_string(),
                    "rework_ref": correction_id,
                })
                .to_string(),
                created_at: now.clone(),
            },
        )
        .await?;
        tx.commit().await?;
        if let Some(event) = work_event.as_ref() {
            crate::domain_event_service::DomainEventService::publish_committed_hint(
                &self.event_bus,
                event,
            );
        }
        crate::domain_event_service::DomainEventService::publish_committed_hint(
            &self.event_bus,
            &state_event,
        );
        Ok(())
    }
}
