use std::path::Path;

use api_types::ActorRef;
use db::{
    new_uuid_v4, now_rfc3339, ActorKind, AdvanceMicroFixReceiptBounds, CreateDomainEvent,
    CreateMicroFixReceipt, DomainEventRepo, ExecutionRepo, GateRepo, MicroFixBoundsOutcome,
    MicroFixBoundsViolation, MicroFixReceipt, MicroFixReceiptRepo, MicroFixSourceKind,
    TaskCoordination, TaskCoordinationMode, TaskCoordinationRepo, TaskRoleRepo, WorkUnitRepo,
    WorkUnitStatus, WorkspaceRepo,
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;
use tokio::process::Command;

use super::{ReviewFindingsDecision, TaskReworkService};
use crate::{
    coordinator_outputs::MicroFixOutput, domain_event_service::DomainEventService,
    task_coordination::CoordinationEnvelope, Result, ServiceError,
};

fn map_micro_fix_receipt_error(error: db::DbError) -> ServiceError {
    if matches!(
        &error,
        db::DbError::Check(message) if message == "micro_fix.max_per_task product limit reached"
    ) {
        crate::task_coordination::output_not_permitted(
            "micro_fix.max_per_task is exhausted; route the finding through ordinary triage or a Human",
        )
    } else {
        error.into()
    }
}

#[derive(Debug, Clone)]
pub(super) struct MicroFixRequest {
    request_id: String,
    authorization_version: i64,
    review_producer_execution_id: String,
    report_digest: String,
    decision: MicroFixOutput,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewFindingsTurn {
    schema_version: u32,
    request_id: String,
    task_id: String,
    authorization_version: i64,
    review_findings: Vec<ReviewFindingsDecision>,
    #[serde(default)]
    outputs: Vec<Value>,
}

impl TaskReworkService {
    pub(super) async fn micro_fix_request(
        &self,
        task_id: &str,
        report_id: &str,
    ) -> Result<Option<MicroFixRequest>> {
        let row = sqlx::query(
            "SELECT request.id,request.authorization_version,
                    request.review_producer_execution_id,request.review_report_digest,
                    triage.disposition,execution.summary
             FROM task_review_triage triage
             JOIN task_coordination_request request
               ON request.task_id=triage.task_id AND request.kind='review_findings'
              AND request.review_report_artifact_id=triage.review_report_artifact_id
              AND request.review_report_digest=triage.report_digest
              AND request.execution_id=triage.execution_id
              AND request.authorization_version=triage.authorization_version
             JOIN execution ON execution.id=request.execution_id
             WHERE triage.task_id=? AND triage.review_report_artifact_id=?
               AND request.state='triaged'",
        )
        .bind(task_id)
        .bind(report_id)
        .fetch_optional(self.db.pool())
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        if row.try_get::<String, _>("disposition")? != "correct" {
            return Ok(None);
        }
        let authorization_version: i64 = row.try_get("authorization_version")?;
        let report_digest: String = row.try_get("review_report_digest")?;
        let request_id: String = row.try_get("id")?;
        let summary: Option<String> = row.try_get("summary")?;
        let Some(summary) = summary else {
            return Ok(None);
        };
        if summary.len() > 64 * 1024 {
            return Err(ServiceError::invalid_operation(
                "ReviewFindings turn exceeds its durable output bound",
            ));
        }
        let turn: ReviewFindingsTurn = serde_json::from_str(&summary)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        if turn.schema_version != 2
            || turn.request_id != request_id
            || turn.task_id != task_id
            || turn.authorization_version != authorization_version
            || turn.review_findings.len() != 1
        {
            return Ok(None);
        }
        drop(turn.outputs);
        let decision = turn
            .review_findings
            .into_iter()
            .next()
            .expect("one ReviewFindings decision was checked");
        if decision.authorization_version != authorization_version
            || decision.review_report_artifact_id != report_id
            || decision.report_digest != report_digest
            || decision.disposition != api_types::TaskReviewDisposition::Correct
        {
            return Ok(None);
        }
        let Some(micro_fix) = decision.micro_fix else {
            return Ok(None);
        };
        micro_fix
            .validate()
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        Ok(Some(MicroFixRequest {
            request_id,
            authorization_version,
            review_producer_execution_id: row.try_get("review_producer_execution_id")?,
            report_digest,
            decision: micro_fix,
        }))
    }

    pub(super) async fn micro_fix_review_actor(
        &self,
        task_id: &str,
        authorization_version: i64,
        next_review_role_id: Option<&str>,
        configured_review_actor: Option<&ActorRef>,
    ) -> Result<Option<ActorRef>> {
        let coordination = TaskCoordinationRepo::get_task_coordination(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task_coordination", task_id.to_owned()))?;
        let envelope: CoordinationEnvelope = serde_json::from_str(&coordination.envelope_json)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        if coordination.mode == TaskCoordinationMode::Manual
            || coordination.revoked_at.is_some()
            || coordination.team_stale
            || coordination.authorization_version != authorization_version
            || !envelope
                .micro_fix
                .as_ref()
                .is_some_and(|grant| grant.enabled)
        {
            return Err(ServiceError::Coordination {
                code: "micro_fix.authorization_stale",
                message:
                    "the micro-fix authorization changed before its correction pair was created"
                        .to_owned(),
            });
        }
        let Some(reviewer) = envelope.micro_fix_reviewer else {
            return Ok(None);
        };
        if next_review_role_id != Some(reviewer.task_role_id.as_str()) {
            return Err(ServiceError::Coordination {
                code: "micro_fix.reviewer_route_mismatch",
                message: "micro_fix_reviewer must match the configured next formal review TaskRole"
                    .to_owned(),
            });
        }
        let reviewer_actor = ActorRef::Agent(reviewer.agent_id);
        if configured_review_actor.is_some_and(|configured| configured != &reviewer_actor) {
            return Err(ServiceError::Coordination {
                code: "micro_fix.reviewer_route_mismatch",
                message: "micro_fix_reviewer must match the configured next formal review Actor"
                    .to_owned(),
            });
        }
        Ok(Some(reviewer_actor))
    }

    pub(super) async fn ensure_micro_fix_receipt(
        &self,
        task_id: &str,
        report_id: &str,
        correction_work_unit_id: &str,
    ) -> Result<()> {
        let Some(micro_fix) = self.micro_fix_request(task_id, report_id).await? else {
            return Ok(());
        };
        let receipts =
            MicroFixReceiptRepo::list_micro_fix_receipts_by_task(&*self.db, task_id).await?;
        if receipts.iter().any(|receipt| {
            receipt.request_id == micro_fix.request_id
                && receipt.fix_work_unit_id == correction_work_unit_id
        }) {
            return Ok(());
        }

        let coordination = TaskCoordinationRepo::get_task_coordination(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task_coordination", task_id.to_owned()))?;
        let envelope: CoordinationEnvelope = serde_json::from_str(&coordination.envelope_json)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let Some(grant) = envelope.micro_fix.as_ref().filter(|grant| grant.enabled) else {
            return Err(ServiceError::Coordination {
                code: "micro_fix.authorization_stale",
                message: "the micro-fix grant is no longer active".to_owned(),
            });
        };
        if coordination.mode == TaskCoordinationMode::Manual
            || coordination.revoked_at.is_some()
            || coordination.team_stale
            || coordination.authorization_version != micro_fix.authorization_version
        {
            return Err(ServiceError::Coordination {
                code: "micro_fix.authorization_stale",
                message: "the micro-fix authorization changed before its receipt was written"
                    .to_owned(),
            });
        }
        if receipts.len() >= grant.max_per_task as usize {
            return Err(crate::task_coordination::output_not_permitted(
                "micro_fix.max_per_task is exhausted; route the finding through ordinary triage or a Human",
            ));
        }

        let source = sqlx::query(
            "SELECT subject.result_commit_sha,reviewer_role.id AS review_role_id,
                    review_unit.assigned_actor_kind,review_unit.assigned_actor_id
             FROM task_rework_receipt rework
             JOIN task_coordination_request request
               ON request.id=? AND request.task_id=rework.task_id
              AND request.review_report_artifact_id=rework.review_report_artifact_id
              AND request.review_report_digest=? AND request.execution_id IS NOT NULL
              AND request.review_producer_execution_id=?
              AND request.execution_id=(SELECT triage.execution_id FROM task_review_triage triage
                 WHERE triage.task_id=request.task_id
                   AND triage.review_report_artifact_id=request.review_report_artifact_id)
              AND request.state='triaged'
             JOIN artifact_execution_producer producer
               ON producer.execution_id=request.review_producer_execution_id
              AND producer.artifact_id=request.review_report_artifact_id
             JOIN execution review_execution
               ON review_execution.id=producer.execution_id
              AND review_execution.task_id=request.task_id
             JOIN work_unit_review_subject subject
               ON subject.review_work_unit_id=review_execution.work_unit_id
              AND subject.task_id=request.task_id
             JOIN work_unit review_unit ON review_unit.id=rework.review_work_unit_id
              AND review_unit.task_id=request.task_id
             JOIN task_role reviewer_role ON reviewer_role.task_id=request.task_id
              AND reviewer_role.role=review_unit.role
             WHERE rework.task_id=? AND rework.review_report_artifact_id=?
               AND rework.correction_work_unit_id=?
               AND request.authorization_version=?",
        )
        .bind(&micro_fix.request_id)
        .bind(&micro_fix.report_digest)
        .bind(&micro_fix.review_producer_execution_id)
        .bind(task_id)
        .bind(report_id)
        .bind(correction_work_unit_id)
        .bind(micro_fix.authorization_version)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation(
                "micro-fix receipt source no longer matches its exact report and correction pair",
            )
        })?;
        let reviewed_commit_sha: Option<String> = source.try_get("result_commit_sha")?;
        let reviewed_commit_sha = reviewed_commit_sha.ok_or_else(|| {
            ServiceError::invalid_operation(
                "micro-fix requires an exact reviewed repository commit",
            )
        })?;

        let reviewer_cost_class = envelope
            .micro_fix_reviewer
            .as_ref()
            .map(|value| value.cost_class);
        let reviewer_actor_id = envelope
            .micro_fix_reviewer
            .as_ref()
            .map(|value| value.agent_id.as_str());
        if let Some(reviewer) = envelope.micro_fix_reviewer.as_ref() {
            let review_role_id: String = source.try_get("review_role_id")?;
            let actor_kind: Option<String> = source.try_get("assigned_actor_kind")?;
            let actor_id: Option<String> = source.try_get("assigned_actor_id")?;
            if review_role_id != reviewer.task_role_id
                || actor_kind.as_deref() != Some("agent")
                || actor_id.as_deref() != Some(reviewer.agent_id.as_str())
            {
                return Err(ServiceError::Coordination {
                    code: "micro_fix.reviewer_route_mismatch",
                    message: "the correction pair does not use its pinned micro_fix_reviewer"
                        .to_owned(),
                });
            }
        }

        MicroFixReceiptRepo::create_micro_fix_receipt(
            &*self.db,
            CreateMicroFixReceipt {
                id: new_uuid_v4(),
                task_id: task_id.to_owned(),
                request_id: micro_fix.request_id,
                source_kind: MicroFixSourceKind::ReviewFindings,
                finding_refs_json: serde_json::to_string(&micro_fix.decision.finding_refs)
                    .map_err(|error| ServiceError::invalid_operation(error.to_string()))?,
                declared_paths_json: serde_json::to_string(&micro_fix.decision.paths)
                    .map_err(|error| ServiceError::invalid_operation(error.to_string()))?,
                fix_work_unit_id: correction_work_unit_id.to_owned(),
                reviewed_commit_sha,
                reviewer_cost_class,
                reviewer_actor_kind: reviewer_actor_id.map(|_| ActorKind::Agent),
                reviewer_actor_id: reviewer_actor_id.map(str::to_owned),
                envelope_digest: coordination.envelope_digest,
                created_at: now_rfc3339(),
            },
        )
        .await
        .map_err(map_micro_fix_receipt_error)?;
        Ok(())
    }

    pub(crate) async fn ensure_micro_fix_source_receipt(
        &self,
        task_id: &str,
        request_id: &str,
        authorization_version: i64,
        source_kind: MicroFixSourceKind,
        correction_work_unit_id: &str,
        reviewed_commit_sha: &str,
        micro_fix: &MicroFixOutput,
    ) -> Result<()> {
        micro_fix
            .validate()
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let request = TaskCoordinationRepo::get_task_coordination_request(&*self.db, request_id)
            .await?
            .filter(|request| {
                request.task_id == task_id
                    && request.kind.to_string() == source_kind.to_string()
                    && request.authorization_version == authorization_version
                    && request.state == db::TaskCoordinationRequestState::Triaged
            })
            .ok_or_else(|| ServiceError::not_found("triaged micro-fix request", request_id))?;
        let receipts =
            MicroFixReceiptRepo::list_micro_fix_receipts_by_task(&*self.db, task_id).await?;
        if receipts.iter().any(|receipt| {
            receipt.request_id == request.id && receipt.fix_work_unit_id == correction_work_unit_id
        }) {
            return Ok(());
        }
        let coordination = TaskCoordinationRepo::get_task_coordination(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task_coordination", task_id.to_owned()))?;
        let envelope: CoordinationEnvelope = serde_json::from_str(&coordination.envelope_json)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let Some(grant) = envelope.micro_fix.as_ref().filter(|grant| grant.enabled) else {
            return Err(ServiceError::Coordination {
                code: "micro_fix.authorization_stale",
                message: "the micro-fix grant is no longer active".to_owned(),
            });
        };
        if coordination.mode == TaskCoordinationMode::Manual
            || coordination.revoked_at.is_some()
            || coordination.team_stale
            || coordination.authorization_version != authorization_version
        {
            return Err(ServiceError::Coordination {
                code: "micro_fix.authorization_stale",
                message: "the micro-fix authorization changed before its receipt was written"
                    .to_owned(),
            });
        }
        if receipts.len() >= grant.max_per_task as usize {
            return Err(crate::task_coordination::output_not_permitted(
                "micro_fix.max_per_task is exhausted; route the finding through ordinary triage or a Human",
            ));
        }
        let reviewer = envelope.micro_fix_reviewer.as_ref();
        MicroFixReceiptRepo::create_micro_fix_receipt(
            &*self.db,
            CreateMicroFixReceipt {
                id: new_uuid_v4(),
                task_id: task_id.to_owned(),
                request_id: request.id,
                source_kind,
                finding_refs_json: serde_json::to_string(&micro_fix.finding_refs)
                    .map_err(|error| ServiceError::invalid_operation(error.to_string()))?,
                declared_paths_json: serde_json::to_string(&micro_fix.paths)
                    .map_err(|error| ServiceError::invalid_operation(error.to_string()))?,
                fix_work_unit_id: correction_work_unit_id.to_owned(),
                reviewed_commit_sha: reviewed_commit_sha.to_owned(),
                reviewer_cost_class: reviewer.map(|value| value.cost_class),
                reviewer_actor_kind: reviewer.map(|_| ActorKind::Agent),
                reviewer_actor_id: reviewer.map(|value| value.agent_id.clone()),
                envelope_digest: coordination.envelope_digest,
                created_at: now_rfc3339(),
            },
        )
        .await
        .map_err(map_micro_fix_receipt_error)?;
        Ok(())
    }

    pub(crate) async fn ensure_validation_micro_fix_receipt(
        &self,
        task_id: &str,
        request_id: &str,
        authorization_version: i64,
        run_id: &str,
        micro_fix: &MicroFixOutput,
    ) -> Result<()> {
        let correction_work_unit_id: Option<String> = sqlx::query_scalar(
            "SELECT rework_ref FROM task_validation_triage
             WHERE request_id=? AND task_id=? AND validation_run_id=?
               AND disposition='correct'",
        )
        .bind(request_id)
        .bind(task_id)
        .bind(run_id)
        .fetch_optional(self.db.pool())
        .await?
        .flatten();
        let correction_work_unit_id = correction_work_unit_id.ok_or_else(|| {
            ServiceError::not_found("validation correction WorkUnit", request_id.to_owned())
        })?;
        let run = db::ValidationRunRepo::get_validation_run(&*self.db, run_id)
            .await?
            .filter(|run| run.task_id == task_id)
            .ok_or_else(|| ServiceError::not_found("validation_run", run_id.to_owned()))?;
        self.ensure_micro_fix_source_receipt(
            task_id,
            request_id,
            authorization_version,
            MicroFixSourceKind::ValidationFindings,
            &correction_work_unit_id,
            &run.commit_sha,
            micro_fix,
        )
        .await
    }

    pub(super) async fn process_micro_fix_result(&self, execution_id: &str) -> Result<bool> {
        let Some(execution) = ExecutionRepo::get_by_id(&*self.db, execution_id).await? else {
            return Ok(false);
        };
        if execution.status != db::ExecutionStatus::Completed {
            return Ok(false);
        }
        let Some(work_unit_id) = execution.work_unit_id.as_deref() else {
            return Ok(false);
        };
        let receipts =
            MicroFixReceiptRepo::list_micro_fix_receipts_by_task(&*self.db, &execution.task_id)
                .await?;
        let Some(receipt) = receipts
            .into_iter()
            .find(|receipt| receipt.fix_work_unit_id == work_unit_id)
        else {
            return Ok(false);
        };
        let result_commit_sha = execution.after_sha.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation(
                "micro-fix correction Execution has no resulting commit",
            )
        })?;
        let workspace_id = execution.workspace_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation(
                "micro-fix correction Execution has no repository Workspace",
            )
        })?;
        let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
            .await?
            .filter(|workspace| workspace.task_id == execution.task_id)
            .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
        let advanced = if receipt.bounds_outcome == MicroFixBoundsOutcome::Pending {
            let coordination =
                TaskCoordinationRepo::get_task_coordination(&*self.db, &execution.task_id)
                    .await?
                    .ok_or_else(|| {
                        ServiceError::not_found("task_coordination", execution.task_id.clone())
                    })?;
            let envelope: CoordinationEnvelope = serde_json::from_str(&coordination.envelope_json)
                .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            let grant = envelope
                .micro_fix
                .as_ref()
                .filter(|grant| grant.enabled)
                .ok_or_else(|| ServiceError::Coordination {
                    code: "micro_fix.authorization_stale",
                    message: "the micro-fix grant is no longer active".to_owned(),
                })?;
            let _source_request =
                TaskCoordinationRepo::get_task_coordination_request(&*self.db, &receipt.request_id)
                    .await?
                    .filter(|request| {
                        request.task_id == receipt.task_id
                            && request.state == db::TaskCoordinationRequestState::Triaged
                            && request.kind.to_string() == receipt.source_kind.to_string()
                            && request.authorization_version == coordination.authorization_version
                    })
                    .ok_or_else(|| {
                        ServiceError::Coordination {
                code: "micro_fix.authorization_stale",
                message: "the micro-fix source request is no longer bound to current authorization"
                    .to_owned(),
            }
                    })?;
            if coordination.mode == TaskCoordinationMode::Manual
                || coordination.revoked_at.is_some()
                || coordination.team_stale
                || coordination.envelope_digest != receipt.envelope_digest
            {
                return Err(ServiceError::Coordination {
                    code: "micro_fix.authorization_stale",
                    message: "the micro-fix authorization changed before its bounds check"
                        .to_owned(),
                });
            }
            let declared_paths: Vec<String> = serde_json::from_str(&receipt.declared_paths_json)
                .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            let diff = git_numstat(
                &workspace.worktree_path,
                &receipt.reviewed_commit_sha,
                result_commit_sha,
            )
            .await?;
            let changed_files = i64::try_from(diff.len()).unwrap_or(i64::MAX);
            let changed_lines = diff.iter().fold(0_i64, |total, entry| {
                total
                    .saturating_add(entry.added)
                    .saturating_add(entry.deleted)
            });
            let violation = classify_bounds_violation(
                &diff,
                &declared_paths,
                &grant.extra_forbidden_globs,
                changed_files,
                changed_lines,
                grant.max_files,
                grant.max_changed_lines,
            );
            let advanced = MicroFixReceiptRepo::advance_micro_fix_receipt_bounds(
                &*self.db,
                AdvanceMicroFixReceiptBounds {
                    id: receipt.id.clone(),
                    result_commit_sha: result_commit_sha.to_owned(),
                    changed_files,
                    changed_lines,
                    bounds_outcome: if violation.is_some() {
                        MicroFixBoundsOutcome::BoundsExceeded
                    } else {
                        MicroFixBoundsOutcome::WithinBounds
                    },
                    bounds_violation: violation,
                    updated_at: now_rfc3339(),
                },
            )
            .await;
            match advanced {
                Ok(receipt) => receipt,
                Err(db::DbError::VersionConflict) => return Ok(true),
                Err(error) => return Err(error.into()),
            }
        } else {
            receipt
        };
        if let Some(bounds_violation) = advanced.bounds_violation {
            let dedupe_key = format!("micro_fix.bounds_exceeded:{}", advanced.id);
            if DomainEventRepo::get_event_by_dedupe(&*self.db, &dedupe_key)
                .await?
                .is_none()
            {
                let event = DomainEventRepo::append_event(
                    &*self.db,
                    CreateDomainEvent {
                        id: new_uuid_v4(),
                        event_type: "micro_fix.bounds_exceeded".to_owned(),
                        entity_type: "micro_fix_receipt".to_owned(),
                        entity_id: advanced.id.clone(),
                        actor_type: "system".to_owned(),
                        actor_id: None,
                        scope_type: "task".to_owned(),
                        scope_id: advanced.task_id.clone(),
                        correlation_id: format!("micro-fix:{}", advanced.id),
                        causation_id: Some(execution.id.clone()),
                        causation_depth: 1,
                        dedupe_key: Some(dedupe_key),
                        payload_json: serde_json::json!({
                            "code": "micro_fix.bounds_exceeded",
                            "bounds_violation": bounds_violation,
                            "micro_fix_receipt_id": advanced.id,
                            "task_id": advanced.task_id,
                            "work_unit_id": advanced.fix_work_unit_id,
                            "result_commit_sha": advanced.result_commit_sha,
                        })
                        .to_string(),
                        created_at: now_rfc3339(),
                    },
                )
                .await?;
                DomainEventService::publish_committed_hint(&self.event_bus, &event);
            }
        }
        if matches!(
            advanced.bounds_outcome,
            MicroFixBoundsOutcome::WithinBounds | MicroFixBoundsOutcome::BoundsExceeded
        ) {
            let work_unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
                .await?
                .filter(|unit| unit.task_id == execution.task_id)
                .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
            if work_unit.status == WorkUnitStatus::Completed {
                self.ensure_micro_fix_formal_review(&advanced, &execution, &workspace)
                    .await?;
            }
        }
        Ok(true)
    }

    pub(super) async fn process_micro_fix_work_unit_result(
        &self,
        work_unit_id: &str,
    ) -> Result<bool> {
        let Some(work_unit) = WorkUnitRepo::get_by_id(&*self.db, work_unit_id).await? else {
            return Ok(false);
        };
        if work_unit.status != WorkUnitStatus::Completed {
            return Ok(false);
        }
        let receipts =
            MicroFixReceiptRepo::list_micro_fix_receipts_by_task(&*self.db, &work_unit.task_id)
                .await?;
        if !receipts
            .iter()
            .any(|receipt| receipt.fix_work_unit_id == work_unit_id)
        {
            return Ok(false);
        }
        let execution_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM execution
             WHERE task_id=? AND work_unit_id=? AND status='completed'
               AND after_sha IS NOT NULL
             ORDER BY work_unit_version DESC, created_at DESC, id DESC LIMIT 1",
        )
        .bind(&work_unit.task_id)
        .bind(work_unit_id)
        .fetch_optional(self.db.pool())
        .await?;
        let Some(execution_id) = execution_id else {
            return Ok(false);
        };
        self.process_micro_fix_result(&execution_id).await
    }

    async fn ensure_micro_fix_formal_review(
        &self,
        receipt: &MicroFixReceipt,
        execution: &db::Execution,
        workspace: &db::Workspace,
    ) -> Result<()> {
        if execution.work_unit_id.as_deref() != Some(receipt.fix_work_unit_id.as_str())
            || execution.task_id != receipt.task_id
            || execution.after_sha.as_deref() != receipt.result_commit_sha.as_deref()
            || execution.workspace_id.as_deref() != Some(workspace.id.as_str())
        {
            return Err(ServiceError::invalid_operation(
                "micro-fix formal review must use its exact bounded result Execution",
            ));
        }
        let unit = WorkUnitRepo::get_by_id(&*self.db, &receipt.fix_work_unit_id)
            .await?
            .filter(|unit| unit.task_id == receipt.task_id)
            .ok_or_else(|| {
                ServiceError::not_found("work_unit", receipt.fix_work_unit_id.clone())
            })?;
        if unit.status != WorkUnitStatus::Completed
            || execution
                .work_unit_version
                .is_none_or(|version| unit.version != version + 1)
        {
            return Err(ServiceError::invalid_operation(
                "micro-fix formal review waits for the exact completed WorkUnit result",
            ));
        }

        let coordination = TaskCoordinationRepo::get_task_coordination(&*self.db, &receipt.task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task_coordination", receipt.task_id.clone()))?;
        let envelope: CoordinationEnvelope = serde_json::from_str(&coordination.envelope_json)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let _source_request =
            TaskCoordinationRepo::get_task_coordination_request(&*self.db, &receipt.request_id)
                .await?
                .filter(|request| {
                    request.task_id == receipt.task_id
                        && request.state == db::TaskCoordinationRequestState::Triaged
                        && request.kind.to_string() == receipt.source_kind.to_string()
                        && request.authorization_version == coordination.authorization_version
                })
                .ok_or_else(|| ServiceError::Coordination {
                    code: "micro_fix.authorization_stale",
                    message:
                        "the micro-fix source request is no longer bound to current authorization"
                            .to_owned(),
                })?;
        if coordination.mode == TaskCoordinationMode::Manual
            || coordination.revoked_at.is_some()
            || coordination.team_stale
            || coordination.envelope_digest != receipt.envelope_digest
            || !envelope
                .micro_fix
                .as_ref()
                .is_some_and(|grant| grant.enabled)
        {
            return Err(ServiceError::Coordination {
                code: "micro_fix.authorization_stale",
                message: "the micro-fix authorization changed before its formal review".to_owned(),
            });
        }
        let current_head = git::get_current_sha(Path::new(&workspace.worktree_path)).await?;
        if current_head != receipt.result_commit_sha.as_deref().unwrap_or_default() {
            return Err(ServiceError::invalid_operation(
                "micro-fix review Workspace no longer matches its bounded result commit",
            ));
        }
        let snapshot_digest =
            crate::ValidationService::snapshot_digest(&workspace.worktree_path).await?;

        let existing_rework_review: Option<String> =
            if receipt.source_kind == MicroFixSourceKind::ReviewFindings {
                sqlx::query_scalar(
                    "SELECT review_work_unit_id FROM task_rework_receipt
                 WHERE task_id=? AND correction_work_unit_id=?",
                )
                .bind(&receipt.task_id)
                .bind(&receipt.fix_work_unit_id)
                .fetch_optional(self.db.pool())
                .await?
            } else {
                None
            };
        let review_work_unit_id = if let Some(review_work_unit_id) = existing_rework_review {
            let subject_matches: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM work_unit_review_subject subject
                    WHERE subject.review_work_unit_id=? AND subject.task_id=?
                      AND subject.reviewed_work_unit_id=?
                      AND subject.reviewed_work_unit_version=?
                      AND subject.result_execution_id=?
                      AND subject.result_commit_sha=?
                      AND subject.result_workspace_id=?
                      AND subject.workspace_snapshot_digest=?
                )",
            )
            .bind(&review_work_unit_id)
            .bind(&receipt.task_id)
            .bind(&receipt.fix_work_unit_id)
            .bind(execution.work_unit_version)
            .bind(&execution.id)
            .bind(receipt.result_commit_sha.as_deref())
            .bind(&workspace.id)
            .bind(&snapshot_digest)
            .fetch_one(self.db.pool())
            .await?;
            if !subject_matches {
                return Err(ServiceError::invalid_operation(
                    "the existing micro-fix review unit does not bind its exact bounded result",
                ));
            }
            review_work_unit_id
        } else {
            self.ensure_micro_fix_review_work_unit(
                receipt,
                &execution.id,
                execution
                    .work_unit_version
                    .expect("completed WorkUnit Execution is versioned"),
                receipt
                    .result_commit_sha
                    .as_deref()
                    .expect("bounds receipt has result commit"),
                &workspace.id,
                &snapshot_digest,
                &coordination,
                &envelope,
            )
            .await?
        };

        let review = WorkUnitRepo::get_by_id(&*self.db, &review_work_unit_id)
            .await?
            .filter(|review| review.task_id == receipt.task_id)
            .ok_or_else(|| {
                ServiceError::not_found("micro-fix review WorkUnit", review_work_unit_id.clone())
            })?;
        let review_role =
            TaskRoleRepo::get_by_task_and_role(&*self.db, &receipt.task_id, &review.role)
                .await?
                .ok_or_else(|| ServiceError::not_found("TaskRole", review.role.clone()))?;
        if !envelope.allowed_review_roles.contains(&review.role)
            || !crate::role_capabilities::current_task_role_has_capability(
                &self.db,
                &review_role.id,
                api_types::TaskRoleCapability::FormalReview,
            )
            .await?
        {
            return Err(ServiceError::Coordination {
                code: "micro_fix.reviewer_route_mismatch",
                message: "micro-fix review must use an allowed current formal-review TaskRole"
                    .to_owned(),
            });
        }
        let reviewer =
            review
                .assigned_actor
                .as_ref()
                .ok_or_else(|| ServiceError::Coordination {
                    code: "micro_fix.reviewer_unavailable",
                    message: "micro-fix requires an assigned formal reviewer".to_owned(),
                })?;
        let reviewer_api = match reviewer {
            db::ActorRef::Human(id) => ActorRef::Human(id.clone()),
            db::ActorRef::Agent(id) => ActorRef::Agent(id.clone()),
        };
        if super::actor_contributed_to_work_unit(
            &self.db,
            &receipt.task_id,
            &receipt.fix_work_unit_id,
            &reviewer_api,
        )
        .await?
        {
            return Err(ServiceError::Coordination {
                code: "micro_fix.reviewer_not_independent",
                message: "micro-fix formal reviewer contributed to its correction result"
                    .to_owned(),
            });
        }
        if !is_current_review_member(&self.db, &receipt.task_id, &review_role.id, reviewer).await? {
            return Err(ServiceError::Coordination {
                code: "micro_fix.reviewer_route_mismatch",
                message: "micro-fix reviewer is no longer an active eligible TaskRole member"
                    .to_owned(),
            });
        }
        if let Some(pin) = envelope.micro_fix_reviewer.as_ref() {
            if review_role.id != pin.task_role_id
                || reviewer != &db::ActorRef::Agent(pin.agent_id.clone())
            {
                return Err(ServiceError::Coordination {
                    code: "micro_fix.reviewer_route_mismatch",
                    message:
                        "the formal review must use the exact preauthorized micro_fix_reviewer"
                            .to_owned(),
                });
            }
        }
        let subject = sqlx::query_scalar::<_, String>(
            "SELECT line.id FROM work_unit_review_line line
             WHERE line.task_id=? AND line.current_work_unit_id=?
               AND line.current_work_unit_version=? AND line.current_result_execution_id=?
               AND line.current_commit_sha=? AND line.current_workspace_id=?
               AND line.current_snapshot_digest=?
             ORDER BY line.created_at,line.id LIMIT 1",
        )
        .bind(&receipt.task_id)
        .bind(&receipt.fix_work_unit_id)
        .bind(execution.work_unit_version)
        .bind(&execution.id)
        .bind(receipt.result_commit_sha.as_deref())
        .bind(&workspace.id)
        .bind(&snapshot_digest)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation(
                "micro-fix formal review has no exact durable review line",
            )
        })?;
        self.ensure_micro_fix_review_gate(&receipt.task_id, &receipt.fix_work_unit_id, &subject)
            .await
    }

    async fn ensure_micro_fix_review_work_unit(
        &self,
        receipt: &MicroFixReceipt,
        result_execution_id: &str,
        result_work_unit_version: i64,
        result_commit_sha: &str,
        result_workspace_id: &str,
        snapshot_digest: &str,
        coordination: &TaskCoordination,
        envelope: &CoordinationEnvelope,
    ) -> Result<String> {
        let id = format!("micro-fix-review:{}", receipt.id);
        if let Some(existing) = WorkUnitRepo::get_by_id(&*self.db, &id).await? {
            if existing.task_id != receipt.task_id
                || existing.provenance.as_ref()
                    != Some(&db::WorkUnitProvenance::WorkUnit(
                        receipt.fix_work_unit_id.clone(),
                    ))
            {
                return Err(ServiceError::conflict(
                    "micro-fix review WorkUnit identity is already bound to another result",
                ));
            }
            return Ok(id);
        }
        let (role_id, role_name, actor, cost_class) =
            self.select_micro_fix_reviewer(receipt, envelope).await?;
        let created_by = coordination
            .requested_by
            .as_ref()
            .map(|id| db::ActorRef::Human(id.clone()))
            .ok_or_else(|| ServiceError::Coordination {
                code: "micro_fix.authorization_stale",
                message: "the micro-fix authorization has no Human authorizer".to_owned(),
            })?;
        let scope = format!(
            "Formally review the bounded micro-fix result for WorkUnit {}. Compare the exact result commit {} with reviewed commit {}. Review only these coordinator findings: {}. Declared paths: {}.",
            receipt.fix_work_unit_id,
            result_commit_sha,
            receipt.reviewed_commit_sha,
            receipt.finding_refs_json,
            receipt.declared_paths_json,
        );
        let event = CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "work_unit.created".to_owned(),
            entity_type: "work_unit".to_owned(),
            entity_id: id.clone(),
            actor_type: created_by.kind().to_string(),
            actor_id: Some(created_by.id().to_owned()),
            scope_type: "task".to_owned(),
            scope_id: receipt.task_id.clone(),
            correlation_id: format!("micro-fix:{}", receipt.id),
            causation_id: Some(result_execution_id.to_owned()),
            causation_depth: 1,
            dedupe_key: Some(format!("micro_fix.review_work_unit:{}", receipt.id)),
            payload_json: serde_json::json!({
                "task_id": receipt.task_id,
                "work_unit_id": id,
                "version": 1,
                "status": "open",
                "micro_fix_receipt_id": receipt.id,
                "reviewed_work_unit_id": receipt.fix_work_unit_id,
                "reviewed_execution_id": result_execution_id,
                "reviewed_commit_sha": result_commit_sha,
                "reviewer_task_role_id": role_id,
                "reviewer_actor_kind": actor.kind().to_string(),
                "reviewer_actor_id": actor.id(),
                "cost_class": cost_class.map(|value| value.to_string()),
            })
            .to_string(),
            created_at: now_rfc3339(),
        };
        let write = WorkUnitRepo::create_with_review_subject(
            &*self.db,
            db::CreateWorkUnit {
                id: id.clone(),
                task_id: receipt.task_id.clone(),
                parent_work_unit_id: None,
                title: "Formally review bounded micro-fix".to_owned(),
                scope,
                execution_instructions: Some(
                    "Use an isolated formal-review session. Inspect the exact delta and findings listed in the scope. Return a normal ReviewReport for the exact frozen result; do not edit the correction.".to_owned(),
                ),
                role: role_name,
                assigned_actor: Some(actor),
                requires_integration: false,
                provenance: Some(db::WorkUnitProvenance::WorkUnit(
                    receipt.fix_work_unit_id.clone(),
                )),
                artifact_input_ids: Vec::new(),
                review_approval_gate_id: None,
                review_approval_line_id: None,
                created_by,
                created_at: now_rfc3339(),
            },
            db::CreateWorkUnitReviewSubject {
                review_work_unit_id: id.clone(),
                task_id: receipt.task_id.clone(),
                reviewed_work_unit_id: receipt.fix_work_unit_id.clone(),
                reviewed_work_unit_version: Some(result_work_unit_version),
                result_execution_id: Some(result_execution_id.to_owned()),
                result_artifact_id: None,
                result_commit_sha: Some(result_commit_sha.to_owned()),
                result_workspace_id: Some(result_workspace_id.to_owned()),
                workspace_snapshot_digest: Some(snapshot_digest.to_owned()),
                manifest_json: None,
                manifest_digest: None,
                created_at: now_rfc3339(),
            },
            event,
        )
        .await?;
        DomainEventService::publish_committed_hint(&self.event_bus, &write.event);
        Ok(id)
    }

    async fn select_micro_fix_reviewer(
        &self,
        receipt: &MicroFixReceipt,
        envelope: &CoordinationEnvelope,
    ) -> Result<(String, String, db::ActorRef, Option<db::CostClass>)> {
        let fix = WorkUnitRepo::get_by_id(&*self.db, &receipt.fix_work_unit_id)
            .await?
            .filter(|unit| unit.task_id == receipt.task_id)
            .ok_or_else(|| {
                ServiceError::not_found("work_unit", receipt.fix_work_unit_id.clone())
            })?;
        let fix_role = TaskRoleRepo::get_by_task_and_role(&*self.db, &receipt.task_id, &fix.role)
            .await?
            .ok_or_else(|| ServiceError::not_found("TaskRole", fix.role.clone()))?;
        let fix_configuration =
            crate::role_capabilities::current_task_role_configuration(&self.db, &fix_role.id)
                .await?;
        let mut role_ids = Vec::new();
        let pinned = envelope.micro_fix_reviewer.as_ref();
        if let Some(pin) = pinned {
            role_ids.push(pin.task_role_id.clone());
        } else {
            if let Some(next_role_id) = fix_configuration
                .as_ref()
                .and_then(|configuration| configuration.rework.as_ref())
                .and_then(|rework| rework.next_review_role_id.as_ref())
            {
                role_ids.push(next_role_id.clone());
            }
            for role_name in &envelope.allowed_review_roles {
                if let Some(role) =
                    TaskRoleRepo::get_by_task_and_role(&*self.db, &receipt.task_id, role_name)
                        .await?
                {
                    if !role_ids.contains(&role.id) {
                        role_ids.push(role.id);
                    }
                }
            }
        }
        for role_id in role_ids {
            let Some(role) = TaskRoleRepo::get_by_id(&*self.db, &role_id)
                .await?
                .filter(|role| role.task_id == receipt.task_id)
            else {
                continue;
            };
            if !envelope.allowed_review_roles.contains(&role.role)
                || !crate::role_capabilities::current_task_role_has_capability(
                    &self.db,
                    &role.id,
                    api_types::TaskRoleCapability::FormalReview,
                )
                .await?
            {
                continue;
            }
            let configured_actor = if let Some(pin) = pinned {
                Some(db::ActorRef::Agent(pin.agent_id.clone()))
            } else {
                fix_configuration
                    .as_ref()
                    .and_then(|configuration| configuration.rework.as_ref())
                    .filter(|rework| {
                        rework.next_review_role_id.as_deref() == Some(role.id.as_str())
                    })
                    .and_then(|rework| rework.review_actor.as_ref())
                    .map(|actor| match actor {
                        ActorRef::Human(id) => db::ActorRef::Human(id.clone()),
                        ActorRef::Agent(id) => db::ActorRef::Agent(id.clone()),
                    })
            };
            let actors = if let Some(actor) = configured_actor.as_ref() {
                vec![actor.clone()]
            } else {
                sqlx::query(
                    "SELECT actor_kind,actor_id FROM role_membership
                     WHERE task_role_id=? AND status='active'
                     ORDER BY actor_kind,actor_id",
                )
                .bind(&role.id)
                .fetch_all(self.db.pool())
                .await?
                .into_iter()
                .map(|row| {
                    let kind: String = row.try_get("actor_kind")?;
                    let id: String = row.try_get("actor_id")?;
                    Ok(match kind.as_str() {
                        "agent" => db::ActorRef::Agent(id),
                        "human" => db::ActorRef::Human(id),
                        _ => {
                            return Err(ServiceError::invalid_operation(
                                "unknown TaskRole Actor kind",
                            ))
                        }
                    })
                })
                .collect::<Result<Vec<_>>>()?
            };
            for actor in actors {
                let active =
                    is_current_review_member(&self.db, &receipt.task_id, &role.id, &actor).await?;
                let actor_api = match &actor {
                    db::ActorRef::Human(id) => ActorRef::Human(id.clone()),
                    db::ActorRef::Agent(id) => ActorRef::Agent(id.clone()),
                };
                if !active
                    || super::actor_contributed_to_work_unit(
                        &self.db,
                        &receipt.task_id,
                        &fix.id,
                        &actor_api,
                    )
                    .await?
                {
                    continue;
                }
                let cost_class = pinned.map(|pin| pin.cost_class);
                return Ok((role.id, role.role, actor, cost_class));
            }
        }
        Err(ServiceError::Coordination {
            code: if pinned.is_some() {
                "micro_fix.reviewer_route_mismatch"
            } else {
                "micro_fix.reviewer_unavailable"
            },
            message: "no independent active formal reviewer is available in an allowed review role"
                .to_owned(),
        })
    }

    async fn ensure_micro_fix_review_gate(
        &self,
        task_id: &str,
        work_unit_id: &str,
        review_line_id: &str,
    ) -> Result<()> {
        let existing = GateRepo::list_active_gate_policies(&*self.db, task_id)
            .await?
            .into_iter()
            .find(|(gate, _)| {
                gate.gate_kind == "micro_fix_review"
                    && gate.scope_kind == db::GateScopeKind::WorkUnit
                    && gate.scope_id == work_unit_id
            });
        if let Some((_gate, policy)) = existing {
            let document: crate::gate_engine::GatePolicyDocument =
                serde_json::from_str(&policy.policy_json)
                    .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            if document
                .review
                .as_ref()
                .and_then(|review| review.review_line_id.as_deref())
                != Some(review_line_id)
            {
                return Err(ServiceError::conflict(
                    "micro-fix review Gate is bound to a different review line",
                ));
            }
            return Ok(());
        }
        let policy = crate::gate_engine::GatePolicyDocument {
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
                review_line_id: Some(review_line_id.to_owned()),
                candidates: Vec::new(),
            }),
            validations: Vec::new(),
            decisions: Vec::new(),
            work_units: Vec::new(),
        };
        crate::gate_engine::GateEngine::new(
            std::sync::Arc::clone(&self.db),
            std::sync::Arc::clone(&self.event_bus),
        )
        .create_gate_with_initial_policy(
            task_id,
            "micro_fix_review",
            db::GateScopeKind::WorkUnit,
            work_unit_id,
            policy,
        )
        .await?;
        Ok(())
    }
}

async fn is_current_review_member(
    db: &db::SqliteDb,
    task_id: &str,
    task_role_id: &str,
    actor: &db::ActorRef,
) -> Result<bool> {
    let kind = actor.kind().to_string();
    let eligible: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM role_membership membership
            JOIN task_role role ON role.id=membership.task_role_id
            JOIN task ON task.id=role.task_id AND task.deleted_at IS NULL
            WHERE membership.task_role_id=? AND role.task_id=?
              AND membership.status='active' AND membership.actor_kind=?
              AND membership.actor_id=?
        ) AND (? != 'agent' OR EXISTS(
            SELECT 1 FROM agent_current agent
            JOIN task ON task.id=? AND task.deleted_at IS NULL
            JOIN project ON project.id=task.project_id
            JOIN project_member owner_member
              ON owner_member.project_id=project.id
             AND owner_member.user_id=project.owner_id
             AND owner_member.role IN ('owner','admin','member')
            WHERE agent.id=? AND agent.owner_id=project.owner_id
        ))",
    )
    .bind(task_role_id)
    .bind(task_id)
    .bind(&kind)
    .bind(actor.id())
    .bind(&kind)
    .bind(task_id)
    .bind(actor.id())
    .fetch_one(db.pool())
    .await?;
    Ok(eligible)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NumstatEntry {
    path: String,
    added: i64,
    deleted: i64,
}

async fn git_numstat(worktree_path: &str, base: &str, head: &str) -> Result<Vec<NumstatEntry>> {
    let output = Command::new("git")
        .args([
            "diff",
            "--no-ext-diff",
            "--no-renames",
            "--numstat",
            "-z",
            base,
            head,
            "--",
        ])
        .current_dir(Path::new(worktree_path))
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("failed to run micro-fix diff: {error}"))
        })?;
    if !output.status.success() {
        return Err(ServiceError::invalid_operation(format!(
            "micro-fix diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let output = String::from_utf8(output.stdout)
        .map_err(|_| ServiceError::invalid_operation("micro-fix diff contains a non-UTF-8 path"))?;
    let mut entries = Vec::new();
    for record in output.split('\0').filter(|record| !record.is_empty()) {
        let mut fields = record.splitn(3, '\t');
        let added = fields.next().unwrap_or_default();
        let deleted = fields.next().unwrap_or_default();
        let path = fields.next().unwrap_or_default();
        if path.is_empty() {
            return Err(ServiceError::invalid_operation(
                "micro-fix diff returned a malformed numstat record",
            ));
        }
        entries.push(NumstatEntry {
            path: path.to_owned(),
            added: added.parse().unwrap_or(0),
            deleted: deleted.parse().unwrap_or(0),
        });
    }
    Ok(entries)
}

fn classify_bounds_violation(
    entries: &[NumstatEntry],
    declared_paths: &[String],
    extra_forbidden_globs: &[String],
    changed_files: i64,
    changed_lines: i64,
    max_files: u32,
    max_changed_lines: u32,
) -> Option<MicroFixBoundsViolation> {
    if entries.iter().any(|entry| {
        !declared_paths
            .iter()
            .any(|declared| path_is_within(&entry.path, declared))
    }) {
        return Some(MicroFixBoundsViolation::PathsOutsideDeclared);
    }
    if entries
        .iter()
        .any(|entry| is_dependency_manifest(&entry.path))
    {
        return Some(MicroFixBoundsViolation::NewDependency);
    }
    if entries.iter().any(|entry| {
        is_migration(&entry.path)
            || is_always_forbidden_control_path(&entry.path)
            || extra_forbidden_globs
                .iter()
                .any(|pattern| glob_matches_path(pattern, &entry.path))
    }) {
        return Some(MicroFixBoundsViolation::ForbiddenGlob);
    }
    if changed_files > i64::from(max_files) || changed_files > i64::from(db::MAX_MICRO_FIX_FILES) {
        return Some(MicroFixBoundsViolation::TooManyFiles);
    }
    if changed_lines > i64::from(max_changed_lines)
        || changed_lines > i64::from(db::MAX_MICRO_FIX_CHANGED_LINES)
    {
        return Some(MicroFixBoundsViolation::TooManyLines);
    }
    None
}

fn is_always_forbidden_control_path(path: &str) -> bool {
    path == "CLAUDE.md" || path.starts_with(".github/")
}

fn path_is_within(path: &str, declared: &str) -> bool {
    path == declared
        || path
            .strip_prefix(declared)
            .is_some_and(|tail| tail.starts_with('/'))
}

fn is_migration(path: &str) -> bool {
    path.split('/').any(|part| part == "migrations")
        || Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.len() >= 10
                    && name.starts_with('V')
                    && name.as_bytes()[1..4].iter().all(u8::is_ascii_digit)
                    && name[4..].starts_with("__")
                    && name.ends_with(".sql")
            })
}

fn is_dependency_manifest(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "cargo.toml"
            | "cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "npm-shrinkwrap.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "go.mod"
            | "go.sum"
            | "pyproject.toml"
            | "requirements.txt"
            | "poetry.lock"
            | "pipfile"
            | "pipfile.lock"
            | "composer.json"
            | "composer.lock"
            | "gemfile"
            | "gemfile.lock"
            | "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "gradle.lockfile"
    ) || name.starts_with("requirements-") && name.ends_with(".txt")
}

fn glob_matches_path(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    wildcard_match(pattern.as_bytes(), path.as_bytes())
        || (!pattern.contains('/')
            && Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| wildcard_match(pattern.as_bytes(), name.as_bytes())))
}

fn wildcard_match(pattern: &[u8], value: &[u8]) -> bool {
    let mut current = vec![false; value.len() + 1];
    current[0] = true;
    for byte in pattern {
        let mut next = vec![false; value.len() + 1];
        match byte {
            b'*' => {
                next[0] = current[0];
                for index in 1..=value.len() {
                    next[index] = current[index] || next[index - 1];
                }
            }
            b'?' => {
                for index in 1..=value.len() {
                    next[index] = current[index - 1] && value[index - 1] != b'/';
                }
            }
            literal => {
                for index in 1..=value.len() {
                    next[index] = current[index - 1] && value[index - 1] == *literal;
                }
            }
        }
        current = next;
    }
    current[value.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micro_fix_bounds_rejects_dependency_manifests_and_migrations() {
        let declared = vec!["crates/db".to_owned()];
        let manifest = vec![NumstatEntry {
            path: "crates/db/Cargo.toml".to_owned(),
            added: 1,
            deleted: 0,
        }];
        assert_eq!(
            classify_bounds_violation(&manifest, &declared, &[], 1, 1, 3, 60),
            Some(MicroFixBoundsViolation::NewDependency)
        );
        let migration = vec![NumstatEntry {
            path: "crates/db/migrations/V181__new_table.sql".to_owned(),
            added: 1,
            deleted: 0,
        }];
        assert_eq!(
            classify_bounds_violation(&migration, &declared, &[], 1, 1, 3, 60),
            Some(MicroFixBoundsViolation::ForbiddenGlob)
        );
    }

    #[test]
    fn micro_fix_bounds_enforces_declared_paths_and_frozen_limits() {
        let declared = vec!["crates/a/src".to_owned()];
        let outside = vec![NumstatEntry {
            path: "crates/a/tests/x.rs".to_owned(),
            added: 1,
            deleted: 0,
        }];
        assert_eq!(
            classify_bounds_violation(&outside, &declared, &[], 1, 1, 3, 60),
            Some(MicroFixBoundsViolation::PathsOutsideDeclared)
        );
        assert_eq!(
            classify_bounds_violation(&[], &declared, &[], 4, 0, 3, 60),
            Some(MicroFixBoundsViolation::TooManyFiles)
        );
        assert_eq!(
            classify_bounds_violation(&[], &declared, &[], 1, 61, 3, 60),
            Some(MicroFixBoundsViolation::TooManyLines)
        );
    }

    #[test]
    fn micro_fix_extra_forbidden_globs_match_repo_paths() {
        assert!(glob_matches_path("pnpm-lock.*", "web/pnpm-lock.yaml"));
        assert!(glob_matches_path(
            "web/**/generated/*",
            "web/src/generated/types.ts"
        ));
        assert!(!glob_matches_path("*.toml", "crates/a/src/lib.rs"));
    }

    #[test]
    fn micro_fix_bounds_always_forbid_agent_and_ci_control_files() {
        let declared = vec![".".to_owned()];
        for path in ["CLAUDE.md", ".github/workflows/build.yml"] {
            let changed = vec![NumstatEntry {
                path: path.to_owned(),
                added: 1,
                deleted: 0,
            }];
            assert_eq!(
                classify_bounds_violation(&changed, &declared, &[], 1, 1, 3, 60),
                Some(MicroFixBoundsViolation::ForbiddenGlob),
                "{path} must stay forbidden for every micro-fix"
            );
        }
    }
}
