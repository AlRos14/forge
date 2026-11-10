use std::sync::Arc;

use api_types::ActorRef;
use db::{
    new_uuid_v4, now_rfc3339, ArtifactKind, CollaborationRepo, CreateDomainEvent, DbError,
    DomainEvent, DomainEventRepo, ExecutionRepo, SqliteDb, TaskLifecycleRepo, TaskLifecycleState,
    TaskRepo, TaskRoleRepo, WorkUnit, WorkUnitRepo, WorkUnitStatus,
};
use events::EventBus;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::Row;

mod triage;
pub(crate) use triage::ReviewFindingsDecision;

use crate::{
    collaboration_service::{CollaborationActorSource, CollaborationService},
    Result, ServiceError,
};

const PENDING_AUTOMATIC_REWORK_SQL: &str = "SELECT intent.*, dispatch.state AS dispatch_state,
                    lifecycle.state AS lifecycle_state
             FROM task_rework_automation_intent intent
             JOIN task_dispatch dispatch ON dispatch.task_id = intent.task_id
             JOIN task_lifecycle lifecycle ON lifecycle.task_id = intent.task_id
             WHERE intent.state = 'pending'
               AND (dispatch.state != 'paused' OR lifecycle.state = 'cancelled')
             ORDER BY intent.created_at, intent.id
             LIMIT ?";

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
    pub correction_role_id: String,
    pub correction_actor: Option<ActorRef>,
    pub review_title: String,
    pub review_scope: String,
    pub review_actor: Option<ActorRef>,
    pub idempotency_key: String,
    /// Automatic processing pins the admission-time configuration. Manual
    /// requests leave this unset and bind to the current persisted route.
    pub expected_source_configuration_json: Option<String>,
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

    /// Capture an automatic rework intent from a newly committed exact
    /// ReviewReport. The event itself is only a hint; all authority and
    /// subject fields are loaded again from SQLite.
    pub async fn process_domain_event(&self, event: &DomainEvent) -> Result<bool> {
        if event.event_type != "artifact.created" || event.entity_type != "artifact" {
            return Ok(false);
        }
        self.capture_automatic_intent(&event.entity_id).await
    }

    /// Process the durable backlog. Paused Tasks retain pending intent; a
    /// periodic caller after resume rechecks the exact source policy and
    /// membership before attempting the correction/review transaction.
    pub async fn process_pending_automatic_rework(&self, limit: i64) -> Result<usize> {
        let rows = sqlx::query(PENDING_AUTOMATIC_REWORK_SQL)
            .bind(limit.clamp(1, 100))
            .fetch_all(self.db.pool())
            .await?;
        let mut processed = 0;
        for row in rows {
            let intent_id: String = row.try_get("id")?;
            let task_id: String = row.try_get("task_id")?;
            let dispatch_state: String = row.try_get("dispatch_state")?;
            let lifecycle_state: String = row.try_get("lifecycle_state")?;
            let source_execution_id: String = row.try_get("source_execution_id")?;
            let source_role_id: String = row.try_get("source_task_role_id")?;
            let source_role_version: i64 = row.try_get("source_task_role_version")?;
            let source_configuration_json: String = row.try_get("source_configuration_json")?;
            let policy_digest: String = row.try_get("policy_digest")?;
            let report_id: String = row.try_get("review_report_artifact_id")?;
            let reviewed_work_unit_id: String = row.try_get("reviewed_work_unit_id")?;
            let reviewed_work_unit_version: i64 = row.try_get("reviewed_work_unit_version")?;
            let idempotency_key: String = row.try_get("idempotency_key")?;

            // A durable receipt means the report has already been handled,
            // even if current automatic authority was later revoked or the
            // Task was subsequently cancelled. Acknowledge that exact result
            // before considering whether any new pair may be created.
            if self
                .reconcile_existing_receipt(
                    &intent_id,
                    &task_id,
                    &report_id,
                    &reviewed_work_unit_id,
                    reviewed_work_unit_version,
                    &idempotency_key,
                )
                .await?
            {
                processed += 1;
                continue;
            }
            if dispatch_state == "cancelled" || lifecycle_state == "cancelled" {
                self.finish_automation_intent(&intent_id, "cancelled", None, None)
                    .await?;
                processed += 1;
                continue;
            }
            if dispatch_state == "paused" {
                continue;
            }
            let triage_required: bool = sqlx::query_scalar("SELECT mode != 'manual' AND revoked_at IS NULL AND COALESCE(json_extract(envelope_json,'$.orchestrator_reviews_findings'),0)=1 FROM task_coordination WHERE task_id=?")
                .bind(&task_id).fetch_optional(self.db.pool()).await?.unwrap_or(false);
            let triage: Option<(String,String)> = sqlx::query_as("SELECT disposition,instructions FROM task_review_triage WHERE review_report_artifact_id=?")
                .bind(&report_id).fetch_optional(self.db.pool()).await?;
            if triage_required {
                match triage.as_ref().map(|value| value.0.as_str()) {
                    None => continue,
                    Some("recheck") => {
                        self.finish_automation_intent(
                            &intent_id,
                            "cancelled",
                            Some("Coordinator requested a new independent review"),
                            None,
                        )
                        .await?;
                        continue;
                    }
                    Some("human") => {
                        self.finish_automation_intent(
                            &intent_id,
                            "blocked",
                            Some("Coordinator requires a Human decision on the review findings"),
                            None,
                        )
                        .await?;
                        continue;
                    }
                    Some("correct") => {}
                    _ => {
                        return Err(ServiceError::invalid_operation(
                            "Unknown review triage disposition",
                        ))
                    }
                }
            }

            let source = sqlx::query(
                "SELECT role.version AS role_version, configuration.configuration_json,
                        authority.task_role_version, authority.configuration_json AS authority_json,
                        authority.capabilities_json, membership.status AS membership_status,
                        execution.status AS execution_status
                 FROM execution
                 JOIN execution_task_role_authority authority
                   ON authority.execution_id = execution.id
                 JOIN task_role role
                   ON role.id = authority.task_role_id AND role.task_id = execution.task_id
                 JOIN task_role_capability_configuration configuration
                   ON configuration.task_role_id = role.id
                 JOIN role_membership membership
                   ON membership.id = authority.membership_id
                  AND membership.task_role_id = role.id
                  AND membership.actor_kind = execution.actor_kind
                  AND membership.actor_id = execution.actor_id
                 WHERE execution.id = ? AND role.id = ? AND execution.task_id = ?",
            )
            .bind(&source_execution_id)
            .bind(&source_role_id)
            .bind(&task_id)
            .fetch_optional(self.db.pool())
            .await?;
            let Some(source) = source else {
                self.finish_automation_intent(
                    &intent_id,
                    "blocked",
                    Some("source Execution or TaskRole authority no longer exists"),
                    None,
                )
                .await?;
                processed += 1;
                continue;
            };
            let current_role_version: i64 = source.try_get("role_version")?;
            let current_snapshot_version: Option<i64> = source.try_get("task_role_version")?;
            let current_configuration_json: String = source.try_get("configuration_json")?;
            let authority_configuration_json: String = source.try_get("authority_json")?;
            let current_membership: String = source.try_get("membership_status")?;
            let execution_status: String = source.try_get("execution_status")?;
            if current_role_version != source_role_version
                || current_snapshot_version != Some(source_role_version)
                || current_configuration_json != source_configuration_json
                || authority_configuration_json != source_configuration_json
                || current_membership != "active"
            {
                self.finish_automation_intent(
                    &intent_id,
                    "blocked",
                    Some("source role configuration, version, or active membership changed"),
                    None,
                )
                .await?;
                processed += 1;
                continue;
            }
            if execution_status != "completed" {
                continue;
            }
            let configuration = match crate::task_role_configuration::parse_task_role_configuration(
                &current_configuration_json,
            ) {
                Ok(configuration) => configuration,
                Err(error) => {
                    self.finish_automation_intent(
                        &intent_id,
                        "blocked",
                        Some(&error.to_string()),
                        None,
                    )
                    .await?;
                    processed += 1;
                    continue;
                }
            };
            let Some(policy) = configuration.rework.as_ref() else {
                self.finish_automation_intent(
                    &intent_id,
                    "blocked",
                    Some("source role has no current rework route"),
                    None,
                )
                .await?;
                processed += 1;
                continue;
            };
            if policy.mode != api_types::TaskRoleReworkMode::Automatic
                || !configuration
                    .capabilities
                    .contains(&api_types::TaskRoleCapability::FormalReview)
                || !configuration
                    .capabilities
                    .contains(&api_types::TaskRoleCapability::RequestRework)
                || hex::encode(Sha256::digest(current_configuration_json.as_bytes()))
                    != policy_digest
            {
                self.finish_automation_intent(
                    &intent_id,
                    "blocked",
                    Some("automatic rework capability or policy digest is no longer current"),
                    None,
                )
                .await?;
                processed += 1;
                continue;
            }

            let Some(correction_role_id) = (policy.allowed_destination_role_ids.len() == 1)
                .then(|| policy.allowed_destination_role_ids[0].clone())
            else {
                self.finish_automation_intent(
                    &intent_id,
                    "blocked",
                    Some("automatic routing requires exactly one correction TaskRole destination"),
                    None,
                )
                .await?;
                processed += 1;
                continue;
            };
            if policy.next_review_role_id.is_none() {
                self.finish_automation_intent(
                    &intent_id,
                    "blocked",
                    Some("automatic routing has no next review TaskRole"),
                    None,
                )
                .await?;
                processed += 1;
                continue;
            }
            let Some(review_actor) = policy.review_actor.clone() else {
                self.finish_automation_intent(
                    &intent_id,
                    "blocked",
                    Some("automatic routing requires an explicit independent review Actor"),
                    None,
                )
                .await?;
                processed += 1;
                continue;
            };
            let reviewed = WorkUnitRepo::get_by_id(&*self.db, &reviewed_work_unit_id)
                .await?
                .filter(|work_unit| work_unit.task_id == task_id);
            let Some(reviewed) = reviewed else {
                self.finish_automation_intent(
                    &intent_id,
                    "blocked",
                    Some("reviewed WorkUnit is no longer available"),
                    None,
                )
                .await?;
                processed += 1;
                continue;
            };
            let report = CollaborationRepo::get_artifact(&*self.db, &report_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("review_report_artifact", report_id.clone())
                })?;
            let report_json: serde_json::Value =
                serde_json::from_str(report.content.as_deref().unwrap_or_default())
                    .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            let findings = report_json
                .get("findings")
                .and_then(serde_json::Value::as_array)
                .filter(|findings| !findings.is_empty())
                .and_then(|findings| {
                    let lines = findings
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .filter(|finding| !finding.trim().is_empty())
                        .map(|finding| format!("- {finding}"))
                        .collect::<Vec<_>>();
                    (!lines.is_empty()).then_some(lines.join("\n"))
                });
            let Some(findings) = findings else {
                self.finish_automation_intent(
                    &intent_id,
                    "blocked",
                    Some("request_changes ReviewReport has no usable exact findings"),
                    None,
                )
                .await?;
                processed += 1;
                continue;
            };
            let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
            let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.clone()))?;
            let correction_actor = match policy.correction_actor.as_ref() {
                Some(api_types::ReworkActorSelection::Explicit { actor_ref }) => {
                    Some(actor_ref.clone())
                }
                Some(api_types::ReworkActorSelection::OriginalContributor) | None => None,
            };
            let input = RequestReworkInput {
                task_id: task_id.clone(),
                expected_task_version: task.version,
                expected_lifecycle_version: lifecycle.version,
                review_report_artifact_id: report_id.clone(),
                reviewed_work_unit_id: reviewed.id.clone(),
                reviewed_work_unit_version,
                correction_title: format!("Address review findings: {}", reviewed.title),
                correction_scope: format!(
                    "Original scope:\n{}\n\nExact findings from ReviewReport {}:\n{}\n\nCoordinator's confirmed correction scope:\n{}",
                    reviewed.scope, report_id, findings,
                    triage.as_ref().map(|value|value.1.as_str()).unwrap_or("Address the exact report within the authorized original scope.")
                ),
                correction_role_id,
                correction_actor,
                review_title: format!("Re-review corrected result: {}", reviewed.title),
                review_scope: format!(
                    "Review the exact corrected result for WorkUnit {} at version {}. The source ReviewReport is {}.",
                    reviewed.id, reviewed_work_unit_version, report_id
                ),
                review_actor: Some(review_actor),
                idempotency_key: idempotency_key.clone(),
                expected_source_configuration_json: Some(source_configuration_json),
            };
            match self
                .request_rework(
                    CollaborationActorSource::Execution(source_execution_id),
                    input,
                )
                .await
            {
                Ok(result) => {
                    let round: i64 = sqlx::query_scalar(
                        "SELECT round FROM task_rework_receipt WHERE correction_work_unit_id=?",
                    )
                    .bind(&result.correction_work_unit.id)
                    .fetch_one(self.db.pool())
                    .await?;
                    self.finish_automation_intent(
                        &intent_id,
                        "completed",
                        None,
                        Some((
                            &result.correction_work_unit.id,
                            &result.review_work_unit.id,
                            round,
                        )),
                    )
                    .await?;
                }
                Err(error) => {
                    // Another consumer or an explicit Human request may have
                    // committed the exact report's receipt while this worker
                    // was preparing the pair. Reconcile that durable result
                    // instead of letting a racing blocker overwrite success.
                    if !self
                        .reconcile_existing_receipt(
                            &intent_id,
                            &task_id,
                            &report_id,
                            &reviewed_work_unit_id,
                            reviewed_work_unit_version,
                            &idempotency_key,
                        )
                        .await?
                    {
                        match classify_automatic_rework_error(&error) {
                            AutomaticReworkErrorDisposition::Deferred => {
                                // A concurrent pause is a retryable deferral.
                                // Keep the durable intent pending so resume can
                                // pick it up.
                                continue;
                            }
                            AutomaticReworkErrorDisposition::Cancelled => {
                                self.finish_automation_intent(&intent_id, "cancelled", None, None)
                                    .await?;
                            }
                            AutomaticReworkErrorDisposition::Blocked => {
                                let reason = error.to_string();
                                self.finish_automation_intent(
                                    &intent_id,
                                    "blocked",
                                    Some(&reason),
                                    None,
                                )
                                .await?;
                            }
                            AutomaticReworkErrorDisposition::Unexpected => return Err(error),
                        }
                    }
                }
            }
            processed += 1;
        }
        Ok(processed)
    }

    pub async fn list_automatic_intents(
        &self,
        task_id: &str,
    ) -> Result<Vec<api_types::TaskReworkAutomationIntentResponse>> {
        let rows = sqlx::query(
            "SELECT id, review_report_artifact_id, reviewed_work_unit_id,
                    reviewed_work_unit_version, state, block_reason,
                    correction_work_unit_id, review_work_unit_id, rework_round,
                    reconciled_manual_receipt_id, created_at, updated_at
             FROM task_rework_automation_intent
             WHERE task_id = ? ORDER BY created_at DESC, id DESC",
        )
        .bind(task_id)
        .fetch_all(self.db.pool())
        .await?;
        rows.into_iter()
            .map(|row| {
                let state: String = row.try_get("state")?;
                let state = match state.as_str() {
                    "pending" => api_types::TaskReworkAutomationState::Pending,
                    "completed" => api_types::TaskReworkAutomationState::Completed,
                    "blocked" => api_types::TaskReworkAutomationState::Blocked,
                    "cancelled" => api_types::TaskReworkAutomationState::Cancelled,
                    _ => {
                        return Err(ServiceError::invalid_operation(
                            "unknown rework intent state",
                        ))
                    }
                };
                Ok(api_types::TaskReworkAutomationIntentResponse {
                    id: row.try_get("id")?,
                    review_report_artifact_id: row.try_get("review_report_artifact_id")?,
                    reviewed_work_unit_id: row.try_get("reviewed_work_unit_id")?,
                    reviewed_work_unit_version: row.try_get("reviewed_work_unit_version")?,
                    state,
                    block_reason: row.try_get("block_reason")?,
                    correction_work_unit_id: row.try_get("correction_work_unit_id")?,
                    review_work_unit_id: row.try_get("review_work_unit_id")?,
                    rework_round: row.try_get("rework_round")?,
                    reconciled_manual_receipt_id: row.try_get("reconciled_manual_receipt_id")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .collect()
    }

    async fn capture_automatic_intent(&self, artifact_id: &str) -> Result<bool> {
        let row = sqlx::query(
            "SELECT report.id AS report_id, report.task_id,
                    producer.execution_id AS source_execution_id,
                    authority.task_role_id AS source_task_role_id,
                    authority.task_role_version AS source_task_role_version,
                    authority.configuration_json AS source_configuration_json,
                    subject.reviewed_work_unit_id, subject.reviewed_work_unit_version,
                    subject.result_execution_id, subject.result_artifact_id,
                    subject.workspace_snapshot_digest
             FROM artifact report
             JOIN artifact_execution_producer producer
               ON producer.artifact_id = report.id AND producer.task_id = report.task_id
             JOIN execution execution
               ON execution.id = producer.execution_id AND execution.task_id = report.task_id
             JOIN execution_task_role_authority authority
               ON authority.execution_id = execution.id
             JOIN task_role role
               ON role.id = authority.task_role_id AND role.task_id = execution.task_id
             JOIN task_role_capability_configuration configuration
               ON configuration.task_role_id = role.id
             JOIN role_membership membership
               ON membership.id = authority.membership_id
              AND membership.task_role_id = role.id
              AND membership.actor_kind = execution.actor_kind
              AND membership.actor_id = execution.actor_id
              AND membership.status = 'active'
             JOIN work_unit_review_subject subject
               ON subject.review_work_unit_id = execution.work_unit_id
              AND subject.task_id = execution.task_id
             WHERE report.id = ? AND report.kind = 'review_report'
               AND authority.source = 'admission'
               AND authority.task_role_version = role.version
               AND authority.configuration_json = configuration.configuration_json
               AND EXISTS (
                   SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities') capability
                   WHERE capability.type = 'text' AND capability.value = 'formal_review'
               )
               AND EXISTS (
                   SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities') capability
                   WHERE capability.type = 'text' AND capability.value = 'request_rework'
               )
               AND json_extract(configuration.configuration_json, '$.rework.mode') = 'automatic'",
        )
        .bind(artifact_id)
        .fetch_optional(self.db.pool())
        .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let report_id: String = row.try_get("report_id")?;
        let task_id: String = row.try_get("task_id")?;
        let source_execution_id: String = row.try_get("source_execution_id")?;
        let source_task_role_id: String = row.try_get("source_task_role_id")?;
        let source_task_role_version: i64 = row.try_get("source_task_role_version")?;
        let source_configuration_json: String = row.try_get("source_configuration_json")?;
        let reviewed_work_unit_id: String = row.try_get("reviewed_work_unit_id")?;
        let reviewed_work_unit_version: i64 = row.try_get("reviewed_work_unit_version")?;
        let result_execution_id: Option<String> = row.try_get("result_execution_id")?;
        let result_artifact_id: Option<String> = row.try_get("result_artifact_id")?;
        let snapshot_digest: Option<String> = row.try_get("workspace_snapshot_digest")?;
        let report = CollaborationRepo::get_artifact(&*self.db, &report_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("review_report_artifact", report_id.clone()))?;
        let content: serde_json::Value =
            serde_json::from_str(report.content.as_deref().unwrap_or_default())
                .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        if content.get("verdict").and_then(serde_json::Value::as_str) != Some("request_changes")
            || content
                .get("subject")
                .and_then(|subject| subject.get("reviewed_work_unit_id"))
                .and_then(serde_json::Value::as_str)
                != Some(reviewed_work_unit_id.as_str())
            || content
                .get("subject")
                .and_then(|subject| subject.get("reviewed_work_unit_version"))
                .and_then(serde_json::Value::as_i64)
                != Some(reviewed_work_unit_version)
            || content
                .get("subject")
                .and_then(|subject| subject.get("reviewed_result"))
                .and_then(|result| result.get("execution_id"))
                .and_then(serde_json::Value::as_str)
                != result_execution_id.as_deref()
            || content
                .get("subject")
                .and_then(|subject| subject.get("reviewed_result"))
                .and_then(|result| result.get("artifact_id"))
                .and_then(serde_json::Value::as_str)
                != result_artifact_id.as_deref()
            || content
                .get("subject")
                .and_then(|subject| subject.get("reviewed_result"))
                .and_then(|result| result.get("workspace_snapshot_digest"))
                .and_then(serde_json::Value::as_str)
                != snapshot_digest.as_deref()
        {
            return Ok(false);
        }
        let policy_digest = hex::encode(Sha256::digest(source_configuration_json.as_bytes()));
        let idempotency_key = automatic_rework_idempotency_key(
            &policy_digest,
            &report_id,
            &reviewed_work_unit_id,
            reviewed_work_unit_version,
            result_execution_id.as_deref(),
            result_artifact_id.as_deref(),
            snapshot_digest.as_deref(),
        );
        let intent_id = new_uuid_v4();
        let now = now_rfc3339();
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let inserted = sqlx::query(
            "INSERT INTO task_rework_automation_intent (
                id, task_id, source_execution_id, source_task_role_id,
                source_task_role_version, source_configuration_json, policy_digest,
                review_report_artifact_id, reviewed_work_unit_id,
                reviewed_work_unit_version, idempotency_key, state,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)
             ON CONFLICT(task_id, idempotency_key) DO NOTHING",
        )
        .bind(&intent_id)
        .bind(&task_id)
        .bind(&source_execution_id)
        .bind(&source_task_role_id)
        .bind(source_task_role_version)
        .bind(&source_configuration_json)
        .bind(&policy_digest)
        .bind(&report_id)
        .bind(&reviewed_work_unit_id)
        .bind(reviewed_work_unit_version)
        .bind(&idempotency_key)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted == 0 {
            let existing = sqlx::query(
                "SELECT source_execution_id, source_task_role_id,
                        source_task_role_version, source_configuration_json,
                        policy_digest, review_report_artifact_id,
                        reviewed_work_unit_id, reviewed_work_unit_version
                 FROM task_rework_automation_intent
                 WHERE task_id = ? AND idempotency_key = ?",
            )
            .bind(&task_id)
            .bind(&idempotency_key)
            .fetch_optional(&mut *tx)
            .await?;
            let matches = match existing {
                Some(existing) => {
                    existing.try_get::<String, _>("source_execution_id")? == source_execution_id
                        && existing.try_get::<String, _>("source_task_role_id")?
                            == source_task_role_id
                        && existing.try_get::<i64, _>("source_task_role_version")?
                            == source_task_role_version
                        && existing.try_get::<String, _>("source_configuration_json")?
                            == source_configuration_json
                        && existing.try_get::<String, _>("policy_digest")? == policy_digest
                        && existing.try_get::<String, _>("review_report_artifact_id")? == report_id
                        && existing.try_get::<String, _>("reviewed_work_unit_id")?
                            == reviewed_work_unit_id
                        && existing.try_get::<i64, _>("reviewed_work_unit_version")?
                            == reviewed_work_unit_version
                }
                None => false,
            };
            if !matches {
                return Err(DbError::IdempotencyConflict.into());
            }
        }
        tx.commit().await?;
        Ok(inserted > 0)
    }

    async fn finish_automation_intent(
        &self,
        intent_id: &str,
        state: &str,
        block_reason: Option<&str>,
        pair: Option<(&str, &str, i64)>,
    ) -> Result<()> {
        self.finish_automation_intent_with_reconciliation(
            intent_id,
            state,
            block_reason,
            pair,
            None,
        )
        .await
    }

    async fn finish_automation_intent_with_reconciliation(
        &self,
        intent_id: &str,
        state: &str,
        block_reason: Option<&str>,
        pair: Option<(&str, &str, i64)>,
        reconciled_manual_receipt_id: Option<&str>,
    ) -> Result<()> {
        let (correction_id, review_id, round) = pair
            .map(|(correction, review, round)| (Some(correction), Some(review), Some(round)))
            .unwrap_or((None, None, None));
        let completed_at = (state == "completed").then(now_rfc3339);
        sqlx::query(
            "UPDATE task_rework_automation_intent
             SET state = ?, block_reason = ?, correction_work_unit_id = ?,
                 review_work_unit_id = ?, rework_round = ?, completed_at = ?,
                 reconciled_manual_receipt_id = ?, updated_at = ?
             WHERE id = ? AND state = 'pending'",
        )
        .bind(state)
        .bind(block_reason.map(|reason| reason.chars().take(2048).collect::<String>()))
        .bind(correction_id)
        .bind(review_id)
        .bind(round)
        .bind(completed_at.as_deref())
        .bind(reconciled_manual_receipt_id)
        .bind(now_rfc3339())
        .bind(intent_id)
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    async fn reconcile_existing_receipt(
        &self,
        intent_id: &str,
        task_id: &str,
        report_id: &str,
        reviewed_work_unit_id: &str,
        reviewed_work_unit_version: i64,
        automatic_idempotency_key: &str,
    ) -> Result<bool> {
        let Some(receipt) = sqlx::query(
            "SELECT id, idempotency_key, reviewed_work_unit_id,
                    reviewed_work_unit_version, correction_work_unit_id,
                    review_work_unit_id, round
             FROM task_rework_receipt
             WHERE task_id = ? AND review_report_artifact_id = ?",
        )
        .bind(task_id)
        .bind(report_id)
        .fetch_optional(self.db.pool())
        .await?
        else {
            return Ok(false);
        };

        let receipt_reviewed_work_unit_id: String = receipt.try_get("reviewed_work_unit_id")?;
        let receipt_reviewed_work_unit_version: i64 =
            receipt.try_get("reviewed_work_unit_version")?;
        if receipt_reviewed_work_unit_id != reviewed_work_unit_id
            || receipt_reviewed_work_unit_version != reviewed_work_unit_version
        {
            return Err(DbError::IdempotencyConflict.into());
        }

        let receipt_id: String = receipt.try_get("id")?;
        let receipt_idempotency_key: String = receipt.try_get("idempotency_key")?;
        let correction_id: String = receipt.try_get("correction_work_unit_id")?;
        let review_id: String = receipt.try_get("review_work_unit_id")?;
        let round: i64 = receipt.try_get("round")?;
        if receipt_idempotency_key == automatic_idempotency_key {
            self.finish_automation_intent(
                intent_id,
                "completed",
                None,
                Some((&correction_id, &review_id, round)),
            )
            .await?;
        } else {
            self.finish_automation_intent_with_reconciliation(
                intent_id,
                "completed",
                None,
                Some((&correction_id, &review_id, round)),
                Some(&receipt_id),
            )
            .await?;
        }
        Ok(true)
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
        if lifecycle.state == TaskLifecycleState::Cancelled {
            return Err(DbError::TaskDispatchCancelled.into());
        }
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
            .filter(|execution| execution.task_id == task.id)
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "ReviewReport producer is not an exact Task Execution",
                )
            })?;
        if !crate::role_capabilities::execution_has_capability(
            &self.db,
            &producer.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?
            || !crate::role_capabilities::execution_has_capability(
                &self.db,
                &producer.id,
                api_types::TaskRoleCapability::RequestRework,
            )
            .await?
        {
            return Err(ServiceError::invalid_operation(
                "ReviewReport producer was not admitted with formal_review and request_rework",
            ));
        }
        let producer_authority =
            db::ExecutionTaskRoleAuthorityRepo::get_by_execution_id(&*self.db, &producer.id)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "ReviewReport producer has no immutable TaskRole authority snapshot",
                    )
                })?;
        let source_role_id = producer_authority.task_role_id;
        let source_configuration =
            crate::role_capabilities::current_task_role_configuration(&self.db, &source_role_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "ReviewReport TaskRole configuration is missing",
                    )
                })?;
        let rework_policy = source_configuration.rework.as_ref().ok_or_else(|| {
            ServiceError::invalid_operation(
                "ReviewReport TaskRole has no current rework routing policy",
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
        let correction_role = TaskRoleRepo::get_by_id(&*self.db, &input.correction_role_id)
            .await?
            .filter(|role| {
                role.task_id == task.id
                    && rework_policy
                        .allowed_destination_role_ids
                        .contains(&role.id)
            })
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "correction TaskRole is not an allowed destination of the current rework policy",
                )
            })?;
        let review_role_id = rework_policy
            .next_review_role_id
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "rework routing has no explicit next review TaskRole",
                )
            })?;
        let review_role = TaskRoleRepo::get_by_id(&*self.db, review_role_id)
            .await?
            .filter(|role| role.task_id == task.id)
            .ok_or_else(|| {
                ServiceError::invalid_operation("next review TaskRole does not belong to this Task")
            })?;
        if !crate::role_capabilities::current_task_role_has_capability(
            &self.db,
            &review_role.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?
        {
            return Err(ServiceError::invalid_operation(
                "next review TaskRole no longer has formal_review capability",
            ));
        }

        let correction_actor = resolve_correction_actor(
            &self.db,
            &task.id,
            &reviewed.id,
            rework_policy.correction_actor.as_ref(),
            input.correction_actor.as_ref(),
        )
        .await?;
        let review_actor = match (&rework_policy.review_actor, &input.review_actor) {
            (Some(configured), Some(requested)) if configured != requested => {
                return Err(ServiceError::invalid_operation(
                    "review Actor conflicts with the persisted rework policy",
                ));
            }
            (Some(configured), _) => Some(configured.clone()),
            (None, requested) => requested.clone(),
        }
        .ok_or_else(|| {
            ServiceError::invalid_operation(
                "rework requires an explicit next review Actor; no correction pair was created",
            )
        })?;
        if (correction_actor == review_actor && matches!(review_actor, ActorRef::Human(_)))
            || actor_contributed_to_work_unit(&self.db, &task.id, &reviewed.id, &review_actor)
                .await?
        {
            return Err(ServiceError::invalid_operation(
                "next review Actor must be independent of every contributor to the reviewed result",
            ));
        }

        let findings = report
            .get("findings")
            .and_then(serde_json::Value::as_array)
            .filter(|findings| !findings.is_empty())
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "request_changes ReviewReport must contain at least one exact finding",
                )
            })?;
        let findings_text = findings
            .iter()
            .filter_map(serde_json::Value::as_str)
            .filter(|finding| !finding.trim().is_empty())
            .map(|finding| format!("- {finding}"))
            .collect::<Vec<_>>();
        if findings_text.is_empty() {
            return Err(ServiceError::invalid_operation(
                "request_changes ReviewReport has no usable exact findings",
            ));
        }
        let correction_scope = format!(
            "{}\n\nExact findings from ReviewReport {}:\n{}",
            input.correction_scope,
            review_report.id,
            findings_text.join("\n")
        );

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
        let current_dispatch_state: String =
            sqlx::query_scalar("SELECT state FROM task_dispatch WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(&mut *tx)
                .await?;
        if current_lifecycle == TaskLifecycleState::Cancelled.to_string()
            || current_dispatch_state == "cancelled"
        {
            return Err(DbError::TaskDispatchCancelled.into());
        }
        if current_dispatch_state == "paused" {
            return Err(DbError::TaskDispatchPaused.into());
        }
        if current_task_version != input.expected_task_version
            || current_lifecycle != lifecycle.state.to_string()
        {
            return Err(DbError::VersionConflict.into());
        }
        let current_source_row = sqlx::query(
            "SELECT role.version AS role_version, configuration.configuration_json
             FROM task_role role
             JOIN task_role_capability_configuration configuration
               ON configuration.task_role_id = role.id
             WHERE role.id = ? AND role.task_id = ?",
        )
        .bind(&source_role_id)
        .bind(&task.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation("source TaskRole configuration is missing")
        })?;
        let current_source_role_version: i64 = current_source_row.try_get("role_version")?;
        let current_source_configuration_json: String =
            current_source_row.try_get("configuration_json")?;
        let current_source_configuration: api_types::TaskRoleConfiguration =
            crate::task_role_configuration::parse_task_role_configuration(
                &current_source_configuration_json,
            )?;
        if input
            .expected_source_configuration_json
            .as_deref()
            .is_some_and(|expected| expected != current_source_configuration_json)
        {
            return Err(ServiceError::invalid_operation(
                "automatic rework policy changed after the ReviewReport was recorded",
            ));
        }
        let current_rework_policy =
            current_source_configuration
                .rework
                .as_ref()
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "rework routing was revoked before pair creation",
                    )
                })?;
        if !current_source_configuration
            .capabilities
            .contains(&api_types::TaskRoleCapability::RequestRework)
            || !current_rework_policy
                .allowed_destination_role_ids
                .contains(&correction_role.id)
            || current_rework_policy.next_review_role_id.as_deref() != Some(review_role.id.as_str())
            || !role_has_capability_in_tx(
                &mut tx,
                &review_role.id,
                api_types::TaskRoleCapability::FormalReview,
            )
            .await?
        {
            return Err(ServiceError::invalid_operation(
                "rework routing or formal_review authority changed before pair creation",
            ));
        }
        if input.expected_source_configuration_json.is_some()
            && current_source_configuration
                .rework
                .as_ref()
                .is_none_or(|policy| policy.mode != api_types::TaskRoleReworkMode::Automatic)
        {
            return Err(ServiceError::invalid_operation(
                "automatic rework mode was revoked before pair creation",
            ));
        }
        let source_authority_is_current: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1
                FROM execution execution
                JOIN execution_task_role_authority authority
                  ON authority.execution_id = execution.id
                JOIN role_membership membership
                  ON membership.id = authority.membership_id
                 AND membership.task_role_id = authority.task_role_id
                 AND membership.actor_kind = execution.actor_kind
                 AND membership.actor_id = execution.actor_id
                JOIN json_each(authority.capabilities_json) capability
                WHERE execution.id = ? AND execution.task_id = ?
                  AND authority.task_role_id = ?
                  AND authority.actor_kind = execution.actor_kind
                  AND authority.actor_id = execution.actor_id
                  AND membership.status = 'active'
                  AND capability.type = 'text'
                  AND capability.value = 'request_rework'
                  AND EXISTS (
                      SELECT 1 FROM json_each(authority.capabilities_json) admitted
                      WHERE admitted.type = 'text' AND admitted.value = 'formal_review'
                  )
            )",
        )
        .bind(&producer.id)
        .bind(&task.id)
        .bind(&source_role_id)
        .fetch_one(&mut *tx)
        .await?;
        if !source_authority_is_current
            || (input.expected_source_configuration_json.is_some()
                && producer_authority.task_role_version != Some(current_source_role_version))
        {
            return Err(ServiceError::invalid_operation(
                "ReviewReport producer no longer has its exact active rework authority",
            ));
        }
        if !role_has_active_member_in_tx(&mut tx, &correction_role.id, &correction_actor).await?
            || !role_has_active_member_in_tx(&mut tx, &review_role.id, &review_actor).await?
        {
            return Err(ServiceError::invalid_operation(
                "correction and review Actors must remain active members of their exact TaskRoles",
            ));
        }
        let round: i64 =
            sqlx::query_scalar("SELECT COUNT(*) + 1 + (SELECT COUNT(*) FROM task_review_triage WHERE task_id=? AND disposition='recheck') FROM task_rework_receipt WHERE task_id = ?")
                .bind(&task.id)
                .bind(&task.id)
                .fetch_one(&mut *tx)
                .await?;
        let existing_report_receipt = sqlx::query(
            "SELECT id FROM task_rework_receipt
             WHERE task_id = ? AND review_report_artifact_id = ?",
        )
        .bind(&task.id)
        .bind(&review_report.id)
        .fetch_optional(&mut *tx)
        .await?;
        if existing_report_receipt.is_some() {
            return Err(ServiceError::conflict(
                "this exact ReviewReport already created a correction and review round",
            ));
        }
        let max_rounds: i64 = sqlx::query_scalar(
            "SELECT max_rework_rounds FROM task_autonomy_limits WHERE task_id=?",
        )
        .bind(&task.id)
        .fetch_one(&mut *tx)
        .await?;
        if round > max_rounds {
            return Err(ServiceError::conflict(format!(
                "rework budget exhausted after {max_rounds} rounds"
            )));
        }

        insert_work_unit_in_tx(
            &mut tx,
            &task.id,
            &correction_id,
            &input.correction_title,
            &correction_scope,
            &correction_role.role,
            Some(&correction_actor),
            reviewed.requires_integration,
            Some(("artifact", &review_report.id, None)),
            &actor,
            &now,
        )
        .await?;
        // A correction to an Artifact package inherits the exact immutable
        // source pins. Its next review can therefore record a new manifest
        // for the corrected result without replacing the original manifest.
        sqlx::query(
            "INSERT INTO work_unit_artifact_input (
                work_unit_id, task_id, artifact_id, artifact_digest,
                producer_execution_id, created_at
             )
             SELECT ?, task_id, artifact_id, artifact_digest,
                    producer_execution_id, ?
             FROM work_unit_artifact_input
             WHERE work_unit_id = ? AND task_id = ?",
        )
        .bind(&correction_id)
        .bind(&now)
        .bind(&reviewed.id)
        .bind(&task.id)
        .execute(&mut *tx)
        .await?;
        insert_work_unit_in_tx(
            &mut tx,
            &task.id,
            &review_id,
            &input.review_title,
            &input.review_scope,
            &review_role.role,
            Some(&review_actor),
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
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&correction_id)
        .bind(&task.id)
        .bind(&reviewed.id)
        .bind(reviewed_version)
        .bind(&result_execution.id)
        .bind(producer_subject.result_artifact_id.as_deref())
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
            rounds_remaining: (max_rounds - round).max(0),
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
        let max_rounds: i64 = sqlx::query_scalar(
            "SELECT max_rework_rounds FROM task_autonomy_limits WHERE task_id=?",
        )
        .bind(&input.task_id)
        .fetch_one(self.db.pool())
        .await?;
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
            rounds_remaining: (max_rounds - round).max(0),
            replayed: true,
        }))
    }
}

async fn resolve_correction_actor(
    db: &SqliteDb,
    task_id: &str,
    work_unit_id: &str,
    configured: Option<&api_types::ReworkActorSelection>,
    requested: Option<&ActorRef>,
) -> Result<ActorRef> {
    match configured {
        Some(api_types::ReworkActorSelection::Explicit { actor_ref }) => {
            if requested.is_some_and(|requested| requested != actor_ref) {
                return Err(ServiceError::invalid_operation(
                    "correction Actor conflicts with the persisted rework policy",
                ));
            }
            Ok(actor_ref.clone())
        }
        Some(api_types::ReworkActorSelection::OriginalContributor) => {
            let contributor = sole_work_unit_contributor(db, task_id, work_unit_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "original contributor selection is ambiguous or has no contributor",
                    )
                })?;
            if requested.is_some_and(|requested| requested != &contributor) {
                return Err(ServiceError::invalid_operation(
                    "correction Actor conflicts with the original contributor selection",
                ));
            }
            Ok(contributor)
        }
        None => requested.cloned().ok_or_else(|| {
            ServiceError::invalid_operation(
                "rework requires an explicit correction Actor; no correction pair was created",
            )
        }),
    }
}

async fn sole_work_unit_contributor(
    db: &SqliteDb,
    task_id: &str,
    work_unit_id: &str,
) -> Result<Option<ActorRef>> {
    let mut contributors = Vec::<ActorRef>::new();
    if let Some((kind, id)) = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT assigned_actor_kind, assigned_actor_id FROM work_unit
         WHERE id = ? AND task_id = ?",
    )
    .bind(work_unit_id)
    .bind(task_id)
    .fetch_optional(db.pool())
    .await?
    {
        if let (Some(kind), Some(id)) = (kind, id) {
            contributors.push(actor_ref_from_parts(&kind, id)?);
        }
    }
    let executions = sqlx::query_as::<_, (String, String)>(
        "SELECT DISTINCT actor_kind, actor_id FROM execution
         WHERE task_id = ? AND work_unit_id = ?
           AND actor_kind IS NOT NULL AND actor_id IS NOT NULL",
    )
    .bind(task_id)
    .bind(work_unit_id)
    .fetch_all(db.pool())
    .await?;
    for (kind, id) in executions {
        let contributor = actor_ref_from_parts(&kind, id)?;
        if !contributors.contains(&contributor) {
            contributors.push(contributor);
        }
    }
    Ok((contributors.len() == 1).then(|| contributors.remove(0)))
}

fn actor_ref_from_parts(kind: &str, id: String) -> Result<ActorRef> {
    match kind {
        "human" => Ok(ActorRef::Human(id)),
        "agent" => Ok(ActorRef::Agent(id)),
        _ => Err(ServiceError::invalid_operation(
            "TaskRole ActorKind is not supported",
        )),
    }
}

async fn role_has_active_member_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_role_id: &str,
    actor: &ActorRef,
) -> Result<bool> {
    let actor_kind = match actor {
        ActorRef::Human(_) => "human",
        ActorRef::Agent(_) => "agent",
    };
    let actor_id = match actor {
        ActorRef::Human(id) | ActorRef::Agent(id) => id,
    };
    let active: i64 = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM role_membership
            WHERE task_role_id = ? AND actor_kind = ? AND actor_id = ?
              AND status = 'active'
        )",
    )
    .bind(task_role_id)
    .bind(actor_kind)
    .bind(actor_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(active != 0)
}

async fn role_has_capability_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_role_id: &str,
    capability: api_types::TaskRoleCapability,
) -> Result<bool> {
    let configuration: Option<String> = sqlx::query_scalar(
        "SELECT configuration_json FROM task_role_capability_configuration
         WHERE task_role_id = ?",
    )
    .bind(task_role_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(configuration) = configuration else {
        return Ok(false);
    };
    let configuration =
        crate::task_role_configuration::parse_task_role_configuration(&configuration)?;
    Ok(configuration.capabilities.contains(&capability))
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
        ("correction_role_id", input.correction_role_id.as_str()),
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

fn automatic_rework_idempotency_key(
    policy_digest: &str,
    report_id: &str,
    reviewed_work_unit_id: &str,
    reviewed_work_unit_version: i64,
    result_execution_id: Option<&str>,
    result_artifact_id: Option<&str>,
    workspace_snapshot_digest: Option<&str>,
) -> String {
    let mut digest = Sha256::new();
    let reviewed_version = reviewed_work_unit_version.to_string();
    for part in [
        "forge.automatic-rework.v1",
        policy_digest,
        report_id,
        reviewed_work_unit_id,
        reviewed_version.as_str(),
        result_execution_id.unwrap_or_default(),
        result_artifact_id.unwrap_or_default(),
        workspace_snapshot_digest.unwrap_or_default(),
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("automatic-rework-v1:{}", hex::encode(digest.finalize()))
}

fn is_automatic_rework_blocker(error: &ServiceError) -> bool {
    match error {
        ServiceError::InvalidOperation { .. }
        | ServiceError::AuthorizationDenied { .. }
        | ServiceError::NotFound { .. }
        | ServiceError::Conflict(_) => true,
        ServiceError::Db(db::DbError::Sqlx(_)) => false,
        ServiceError::Db(_) => true,
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutomaticReworkErrorDisposition {
    Deferred,
    Cancelled,
    Blocked,
    Unexpected,
}

fn classify_automatic_rework_error(error: &ServiceError) -> AutomaticReworkErrorDisposition {
    match error {
        ServiceError::Db(DbError::TaskDispatchPaused) => AutomaticReworkErrorDisposition::Deferred,
        ServiceError::Db(DbError::TaskDispatchCancelled) => {
            AutomaticReworkErrorDisposition::Cancelled
        }
        error if is_automatic_rework_blocker(error) => AutomaticReworkErrorDisposition::Blocked,
        _ => AutomaticReworkErrorDisposition::Unexpected,
    }
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
    // Agent profiles are reusable across responsibilities. Their native session
    // and context ancestry are isolated by execution_role_session_isolation_*.
    // Humans cannot obtain independence by changing a role label.
    if matches!(actor, ActorRef::Agent(_)) {
        return Ok(false);
    }
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

/// Return the durable review lineage for a Task, including every immutable
/// revision. The caller must authorize visibility of `task_id` first.
pub async fn list_review_lines(
    db: &SqliteDb,
    task_id: &str,
) -> Result<Vec<api_types::ReviewLineResponse>> {
    let rows = sqlx::query(
        "SELECT id, task_id, root_work_unit_id, current_work_unit_id,
                current_work_unit_version, current_result_execution_id,
                current_result_artifact_id, current_commit_sha, current_workspace_id,
                current_snapshot_digest, current_manifest_id, latest_report_artifact_id,
                latest_verdict, revision, created_at, updated_at
         FROM work_unit_review_line WHERE task_id = ? ORDER BY created_at, id",
    )
    .bind(task_id)
    .fetch_all(db.pool())
    .await?;

    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.try_get("id")?;
        let revisions = sqlx::query(
            "SELECT line_id, task_id, revision, work_unit_id, work_unit_version,
                    result_execution_id, result_artifact_id, result_commit_sha,
                    result_workspace_id, workspace_snapshot_digest, manifest_id,
                    review_report_artifact_id, verdict, created_at
             FROM work_unit_review_line_revision WHERE line_id = ? ORDER BY revision",
        )
        .bind(&id)
        .fetch_all(db.pool())
        .await?
        .into_iter()
        .map(|revision| {
            Ok(api_types::ReviewLineRevisionResponse {
                line_id: revision.try_get("line_id")?,
                task_id: revision.try_get("task_id")?,
                revision: revision.try_get("revision")?,
                work_unit_id: revision.try_get("work_unit_id")?,
                work_unit_version: revision.try_get("work_unit_version")?,
                result_execution_id: revision.try_get("result_execution_id")?,
                result_artifact_id: revision.try_get("result_artifact_id")?,
                result_commit_sha: revision.try_get("result_commit_sha")?,
                result_workspace_id: revision.try_get("result_workspace_id")?,
                workspace_snapshot_digest: revision.try_get("workspace_snapshot_digest")?,
                manifest_id: revision.try_get("manifest_id")?,
                review_report_artifact_id: revision.try_get("review_report_artifact_id")?,
                verdict: revision.try_get("verdict")?,
                created_at: revision.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        lines.push(api_types::ReviewLineResponse {
            id,
            task_id: row.try_get("task_id")?,
            root_work_unit_id: row.try_get("root_work_unit_id")?,
            current_work_unit_id: row.try_get("current_work_unit_id")?,
            current_work_unit_version: row.try_get("current_work_unit_version")?,
            current_result_execution_id: row.try_get("current_result_execution_id")?,
            current_result_artifact_id: row.try_get("current_result_artifact_id")?,
            current_commit_sha: row.try_get("current_commit_sha")?,
            current_workspace_id: row.try_get("current_workspace_id")?,
            current_snapshot_digest: row.try_get("current_snapshot_digest")?,
            current_manifest_id: row.try_get("current_manifest_id")?,
            latest_report_artifact_id: row.try_get("latest_report_artifact_id")?,
            latest_verdict: row.try_get("latest_verdict")?,
            revision: row.try_get("revision")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            history: revisions,
        });
    }
    Ok(lines)
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

#[cfg(test)]
mod tests {
    use super::{
        classify_automatic_rework_error, AutomaticReworkErrorDisposition,
        PENDING_AUTOMATIC_REWORK_SQL,
    };
    use crate::ServiceError;
    use db::DbError;
    use sqlx::Row;

    #[test]
    fn concurrent_pause_defers_but_cancellation_and_human_decisions_are_distinct() {
        assert_eq!(
            classify_automatic_rework_error(&DbError::TaskDispatchPaused.into()),
            AutomaticReworkErrorDisposition::Deferred
        );
        assert_eq!(
            classify_automatic_rework_error(&DbError::TaskDispatchCancelled.into()),
            AutomaticReworkErrorDisposition::Cancelled
        );
        assert_eq!(
            classify_automatic_rework_error(&ServiceError::invalid_operation(
                "human decision rejected the requested correction"
            )),
            AutomaticReworkErrorDisposition::Blocked
        );
    }

    #[tokio::test]
    async fn paused_intents_do_not_consume_the_pending_batch_limit() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("in-memory SQLite pool creates");
        sqlx::raw_sql(
            "CREATE TABLE task_rework_automation_intent (
                 id TEXT PRIMARY KEY, task_id TEXT NOT NULL,
                 state TEXT NOT NULL, created_at TEXT NOT NULL
             );
             CREATE TABLE task_dispatch (task_id TEXT PRIMARY KEY, state TEXT NOT NULL);
             CREATE TABLE task_lifecycle (task_id TEXT PRIMARY KEY, state TEXT NOT NULL);",
        )
        .execute(&pool)
        .await
        .expect("minimal pending queue schema creates");

        for index in 0..100 {
            let task_id = format!("paused-task-{index:03}");
            sqlx::query(
                "INSERT INTO task_rework_automation_intent (id, task_id, state, created_at)
                 VALUES (?, ?, 'pending', ?)",
            )
            .bind(format!("paused-intent-{index:03}"))
            .bind(&task_id)
            .bind(format!("2026-01-01T00:{:02}:00Z", index))
            .execute(&pool)
            .await
            .expect("paused intent inserts");
            sqlx::query("INSERT INTO task_dispatch (task_id, state) VALUES (?, 'paused')")
                .bind(&task_id)
                .execute(&pool)
                .await
                .expect("paused dispatch inserts");
            sqlx::query("INSERT INTO task_lifecycle (task_id, state) VALUES (?, 'ready')")
                .bind(task_id)
                .execute(&pool)
                .await
                .expect("paused lifecycle inserts");
        }

        sqlx::raw_sql(
            "INSERT INTO task_rework_automation_intent (id, task_id, state, created_at)
             VALUES ('active-intent', 'active-task', 'pending', '2026-01-02T00:00:00Z');
             INSERT INTO task_dispatch (task_id, state) VALUES ('active-task', 'running');
             INSERT INTO task_lifecycle (task_id, state) VALUES ('active-task', 'active');
             INSERT INTO task_rework_automation_intent (id, task_id, state, created_at)
             VALUES ('cancelled-intent', 'cancelled-task', 'pending', '2026-01-02T00:01:00Z');
             INSERT INTO task_dispatch (task_id, state) VALUES ('cancelled-task', 'cancelled');
             INSERT INTO task_lifecycle (task_id, state) VALUES ('cancelled-task', 'cancelled');",
        )
        .execute(&pool)
        .await
        .expect("active intent follows the paused backlog");

        let rows = sqlx::query(PENDING_AUTOMATIC_REWORK_SQL)
            .bind(100_i64)
            .fetch_all(&pool)
            .await
            .expect("eligible pending batch loads");
        assert_eq!(rows.len(), 2);
        let states = rows
            .iter()
            .map(|row| {
                (
                    row.try_get::<String, _>("task_id").unwrap(),
                    row.try_get::<String, _>("dispatch_state").unwrap(),
                    row.try_get::<String, _>("lifecycle_state").unwrap(),
                )
            })
            .collect::<std::collections::HashSet<_>>();
        assert!(states.contains(&(
            "active-task".to_owned(),
            "running".to_owned(),
            "active".to_owned()
        )));
        assert!(states.contains(&(
            "cancelled-task".to_owned(),
            "cancelled".to_owned(),
            "cancelled".to_owned()
        )));
    }
}
