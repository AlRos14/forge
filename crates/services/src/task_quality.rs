//! Durable quality-request routing and resolution.
//!
//! A quality request records the reviewer/human finding and is then consumed
//! into real WorkUnits or a coordination turn. The consumer is idempotent:
//! WorkUnit IDs and coordination request keys derive from the quality request,
//! and every state transition uses the request version as its fence.

use std::sync::Arc;

use db::{
    new_uuid_v4, now_rfc3339, ArtifactKind, CollaborationRepo, CreateDomainEvent, CreateWorkUnit,
    DbError, DomainEvent, RoleMembershipRepo, RoleMembershipStatus, SqliteDb, TaskCoordinationMode,
    TaskCoordinationRepo, TaskCoordinationRequest, TaskCoordinationRequestState,
    TaskQualityCategory, TaskQualityRequest, TaskQualityState, TaskQualityTargetKind, TaskRepo,
    TaskRoleRepo, TransitionWorkUnit, UpdateTaskQualityRequestState, WorkUnit, WorkUnitProvenance,
    WorkUnitRepo, WorkUnitStatus,
};
use events::EventBus;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    domain_event_service::DomainEventService,
    task_coordination::{CoordinationBrief, TaskCoordinationService, TurnRequestInput},
    Result, ServiceError,
};

#[derive(Clone)]
pub struct TaskQualityService {
    db: Arc<SqliteDb>,
    domain_events: DomainEventService,
    coordination: TaskCoordinationService,
}

impl TaskQualityService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            domain_events: DomainEventService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            coordination: TaskCoordinationService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            db,
        }
    }

    /// Classify a committed event as a quality-routing fact. The event is only
    /// a hint: the request and its exact target authority are re-read here.
    pub async fn process_domain_event(&self, event: &DomainEvent) -> Result<bool> {
        match event.event_type.as_str() {
            "task.quality_requested" if event.entity_type == "task_quality_request" => {
                self.process_request(&event.entity_id).await?;
                Ok(true)
            }
            "work_unit.completed"
            | "work_unit.cancelled"
            | "execution.completed"
            | "task.coordination_plan_approved"
            | "task.coordination_plan_materialized" => {
                if event.scope_type != "task" {
                    return Ok(false);
                }
                self.reconcile_task(&event.scope_id, 100).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Periodic durable scan. A paused Task keeps its request in a non-terminal
    /// state; resume and restart both converge through this method.
    pub async fn process_pending(&self, limit: i64) -> Result<usize> {
        let rows = sqlx::query(
            "SELECT id FROM task_quality_request
             WHERE state IN ('requested', 'notified', 'work_created', 'running')
             ORDER BY created_at, id
             LIMIT ?",
        )
        .bind(limit.clamp(1, 100))
        .fetch_all(self.db.pool())
        .await?;
        let mut processed = 0;
        for row in rows {
            let id: String = row.try_get("id")?;
            match self.process_request(&id).await {
                Ok(_) => processed += 1,
                Err(ServiceError::Db(DbError::VersionConflict)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(processed)
    }

    async fn reconcile_task(&self, task_id: &str, limit: i64) -> Result<()> {
        let rows = sqlx::query(
            "SELECT id FROM task_quality_request
             WHERE task_id = ? AND state IN ('work_created', 'running')
             ORDER BY created_at, id LIMIT ?",
        )
        .bind(task_id)
        .bind(limit.clamp(1, 100))
        .fetch_all(self.db.pool())
        .await?;
        for row in rows {
            let id: String = row.try_get("id")?;
            match self.process_request(&id).await {
                Ok(_) => {}
                Err(ServiceError::Db(DbError::VersionConflict)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn process_request(&self, request_id: &str) -> Result<()> {
        let Some(request) =
            TaskCoordinationRepo::get_task_quality_request(&*self.db, request_id).await?
        else {
            return Ok(());
        };
        match request.state {
            TaskQualityState::Requested | TaskQualityState::Notified => {
                self.route_request(&request).await
            }
            TaskQualityState::WorkCreated | TaskQualityState::Running => {
                self.reconcile_request(&request).await
            }
            TaskQualityState::Resolved
            | TaskQualityState::RequiresHuman
            | TaskQualityState::Cancelled => Ok(()),
        }
    }

    async fn route_request(&self, request: &TaskQualityRequest) -> Result<()> {
        if request.category == TaskQualityCategory::HumanDecision
            || request.target_kind == TaskQualityTargetKind::Human
        {
            self.transition(
                request,
                TaskQualityState::RequiresHuman,
                None,
                None,
                None,
                Some("coordination.requires_human"),
                "the quality request requires an exact Human decision",
            )
            .await?;
            return Ok(());
        }
        let Some(role_id) = request.target_task_role_id.as_deref() else {
            self.transition(
                request,
                TaskQualityState::RequiresHuman,
                None,
                None,
                None,
                Some("coordination.target_missing"),
                "the quality request has no exact TaskRole target",
            )
            .await?;
            return Ok(());
        };
        let Some(role) = TaskRoleRepo::get_by_id(&*self.db, role_id)
            .await?
            .filter(|role| role.task_id == request.task_id)
        else {
            self.transition(
                request,
                TaskQualityState::RequiresHuman,
                None,
                None,
                None,
                Some("coordination.target_missing"),
                "the target TaskRole no longer exists",
            )
            .await?;
            return Ok(());
        };
        if let Some(actor) = request.target_actor.as_ref() {
            let members = RoleMembershipRepo::list_by_role(&*self.db, &role.id, false).await?;
            if !members.iter().any(|member| {
                member.status == RoleMembershipStatus::Active && member.actor_ref() == *actor
            }) {
                self.transition(
                    request,
                    TaskQualityState::RequiresHuman,
                    None,
                    None,
                    None,
                    Some("coordination.not_authorized"),
                    "the target Actor is no longer an active member of the target TaskRole",
                )
                .await?;
                return Ok(());
            }
        }

        match request.category {
            TaskQualityCategory::DeliveryCorrection => {
                match self.rework_correction_for(request).await? {
                    Some((state, Some(work_unit_id))) if state == "completed" => {
                        self.transition(
                            request,
                            TaskQualityState::WorkCreated,
                            Some(Some(work_unit_id)),
                            None,
                            None,
                            None,
                            "the exact rework round now owns this correction",
                        )
                        .await?;
                    }
                    Some((state, _)) if matches!(state.as_str(), "pending" | "completed") => {
                        if request.state != TaskQualityState::Running {
                            self.transition(
                                request,
                                TaskQualityState::Running,
                                None,
                                None,
                                None,
                                None,
                                "the exact rework round is preparing the correction",
                            )
                            .await?;
                        }
                    }
                    Some((state, _)) => {
                        self.transition(
                            request,
                            TaskQualityState::RequiresHuman,
                            None,
                            None,
                            None,
                            Some("coordination.rework_unavailable"),
                            &format!("the rework intent is {state}; Human routing is required"),
                        )
                        .await?;
                    }
                    None => {
                        // A delivery correction must reuse the exact rework
                        // pair: reviewed subject, result pins, repository
                        // policy, and an independent review. A generic WorkUnit
                        // would silently replace all four guarantees.
                        self.transition(
                            request,
                            TaskQualityState::RequiresHuman,
                            None,
                            None,
                            None,
                            Some("coordination.rework_unavailable"),
                            "a delivery correction requires an exact ReviewReport and rework receipt or intent",
                        )
                        .await?;
                    }
                }
            }
            TaskQualityCategory::Specialist => {
                let unit = match self.create_quality_work_unit(request, &role.role).await {
                    Ok(unit) => unit,
                    Err(error) => {
                        self.transition(
                            request,
                            TaskQualityState::RequiresHuman,
                            None,
                            None,
                            None,
                            Some("coordination.work_creation_failed"),
                            &error.to_string(),
                        )
                        .await?;
                        return Ok(());
                    }
                };
                self.transition(
                    request,
                    TaskQualityState::WorkCreated,
                    Some(Some(unit.id)),
                    None,
                    None,
                    None,
                    "the specialist WorkUnit was created",
                )
                .await?;
            }
            TaskQualityCategory::PlanRevision => {
                let coordination =
                    TaskCoordinationRepo::get_task_coordination(&*self.db, &request.task_id)
                        .await?
                        .ok_or_else(|| {
                            ServiceError::not_found("task coordination", request.task_id.clone())
                        })?;
                let Some(authorizer) = coordination.requested_by.as_deref() else {
                    self.transition(
                        request,
                        TaskQualityState::RequiresHuman,
                        None,
                        None,
                        None,
                        Some("coordination.not_authorized"),
                        "the Task has no Human coordination authorizer",
                    )
                    .await?;
                    return Ok(());
                };
                if coordination.mode == TaskCoordinationMode::Manual {
                    self.transition(
                        request,
                        TaskQualityState::RequiresHuman,
                        None,
                        None,
                        None,
                        Some("coordination.not_authorized"),
                        "coordination is manual; a plan revision requires Human action",
                    )
                    .await?;
                    return Ok(());
                }
                let task_version = TaskRepo::get_by_id(&*self.db, &request.task_id, false)
                    .await?
                    .map(|task| task.version)
                    .unwrap_or_default();
                let brief = revision_brief(request);
                let turn = match self
                    .coordination
                    .request_turn(
                        &request.task_id,
                        authorizer,
                        TurnRequestInput {
                            expected_task_version: task_version,
                            expected_coordination_version: coordination.authorization_version,
                            brief,
                            idempotency_key: format!("task-quality:{}", request.id),
                        },
                    )
                    .await
                {
                    Ok((turn, _)) => turn,
                    Err(error) => {
                        let (code, message) = match &error {
                            ServiceError::Coordination { code, message } => {
                                (*code, message.clone())
                            }
                            other => ("coordination.plan_request_failed", other.to_string()),
                        };
                        self.transition(
                            request,
                            TaskQualityState::RequiresHuman,
                            None,
                            None,
                            None,
                            Some(code),
                            &message,
                        )
                        .await?;
                        return Ok(());
                    }
                };
                self.transition(
                    request,
                    TaskQualityState::WorkCreated,
                    None,
                    Some(Some(turn.id)),
                    None,
                    None,
                    "the plan-revision coordination turn was requested",
                )
                .await?;
            }
            TaskQualityCategory::HumanDecision => unreachable!("handled above"),
        }
        Ok(())
    }

    async fn is_exact_rework_correction(&self, work_unit_id: &str) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM task_rework_receipt
                WHERE correction_work_unit_id = ?
                UNION ALL
                SELECT 1 FROM task_rework_automation_intent
                WHERE correction_work_unit_id = ?
            )",
        )
        .bind(work_unit_id)
        .bind(work_unit_id)
        .fetch_one(self.db.pool())
        .await?;
        Ok(exists)
    }

    async fn correction_for_review_report(
        &self,
        task_id: &str,
        report_id: &str,
    ) -> Result<Option<String>> {
        let manual: Option<String> = sqlx::query_scalar(
            "SELECT correction_work_unit_id FROM task_rework_receipt
             WHERE task_id = ? AND review_report_artifact_id = ?",
        )
        .bind(task_id)
        .bind(report_id)
        .fetch_optional(self.db.pool())
        .await?;
        if manual.is_some() {
            return Ok(manual);
        }
        let automation: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT state, correction_work_unit_id
             FROM task_rework_automation_intent
             WHERE task_id = ? AND review_report_artifact_id = ?",
        )
        .bind(task_id)
        .bind(report_id)
        .fetch_optional(self.db.pool())
        .await?;
        Ok(automation
            .and_then(|(state, correction)| (state == "completed").then_some(correction).flatten()))
    }

    async fn rework_correction_for(
        &self,
        request: &TaskQualityRequest,
    ) -> Result<Option<(String, Option<String>)>> {
        if request.source_kind != "review_report" {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT state, correction_work_unit_id
             FROM task_rework_automation_intent
             WHERE task_id = ? AND review_report_artifact_id = ?",
        )
        .bind(&request.task_id)
        .bind(&request.source_id)
        .fetch_optional(self.db.pool())
        .await?;
        if let Some(row) = row {
            let state: String = row.try_get("state")?;
            let correction: Option<String> = row.try_get("correction_work_unit_id")?;
            return Ok(Some((state, correction)));
        }
        let manual: Option<String> = sqlx::query_scalar(
            "SELECT correction_work_unit_id FROM task_rework_receipt
             WHERE task_id = ? AND review_report_artifact_id = ?",
        )
        .bind(&request.task_id)
        .bind(&request.source_id)
        .fetch_optional(self.db.pool())
        .await?;
        Ok(manual.map(|id| ("completed".to_owned(), Some(id))))
    }

    async fn create_quality_work_unit(
        &self,
        request: &TaskQualityRequest,
        role: &str,
    ) -> Result<WorkUnit> {
        let id = deterministic_quality_work_unit_id(&request.id)?;
        if let Some(existing) = WorkUnitRepo::get_by_id(&*self.db, &id).await? {
            if existing.task_id == request.task_id {
                return Ok(existing);
            }
            return Err(ServiceError::conflict(
                "quality WorkUnit identity conflicts with another Task",
            ));
        }
        let now = now_rfc3339();
        let findings = serde_json::from_str::<Vec<String>>(&request.findings_json)
            .unwrap_or_default()
            .join("\n- ");
        let title = bounded_text(&format!("Quality: {}", request.summary), 512);
        let scope = bounded_text(&request.resolution_condition, 4096);
        let instructions = bounded_text(
            &format!(
                "Address the durable quality request {}.\nSummary: {}\nFindings:\n- {}\nResolution condition: {}\nSource: {} {}",
                request.id,
                request.summary,
                findings,
                request.resolution_condition,
                request.source_kind,
                request.source_id,
            ),
            64 * 1024,
        );
        let artifact_inputs = if request.source_kind == "review_report" {
            let source = CollaborationRepo::get_artifact(&*self.db, &request.source_id)
                .await?
                .filter(|artifact| {
                    artifact.task_id == request.task_id
                        && artifact.kind == ArtifactKind::ReviewReport
                        && artifact
                            .digest
                            .as_deref()
                            .is_some_and(|digest| !digest.is_empty())
                });
            if source.is_none() {
                return Err(ServiceError::invalid_operation(
                    "the ReviewReport source is no longer an exact digest-bound Artifact",
                ));
            }
            vec![request.source_id.clone()]
        } else {
            Vec::new()
        };
        let event = CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "work_unit.created".to_owned(),
            entity_type: "work_unit".to_owned(),
            entity_id: id.clone(),
            actor_type: request.created_by.kind().to_string(),
            actor_id: Some(request.created_by.id().to_owned()),
            scope_type: "task".to_owned(),
            scope_id: request.task_id.clone(),
            correlation_id: request.id.clone(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("task-quality-work-unit:{}", request.id)),
            payload_json: json!({
                "task_id": request.task_id,
                "work_unit_id": id,
                "version": 1,
                "status": "open",
                "quality_request_id": request.id,
            })
            .to_string(),
            created_at: now.clone(),
        };
        let write = WorkUnitRepo::create(
            &*self.db,
            CreateWorkUnit {
                id,
                task_id: request.task_id.clone(),
                parent_work_unit_id: None,
                title,
                scope,
                execution_instructions: Some(instructions),
                role: role.to_owned(),
                assigned_actor: request.target_actor.clone(),
                requires_integration: false,
                provenance: (request.source_kind == "review_report")
                    .then(|| WorkUnitProvenance::Artifact(request.source_id.clone())),
                artifact_input_ids: artifact_inputs,
                review_approval_gate_id: None,
                review_approval_line_id: None,
                created_by: request.created_by.clone(),
                created_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    async fn reconcile_request(&self, request: &TaskQualityRequest) -> Result<()> {
        if request.work_unit_id.is_none()
            && request.coordination_request_id.is_none()
            && request.category == TaskQualityCategory::DeliveryCorrection
            && request.state == TaskQualityState::Running
        {
            match self.rework_correction_for(request).await? {
                Some((state, Some(work_unit_id))) if state == "completed" => {
                    return self
                        .transition(
                            request,
                            TaskQualityState::WorkCreated,
                            Some(Some(work_unit_id)),
                            None,
                            None,
                            None,
                            "the existing rework round now owns this correction",
                        )
                        .await;
                }
                Some((state, _)) if matches!(state.as_str(), "pending" | "completed") => {
                    return Ok(());
                }
                Some((state, _)) => {
                    return self
                        .transition(
                            request,
                            TaskQualityState::RequiresHuman,
                            None,
                            None,
                            None,
                            Some("coordination.rework_unavailable"),
                            &format!("the rework intent is {state}; Human routing is required"),
                        )
                        .await;
                }
                None => {}
            }
        }
        if let Some(work_unit_id) = request.work_unit_id.as_deref() {
            let Some(unit) = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
                .await?
                .filter(|unit| unit.task_id == request.task_id)
            else {
                self.transition(
                    request,
                    TaskQualityState::RequiresHuman,
                    None,
                    None,
                    None,
                    Some("coordination.work_missing"),
                    "the linked WorkUnit no longer exists",
                )
                .await?;
                return Ok(());
            };
            if request.category == TaskQualityCategory::DeliveryCorrection
                && !self.is_exact_rework_correction(&unit.id).await?
            {
                return self
                    .transition(
                        request,
                        TaskQualityState::RequiresHuman,
                        None,
                        None,
                        None,
                        Some("coordination.rework_unavailable"),
                        "the linked WorkUnit is not an exact rework correction",
                    )
                    .await;
            }
            return match unit.status {
                WorkUnitStatus::Completed => {
                    let has_dependent_review: bool = sqlx::query_scalar(
                        "SELECT EXISTS(
                            SELECT 1 FROM work_unit_review_subject
                            WHERE task_id = ? AND reviewed_work_unit_id = ?
                        )",
                    )
                    .bind(&request.task_id)
                    .bind(&unit.id)
                    .fetch_one(self.db.pool())
                    .await?;
                    if has_dependent_review {
                        return match self.dependent_review_verdict(&unit).await? {
                            Some((verdict, report_id, digest)) if verdict == "pass" => {
                                self.transition(
                                    request,
                                    TaskQualityState::Resolved,
                                    None,
                                    None,
                                    Some(("review_report".to_owned(), report_id, digest)),
                                    None,
                                    "the independent review passed the corrected result",
                                )
                                .await
                            }
                            Some((verdict, report_id, _)) if verdict == "request_changes" => {
                                if let Some(next) =
                                    self.correction_for_review_report(&request.task_id, &report_id)
                                        .await?
                                {
                                    if next != unit.id {
                                        return self
                                            .transition(
                                                request,
                                                TaskQualityState::WorkCreated,
                                                Some(Some(next)),
                                                None,
                                                None,
                                                None,
                                                "the quality request advanced to the next exact rework round",
                                            )
                                            .await;
                                    }
                                }
                                if request.state == TaskQualityState::Running {
                                    return Ok(());
                                }
                                self.transition(
                                    request,
                                    TaskQualityState::Running,
                                    None,
                                    None,
                                    None,
                                    None,
                                    "the independent review requested another exact rework round",
                                )
                                .await
                            }
                            Some((verdict, _, _)) => {
                                self.transition(
                                    request,
                                    TaskQualityState::RequiresHuman,
                                    None,
                                    None,
                                    None,
                                    Some("coordination.review_blocked"),
                                    &format!(
                                        "the independent review returned {verdict}; Human routing is required"
                                    ),
                                )
                                .await
                            }
                            None => {
                                if request.state == TaskQualityState::Running {
                                    return Ok(());
                                }
                                self.transition(
                                    request,
                                    TaskQualityState::Running,
                                    None,
                                    None,
                                    None,
                                    None,
                                    "the corrected result is awaiting independent review",
                                )
                                .await
                            }
                        };
                    }
                    if request.category == TaskQualityCategory::DeliveryCorrection {
                        // Every delivery correction must retain its independent
                        // review subject; completion alone is not resolution.
                        return self
                            .transition(
                                request,
                                TaskQualityState::RequiresHuman,
                                None,
                                None,
                                None,
                                Some("coordination.review_missing"),
                                "the correction has no independent review subject",
                            )
                            .await;
                    }
                    if let Some((kind, reference, digest)) =
                        self.exact_work_unit_result(&unit).await?
                    {
                        self.transition(
                            request,
                            TaskQualityState::Resolved,
                            None,
                            None,
                            Some((kind, reference, digest)),
                            None,
                            "the linked WorkUnit completed with an exact result",
                        )
                        .await?;
                    } else {
                        self.transition(
                            request,
                            TaskQualityState::RequiresHuman,
                            None,
                            None,
                            None,
                            Some("coordination.result_missing"),
                            "the linked WorkUnit completed without an exact Execution result",
                        )
                        .await?;
                    }
                    Ok(())
                }
                WorkUnitStatus::Cancelled => {
                    self.transition(
                        request,
                        TaskQualityState::Cancelled,
                        None,
                        None,
                        None,
                        Some("coordination.work_cancelled"),
                        "the linked WorkUnit was cancelled",
                    )
                    .await
                }
                WorkUnitStatus::Open => {
                    let active: bool = sqlx::query_scalar(
                        "SELECT EXISTS(
                            SELECT 1 FROM execution
                            WHERE task_id = ? AND work_unit_id = ? AND status = 'running'
                        )",
                    )
                    .bind(&request.task_id)
                    .bind(&unit.id)
                    .fetch_one(self.db.pool())
                    .await?;
                    if active {
                        if request.state == TaskQualityState::Running {
                            return Ok(());
                        }
                        return self
                            .transition(
                                request,
                                TaskQualityState::Running,
                                None,
                                None,
                                None,
                                None,
                                "the linked WorkUnit has a Running Execution",
                            )
                            .await;
                    }
                    let has_dependent_review: bool = sqlx::query_scalar(
                        "SELECT EXISTS(
                            SELECT 1 FROM work_unit_review_subject
                            WHERE task_id = ? AND reviewed_work_unit_id = ?
                        )",
                    )
                    .bind(&request.task_id)
                    .bind(&unit.id)
                    .fetch_one(self.db.pool())
                    .await?;
                    if has_dependent_review {
                        if request.state == TaskQualityState::Running {
                            return Ok(());
                        }
                        return self
                            .transition(
                                request,
                                TaskQualityState::Running,
                                None,
                                None,
                                None,
                                None,
                                "the corrected result is awaiting independent review",
                            )
                            .await;
                    }
                    // The authorized quality route completes the exact work
                    // item once its Execution finished and no newer attempt is
                    // active. This is technical completion, not review pass.
                    let completed_execution: Option<(String, Option<String>)> = sqlx::query_as(
                        "SELECT id, after_sha FROM execution
                         WHERE task_id = ? AND work_unit_id = ? AND status = 'completed'
                           AND work_unit_version = ?
                         ORDER BY created_at DESC, id DESC LIMIT 1",
                    )
                    .bind(&request.task_id)
                    .bind(&unit.id)
                    .bind(unit.version)
                    .fetch_optional(self.db.pool())
                    .await?;
                    let Some((execution_id, after_sha)) = completed_execution else {
                        return Ok(());
                    };
                    let now = now_rfc3339();
                    let event = CreateDomainEvent {
                        id: new_uuid_v4(),
                        event_type: "work_unit.completed".to_owned(),
                        entity_type: "work_unit".to_owned(),
                        entity_id: unit.id.clone(),
                        actor_type: "system".to_owned(),
                        actor_id: None,
                        scope_type: "task".to_owned(),
                        scope_id: request.task_id.clone(),
                        correlation_id: request.id.clone(),
                        causation_id: None,
                        causation_depth: 0,
                        dedupe_key: Some(format!("task-quality-work-unit-completed:{}", unit.id)),
                        payload_json: json!({
                            "task_id": request.task_id,
                            "work_unit_id": unit.id,
                            "version": unit.version + 1,
                            "status": "completed",
                            "quality_request_id": request.id,
                            "execution_id": execution_id,
                        })
                        .to_string(),
                        created_at: now.clone(),
                    };
                    let write = WorkUnitRepo::transition(
                        &*self.db,
                        TransitionWorkUnit {
                            id: unit.id.clone(),
                            expected_version: unit.version,
                            status: WorkUnitStatus::Completed,
                            updated_at: now,
                            reviewed_result: None,
                        },
                        event,
                    )
                    .await?;
                    self.domain_events.publish_committed(&write.event);
                    let digest = sha256_hex(
                        json!({
                            "work_unit_id": unit.id,
                            "work_unit_version": unit.version,
                            "execution_id": execution_id,
                            "after_sha": after_sha,
                        })
                        .to_string()
                        .as_bytes(),
                    );
                    self.transition(
                        request,
                        TaskQualityState::Resolved,
                        None,
                        None,
                        Some(("execution".to_owned(), execution_id, digest)),
                        None,
                        "the quality WorkUnit completed technically",
                    )
                    .await
                }
            };
        }
        if let Some(coordination_request_id) = request.coordination_request_id.as_deref() {
            let Some(turn) = TaskCoordinationRepo::get_task_coordination_request(
                &*self.db,
                coordination_request_id,
            )
            .await?
            .filter(|turn| turn.task_id == request.task_id) else {
                self.transition(
                    request,
                    TaskQualityState::RequiresHuman,
                    None,
                    None,
                    None,
                    Some("coordination.request_missing"),
                    "the linked coordination turn no longer exists",
                )
                .await?;
                return Ok(());
            };
            return self.reconcile_plan_revision(request, &turn).await;
        }
        self.transition(
            request,
            TaskQualityState::RequiresHuman,
            None,
            None,
            None,
            Some("coordination.work_missing"),
            "the quality request has no linked WorkUnit or coordination turn",
        )
        .await
    }

    async fn reconcile_plan_revision(
        &self,
        request: &TaskQualityRequest,
        turn: &TaskCoordinationRequest,
    ) -> Result<()> {
        if let Some(plan_artifact_id) = turn.plan_artifact_id.as_deref() {
            if TaskCoordinationRepo::get_task_coordination_materialization(
                &*self.db,
                plan_artifact_id,
            )
            .await?
            .is_some()
            {
                let plan =
                    TaskCoordinationRepo::get_task_coordination_plan(&*self.db, plan_artifact_id)
                        .await?
                        .filter(|plan| plan.task_id == request.task_id);
                if let Some(plan) = plan {
                    return self
                        .transition(
                            request,
                            TaskQualityState::Resolved,
                            None,
                            None,
                            Some(("plan".to_owned(), plan.artifact_id, plan.plan_digest)),
                            None,
                            "the plan revision was materialized",
                        )
                        .await;
                }
            }
        }
        match turn.state {
            TaskCoordinationRequestState::Materialized => {
                let Some(plan_artifact_id) = turn.plan_artifact_id.as_deref() else {
                    return self
                        .transition(
                            request,
                            TaskQualityState::RequiresHuman,
                            None,
                            None,
                            None,
                            Some("coordination.plan_missing"),
                            "the materialized coordination turn has no plan Artifact",
                        )
                        .await;
                };
                let plan =
                    TaskCoordinationRepo::get_task_coordination_plan(&*self.db, plan_artifact_id)
                        .await?
                        .filter(|plan| plan.task_id == request.task_id);
                let Some(plan) = plan else {
                    return self
                        .transition(
                            request,
                            TaskQualityState::RequiresHuman,
                            None,
                            None,
                            None,
                            Some("coordination.plan_missing"),
                            "the linked plan row no longer exists",
                        )
                        .await;
                };
                self.transition(
                    request,
                    TaskQualityState::Resolved,
                    None,
                    None,
                    Some(("plan".to_owned(), plan.artifact_id, plan.plan_digest)),
                    None,
                    "the plan revision was materialized",
                )
                .await
            }
            TaskCoordinationRequestState::PlanReady
            | TaskCoordinationRequestState::AwaitingApproval => {
                if request.state == TaskQualityState::Running {
                    return Ok(());
                }
                self.transition(
                    request,
                    TaskQualityState::Running,
                    None,
                    None,
                    None,
                    None,
                    "the plan revision is waiting for approval",
                )
                .await
            }
            TaskCoordinationRequestState::Failed
            | TaskCoordinationRequestState::Cancelled
            | TaskCoordinationRequestState::Uncertain => {
                self.transition(
                    request,
                    TaskQualityState::RequiresHuman,
                    None,
                    None,
                    None,
                    Some("coordination.plan_failed"),
                    "the plan revision did not produce a usable plan",
                )
                .await
            }
            TaskCoordinationRequestState::Requested | TaskCoordinationRequestState::Running => {
                if request.state == TaskQualityState::Running {
                    return Ok(());
                }
                self.transition(
                    request,
                    TaskQualityState::Running,
                    None,
                    None,
                    None,
                    None,
                    "the plan revision turn is running",
                )
                .await
            }
        }
    }

    async fn dependent_review_verdict(
        &self,
        unit: &WorkUnit,
    ) -> Result<Option<(String, String, String)>> {
        let row = sqlx::query(
            "SELECT report.id AS report_id, report.digest AS report_digest,
                    json_extract(report.content, '$.verdict') AS verdict
             FROM work_unit_review_subject subject
             JOIN execution result_execution
               ON result_execution.work_unit_id = subject.reviewed_work_unit_id
              AND result_execution.task_id = subject.task_id
              AND result_execution.work_unit_version = subject.reviewed_work_unit_version
              AND result_execution.status = 'completed'
             JOIN work_unit review_unit
               ON review_unit.id = subject.review_work_unit_id
              AND review_unit.task_id = subject.task_id
             JOIN execution review_execution
               ON review_execution.work_unit_id = review_unit.id
              AND review_execution.task_id = review_unit.task_id
              AND review_execution.status = 'completed'
             JOIN artifact_execution_producer producer
               ON producer.execution_id = review_execution.id
              AND producer.task_id = review_unit.task_id
             JOIN artifact report
               ON report.id = producer.artifact_id
              AND report.task_id = producer.task_id
              AND report.kind = 'review_report'
             WHERE subject.task_id = ? AND subject.reviewed_work_unit_id = ?
             ORDER BY review_execution.created_at DESC, review_execution.id DESC,
                      report.id DESC
             LIMIT 1",
        )
        .bind(&unit.task_id)
        .bind(&unit.id)
        .fetch_optional(self.db.pool())
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let verdict: Option<String> = row.try_get("verdict")?;
        let report_id: String = row.try_get("report_id")?;
        let report_digest: Option<String> = row.try_get("report_digest")?;
        Ok(verdict.map(|verdict| {
            (
                verdict,
                report_id,
                report_digest.unwrap_or_else(|| sha256_hex(b"review_report")),
            )
        }))
    }

    async fn exact_work_unit_result(
        &self,
        unit: &WorkUnit,
    ) -> Result<Option<(String, String, String)>> {
        let row = sqlx::query(
            "SELECT id, after_sha FROM execution
             WHERE task_id = ? AND work_unit_id = ? AND status = 'completed'
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(&unit.task_id)
        .bind(&unit.id)
        .fetch_optional(self.db.pool())
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let execution_id: String = row.try_get("id")?;
        let after_sha: Option<String> = row.try_get("after_sha")?;
        let digest = sha256_hex(
            json!({
                "work_unit_id": unit.id,
                "work_unit_version": unit.version,
                "execution_id": execution_id,
                "after_sha": after_sha,
            })
            .to_string()
            .as_bytes(),
        );
        Ok(Some(("execution".to_owned(), execution_id, digest)))
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition(
        &self,
        request: &TaskQualityRequest,
        state: TaskQualityState,
        work_unit_id: Option<Option<String>>,
        coordination_request_id: Option<Option<String>>,
        resolution: Option<(String, String, String)>,
        failure_code: Option<&str>,
        message: &str,
    ) -> Result<()> {
        let now = now_rfc3339();
        let (resolution_kind, resolution_ref, resolution_digest) = match resolution {
            Some((kind, reference, digest)) => {
                (Some(Some(kind)), Some(Some(reference)), Some(Some(digest)))
            }
            None => (None, None, None),
        };
        let event = CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "task.quality_request_state_changed".to_owned(),
            entity_type: "task_quality_request".to_owned(),
            entity_id: request.id.clone(),
            actor_type: "system".to_owned(),
            actor_id: None,
            scope_type: "task".to_owned(),
            scope_id: request.task_id.clone(),
            correlation_id: request.id.clone(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!(
                "task-quality-state:{}:{}",
                request.id,
                request.version + 1
            )),
            payload_json: json!({
                "task_id": request.task_id,
                "quality_request_id": request.id,
                "state": state.to_string(),
                "message": message,
                "failure_code": failure_code,
            })
            .to_string(),
            created_at: now.clone(),
        };
        TaskCoordinationRepo::update_task_quality_request_state(
            &*self.db,
            UpdateTaskQualityRequestState {
                id: request.id.clone(),
                expected_version: request.version,
                state,
                handoff_id: None,
                work_unit_id,
                coordination_request_id,
                resolution_kind,
                resolution_ref,
                resolution_digest,
                requires_human_reason_code: if state == TaskQualityState::RequiresHuman {
                    Some(Some(
                        failure_code
                            .unwrap_or("coordination.requires_human")
                            .to_owned(),
                    ))
                } else {
                    None
                },
                requires_human_reason: if state == TaskQualityState::RequiresHuman {
                    Some(Some(message.to_owned()))
                } else {
                    None
                },
                updated_at: now,
                event: Some(event),
            },
        )
        .await?;
        self.domain_events
            .publish_by_dedupe(&format!(
                "task-quality-state:{}:{}",
                request.id,
                request.version + 1
            ))
            .await?;
        Ok(())
    }
}

fn revision_brief(request: &TaskQualityRequest) -> CoordinationBrief {
    let findings = serde_json::from_str::<Vec<String>>(&request.findings_json).unwrap_or_default();
    CoordinationBrief {
        objective: bounded_text(
            &format!(
                "Revise the Task plan to address quality request {}: {}",
                request.id, request.summary
            ),
            65_536,
        ),
        constraints: (!findings.is_empty()).then(|| bounded_text(&findings.join("\n"), 65_536)),
        acceptance_criteria: vec![bounded_text(&request.resolution_condition, 4096)],
        context: Some(bounded_text(
            &format!(
                "Source: {} {}. The quality request is durable and must reach a plan that addresses its resolution condition.",
                request.source_kind, request.source_id
            ),
            65_536,
        )),
    }
}

fn deterministic_quality_work_unit_id(request_id: &str) -> Result<String> {
    let digest = Sha256::digest(
        [
            b"forge.task-coordination.quality-work-unit.v1".as_slice(),
            request_id.as_bytes(),
        ]
        .concat(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes).to_string())
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
