use super::*;
use crate::coordinator_outputs::{self, CoordinatorOutput, CoordinatorOutputKind};
use crate::human_decision_service::{HumanDecisionService, RaiseHumanDecisionInput, ResumeBinding};
use db::{
    AddWorkUnitDependency, ApplyReviewCorrections, ApplyReviewTriage, CreateTaskReviewTriage,
    CreateWorkUnit, HumanDecisionRequest, HumanDecisionSourceKind, HumanDecisionState,
    ReviewCorrectionLink, ReviewFindingDisposition, ReviewFindingTriage, ReviewLedgerRepo,
    ReviewRangeReportRepo, ReviewReworkRound, TaskCoordinationRequestKind,
    TaskCoordinationRequestState, UpdateTaskCoordinationRequestState,
    WorkUnitRepositoryIntegrationOutcome,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RangeReviewDecisionV3 {
    schema_version: u32,
    request_id: String,
    task_id: String,
    authorization_version: i64,
    range_review: RangeReviewTriageV3,
    #[serde(default)]
    outputs: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RangeReviewTriageV3 {
    review_report_artifact_id: String,
    report_digest: String,
    findings: Vec<RangeFindingDecisionV3>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RangeFindingDecisionV3 {
    finding_id: String,
    disposition: api_types::TaskReviewDisposition,
    rationale: String,
    #[serde(default)]
    instructions: String,
}

fn db_disposition(disposition: api_types::TaskReviewDisposition) -> ReviewFindingDisposition {
    match disposition {
        api_types::TaskReviewDisposition::Correct => ReviewFindingDisposition::Correct,
        api_types::TaskReviewDisposition::Recheck => ReviewFindingDisposition::Recheck,
        api_types::TaskReviewDisposition::Human => ReviewFindingDisposition::Human,
        api_types::TaskReviewDisposition::Defer => ReviewFindingDisposition::Defer,
    }
}

fn disposition_rank(disposition: api_types::TaskReviewDisposition) -> u8 {
    match disposition {
        api_types::TaskReviewDisposition::Defer => 0,
        api_types::TaskReviewDisposition::Recheck => 1,
        api_types::TaskReviewDisposition::Correct => 2,
        api_types::TaskReviewDisposition::Human => 3,
    }
}

impl TaskCoordinationService {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn complete_range_human_decision_resume(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
        decision: &HumanDecisionRequest,
        paused: &TaskCoordinationRequest,
        report_id: &str,
        report_digest: &str,
        echoed_decision: &str,
        answered_option_id: &str,
    ) -> Result<()> {
        if request.kind != TaskCoordinationRequestKind::HumanDecisionResume
            || request.resumes_request_id.as_deref() != Some(paused.id.as_str())
            || request.task_id != paused.task_id
            || paused.kind != TaskCoordinationRequestKind::ReviewFindings
            || paused.state != TaskCoordinationRequestState::Triaged
            || paused.review_report_artifact_id.as_deref() != Some(report_id)
            || paused.review_report_digest.as_deref() != Some(report_digest)
            || decision.task_id != request.task_id
            || decision.source_kind != HumanDecisionSourceKind::CoordinationRequest
            || decision.source_id != paused.id
            || decision.state != HumanDecisionState::Answered
            || decision.answered_option_id.as_deref() != Some(answered_option_id)
            || echoed_decision != answered_option_id
            || !matches!(answered_option_id, "correct" | "keep_open")
        {
            return Err(invalid_plan(
                "range Human resume must match its exact paused report and answered option",
            ));
        }
        let authorization_version = paused.authorization_version;
        if decision.authorization_version != Some(authorization_version)
            || request.authorization_version != authorization_version
        {
            return Err(output_not_permitted(
                "the Human decision and range report have different authorization versions",
            ));
        }
        let binding: ResumeBinding = serde_json::from_str(&decision.resume_binding_json)
            .map_err(|_| invalid_plan("range Human decision has an unreadable resume binding"))?;
        if binding.request_id != paused.id
            || binding.request_kind != TaskCoordinationRequestKind::ReviewFindings
            || binding.authorization_version != authorization_version
            || binding.source_subject.get("kind").and_then(Value::as_str) != Some("commit_range")
            || binding
                .source_subject
                .get("review_report_artifact_id")
                .and_then(Value::as_str)
                != Some(report_id)
            || binding
                .source_subject
                .get("report_digest")
                .and_then(Value::as_str)
                != Some(report_digest)
        {
            return Err(output_not_permitted(
                "the Human decision resume binding does not match the exact range report",
            ));
        }
        let bound_ids = |key: &str| -> Result<BTreeSet<String>> {
            binding
                .source_subject
                .get(key)
                .and_then(Value::as_array)
                .ok_or_else(|| invalid_plan("range Human decision has incomplete finding IDs"))?
                .iter()
                .map(|value| {
                    value.as_str().map(str::to_owned).ok_or_else(|| {
                        invalid_plan("range Human decision has an invalid finding ID")
                    })
                })
                .collect()
        };
        let expected_human = bound_ids("human_finding_ids")?;
        let expected_correct = bound_ids("correct_finding_ids")?;
        if expected_human.is_empty() {
            return Err(invalid_plan(
                "range Human decision has no Human-routed findings",
            ));
        }

        let exact_context = crate::task_rework::TaskReworkService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .exact_review_findings_context(paused)
        .await?;
        if exact_context
            .get("report_artifact_id")
            .and_then(Value::as_str)
            != Some(report_id)
            || exact_context.get("report_digest").and_then(Value::as_str) != Some(report_digest)
        {
            return Err(output_not_permitted(
                "the range Human decision no longer matches the exact report context",
            ));
        }
        let triage_rows = sqlx::query(
            "SELECT finding_id, disposition, instructions, rationale FROM review_finding_triage
             WHERE request_id=? AND task_id=? AND report_artifact_id=?
               AND authorization_version=? ORDER BY finding_id",
        )
        .bind(&paused.id)
        .bind(&paused.task_id)
        .bind(report_id)
        .bind(authorization_version)
        .fetch_all(self.db.pool())
        .await?;
        let mut actual_human = BTreeSet::new();
        let mut actual_correct = BTreeSet::new();
        let mut all_instructions = BTreeMap::new();
        for row in triage_rows {
            let finding_id: String = row.try_get("finding_id")?;
            let disposition: String = row.try_get("disposition")?;
            let instructions: String = row.try_get("instructions")?;
            let rationale: String = row.try_get("rationale")?;
            match disposition.as_str() {
                "human" => {
                    actual_human.insert(finding_id.clone());
                    all_instructions.insert(
                        finding_id,
                        if instructions.trim().is_empty() {
                            rationale
                        } else {
                            instructions
                        },
                    );
                }
                "correct" => {
                    actual_correct.insert(finding_id.clone());
                    all_instructions.insert(
                        finding_id,
                        if instructions.trim().is_empty() {
                            rationale
                        } else {
                            instructions
                        },
                    );
                }
                _ => {}
            }
        }
        if actual_human != expected_human || actual_correct != expected_correct {
            return Err(output_not_permitted(
                "the Human decision finding set no longer matches persisted range triage",
            ));
        }
        let mut selected = expected_correct;
        if answered_option_id == "correct" {
            selected.extend(expected_human);
        }
        let user_id = decision.answered_by_user_id.as_deref().ok_or_else(|| {
            output_not_permitted("range Human decision has no authenticated answerer")
        })?;

        let current_request =
            TaskCoordinationRepo::get_task_coordination_request(&*self.db, &request.id)
                .await?
                .filter(|current| {
                    current.task_id == request.task_id
                        && current.kind == TaskCoordinationRequestKind::HumanDecisionResume
                        && current.resumes_request_id.as_deref() == Some(paused.id.as_str())
                        && current.source_subject_id.as_deref() == Some(decision.id.as_str())
                })
                .ok_or_else(|| invalid_plan("range HumanDecisionResume is no longer exact"))?;
        if current_request.state == TaskCoordinationRequestState::Triaged {
            if current_request.execution_id.as_deref() == Some(execution.id.as_str()) {
                return Ok(());
            }
            return Err(ServiceError::conflict(
                "range HumanDecisionResume was completed by a different Execution",
            ));
        }
        if !matches!(
            current_request.state,
            TaskCoordinationRequestState::Running | TaskCoordinationRequestState::Uncertain
        ) || current_request.execution_id.as_deref() != Some(execution.id.as_str())
        {
            return Err(invalid_plan(
                "range HumanDecisionResume is not running on this Execution",
            ));
        }

        let mut correction_work_units = Vec::new();
        if !selected.is_empty() {
            let task = TaskRepo::get_by_id(&*self.db, &request.task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", request.task_id.clone()))?;
            let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
                .await?
                .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
            let report = ReviewRangeReportRepo::get_review_range_report(&*self.db, report_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("ReviewRangeReport", report_id.to_owned())
                })?;
            let review_work_unit =
                db::WorkUnitRepo::get_by_id(&*self.db, &report.review_work_unit_id)
                    .await?
                    .filter(|unit| unit.task_id == task.id)
                    .ok_or_else(|| invalid_plan("range ReviewReport WorkUnit is missing"))?;
            let instructions = selected
                .iter()
                .map(|id| {
                    format!(
                        "{id}: {}",
                        all_instructions.get(id).map(String::as_str).unwrap_or(
                            "Create a bounded correction for this Human-routed range finding.",
                        )
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let result = crate::task_rework::TaskReworkService::new(
                Arc::clone(&self.db),
                Arc::clone(&self.event_bus),
            )
            .request_manual_range_rework_for_findings(
                crate::collaboration_service::CollaborationActorSource::Human(user_id.to_owned()),
                crate::task_rework::RequestReworkInput {
                    task_id: task.id.clone(),
                    expected_task_version: task.version,
                    expected_lifecycle_version: lifecycle.version,
                    review_report_artifact_id: report_id.to_owned(),
                    reviewed_work_unit_id: review_work_unit.id,
                    reviewed_work_unit_version: review_work_unit.version,
                    correction_title: "Human-approved range correction".to_owned(),
                    correction_scope: instructions,
                    correction_role_id: String::new(),
                    correction_actor: None,
                    review_title: String::new(),
                    review_scope: String::new(),
                    review_actor: None,
                    idempotency_key: format!("human-decision:{}", decision.id),
                    expected_source_configuration_json: None,
                },
                Some(selected),
            )
            .await?;
            correction_work_units = result
                .correction_work_units
                .into_iter()
                .map(|unit| unit.id)
                .collect();
        }

        let now = now_rfc3339();
        let actor = execution
            .actor_ref()
            .ok_or_else(|| invalid_plan("range HumanDecisionResume Execution has no Actor"))?;
        let mut event = self.event(
            "task.coordination_request_state_changed",
            "task_coordination_request",
            &request.id,
            &actor.kind().to_string(),
            Some(actor.id()),
            &request.task_id,
            json!({
                "task_id": request.task_id,
                "request_id": request.id,
                "state": "triaged",
                "human_decision_id": decision.id,
                "answered_option_id": answered_option_id,
                "review_report_artifact_id": report_id,
                "correction_work_unit_ids": correction_work_units,
            }),
            &now,
        );
        let dedupe_key = format!("range-human-decision-resume-triaged:{}", request.id);
        event.dedupe_key = Some(dedupe_key.clone());
        TaskCoordinationRepo::update_task_coordination_request(
            &*self.db,
            UpdateTaskCoordinationRequestState {
                id: request.id.clone(),
                expected_version: current_request.version,
                state: TaskCoordinationRequestState::Triaged,
                wake_id: None,
                execution_id: None,
                plan_artifact_id: None,
                failure_code: None,
                failure_message: None,
                completed_at: Some(now.clone()),
                updated_at: now,
                event: Some(event),
            },
        )
        .await?;
        self.domain_events.publish_by_dedupe(&dedupe_key).await?;
        Ok(())
    }

    pub(super) async fn complete_range_review_findings_turn(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
        allowed_outputs: Option<&[CoordinatorOutputKind]>,
    ) -> Result<()> {
        let output = execution.summary.as_deref().ok_or_else(|| {
            invalid_plan("the range ReviewFindings coordination Execution has no triage output")
        })?;
        if output.len() > 64 * 1024 {
            return Err(invalid_plan(
                "the range ReviewFindings output exceeds 64 KiB",
            ));
        }
        let response: RangeReviewDecisionV3 = serde_json::from_str(output).map_err(|_| {
            invalid_plan("range ReviewFindings requires its schema 3 typed JSON envelope")
        })?;
        if response.schema_version != 3
            || response.request_id != request.id
            || response.task_id != request.task_id
            || response.authorization_version != request.authorization_version
        {
            return Err(invalid_plan(
                "range ReviewFindings requires schema 3 for its exact request and authorization",
            ));
        }
        let report_id = request
            .review_report_artifact_id
            .as_deref()
            .ok_or_else(|| invalid_plan("ReviewFindings request has no exact report ID"))?;
        let report_digest = request
            .review_report_digest
            .as_deref()
            .ok_or_else(|| invalid_plan("ReviewFindings request has no exact report digest"))?;
        if response.range_review.review_report_artifact_id != report_id
            || response.range_review.report_digest != report_digest
            || request.review_producer_execution_id.as_deref().is_none()
            || request.execution_id.as_deref() != Some(execution.id.as_str())
        {
            return Err(invalid_plan(
                "range ReviewFindings decision does not match its exact report, producer, and request",
            ));
        }
        if response.range_review.findings.len() > 256 {
            return Err(invalid_plan(
                "range ReviewFindings accepts at most 256 findings",
            ));
        }

        let context = crate::task_rework::TaskReworkService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .exact_review_findings_context(request)
        .await?;
        if context.get("kind").and_then(Value::as_str) != Some("commit_range") {
            return Err(invalid_plan(
                "schema 3 is only valid for a commit-range report",
            ));
        }
        let required = context
            .get("findings")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_plan("range context has no findings list"))?;
        let mut expected = BTreeMap::<String, (String, i64)>::new();
        for value in required {
            let id = value
                .get("finding_id")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_plan("range context finding has no ID"))?;
            let severity = value
                .get("severity")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_plan("range context finding has no severity"))?;
            let ordinal = value
                .get("ordinal")
                .and_then(Value::as_i64)
                .ok_or_else(|| invalid_plan("range context finding has no ordinal"))?;
            expected.insert(id.to_owned(), (severity.to_owned(), ordinal));
        }
        let mut decisions = BTreeMap::<String, RangeFindingDecisionV3>::new();
        for decision in response.range_review.findings.iter().cloned() {
            let Some((severity, _)) = expected.get(&decision.finding_id) else {
                return Err(invalid_plan(
                    "range triage names a finding outside its exact context",
                ));
            };
            if decisions
                .insert(decision.finding_id.clone(), decision.clone())
                .is_some()
            {
                return Err(invalid_plan("range triage finding IDs must be unique"));
            }
            if decision.rationale.trim().is_empty() || decision.rationale.len() > 2048 {
                return Err(invalid_plan(
                    "range finding rationale must contain 1 to 2048 bytes",
                ));
            }
            if decision.instructions.len() > 8192 {
                return Err(invalid_plan("range finding instructions exceed 8192 bytes"));
            }
            if decision.disposition == api_types::TaskReviewDisposition::Correct
                && decision.instructions.trim().is_empty()
            {
                return Err(invalid_plan(
                    "correct range findings require actionable instructions",
                ));
            }
            if matches!(severity.as_str(), "P0" | "P1")
                && decision.disposition == api_types::TaskReviewDisposition::Defer
            {
                return Err(invalid_plan("P0/P1 range findings cannot be deferred"));
            }
        }
        for (finding_id, (severity, _)) in &expected {
            if matches!(severity.as_str(), "P0" | "P1") && !decisions.contains_key(finding_id) {
                return Err(invalid_plan(
                    "every P0/P1 range finding requires one disposition",
                ));
            }
        }

        let coordination = self.require_coordination(&request.task_id).await?;
        let envelope = self.envelope(&coordination)?;
        let outputs =
            coordinator_outputs::parse_outputs(response.outputs, &envelope, allowed_outputs)
                .map_err(ServiceError::from)?;
        if outputs.iter().any(|output| {
            !matches!(
                output,
                CoordinatorOutput::ProposeReplan(_) | CoordinatorOutput::RequestRangeReview(_)
            )
        }) {
            return Err(output_not_permitted(
                "range ReviewFindings supports request_range_review and propose_replan outputs only",
            ));
        }
        let ordered = expected
            .iter()
            .filter_map(|(finding_id, (_, ordinal))| {
                decisions
                    .get(finding_id)
                    .map(|decision| (*ordinal, decision.clone()))
            })
            .collect::<Vec<_>>();
        let human_finding_ids = ordered
            .iter()
            .filter(|(_, decision)| decision.disposition == api_types::TaskReviewDisposition::Human)
            .map(|(_, decision)| decision.finding_id.clone())
            .collect::<Vec<_>>();
        let correct_finding_ids = ordered
            .iter()
            .filter(|(_, decision)| {
                decision.disposition == api_types::TaskReviewDisposition::Correct
            })
            .map(|(_, decision)| decision.finding_id.clone())
            .collect::<Vec<_>>();
        let aggregate_disposition = ordered
            .iter()
            .map(|(_, decision)| decision.disposition)
            .max_by_key(|disposition| disposition_rank(*disposition))
            .unwrap_or(api_types::TaskReviewDisposition::Defer);
        let aggregate_rationale = ordered
            .iter()
            .find(|(_, decision)| decision.disposition == aggregate_disposition)
            .map(|(_, decision)| decision.rationale.clone())
            .unwrap_or_else(|| "No unresolved range findings were supplied.".to_owned());
        let triage_digest = canonical_json(&response.range_review)?;
        let decision_digest = sha256_hex(triage_digest.as_bytes());
        let report = ReviewRangeReportRepo::get_review_range_report(&*self.db, report_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("ReviewRangeReport", report_id.to_owned()))?;
        let now = now_rfc3339();
        let finding_rows = ordered
            .iter()
            .map(|(_, decision)| ReviewFindingTriage {
                finding_id: decision.finding_id.clone(),
                request_id: request.id.clone(),
                task_id: request.task_id.clone(),
                report_artifact_id: report_id.to_owned(),
                execution_id: execution.id.clone(),
                authorization_version: request.authorization_version,
                disposition: db_disposition(decision.disposition),
                rationale: decision.rationale.clone(),
                instructions: decision.instructions.clone(),
                created_at: now.clone(),
            })
            .collect::<Vec<_>>();
        let has_correct = ordered
            .iter()
            .any(|(_, decision)| decision.disposition == api_types::TaskReviewDisposition::Correct);
        let corrections = if has_correct && human_finding_ids.is_empty() {
            Some(
                self.build_range_corrections(request, execution, &report, &context, &ordered, &now)
                    .await?,
            )
        } else {
            None
        };
        let recheck = if ordered
            .iter()
            .any(|(_, decision)| decision.disposition == api_types::TaskReviewDisposition::Recheck)
        {
            Some(
                crate::review_range::ReviewScheduler::new(
                    Arc::clone(&self.db),
                    self.workspace_root.clone(),
                )
                .recheck_input(&report, &now)
                .await?,
            )
        } else {
            None
        };
        let actor = execution
            .actor_ref()
            .ok_or_else(|| invalid_plan("range ReviewFindings Execution has no Actor"))?;
        let actor_type = actor.kind().to_string();
        let actor_id = actor.id().to_owned();
        let event = self.event(
            "review.findings_triaged",
            "artifact",
            report_id,
            &actor_type,
            Some(&actor_id),
            &request.task_id,
            json!({
                "task_id": request.task_id,
                "report_artifact_id": report_id,
                "request_id": request.id,
                "correct": finding_rows.iter().filter(|row| row.disposition == ReviewFindingDisposition::Correct).count(),
                "recheck": finding_rows.iter().filter(|row| row.disposition == ReviewFindingDisposition::Recheck).count(),
                "human": finding_rows.iter().filter(|row| row.disposition == ReviewFindingDisposition::Human).count(),
                "defer": finding_rows.iter().filter(|row| row.disposition == ReviewFindingDisposition::Defer).count(),
            }),
            &now,
        );
        let events = vec![event.clone()];
        let mut published_events = events.clone();
        if let Some(recheck) = recheck.as_ref() {
            published_events.extend(recheck.events.iter().cloned());
        }
        if let Some(corrections) = corrections.as_ref() {
            published_events.extend(corrections.events.iter().cloned());
        }
        let event_dedupe_keys = published_events
            .iter()
            .map(|event| event.dedupe_key.clone().unwrap_or_else(|| event.id.clone()))
            .collect::<Vec<_>>();
        let fresh_request =
            TaskCoordinationRepo::get_task_coordination_request(&*self.db, &request.id)
                .await?
                .filter(|fresh| {
                    fresh.task_id == request.task_id
                        && fresh.state == TaskCoordinationRequestState::Running
                        && fresh.execution_id.as_deref() == Some(execution.id.as_str())
                })
                .ok_or_else(|| {
                    invalid_plan("range triage request is no longer running on this Execution")
                })?;
        let write = ApplyReviewTriage {
            aggregate: CreateTaskReviewTriage {
                review_report_artifact_id: report_id.to_owned(),
                task_id: request.task_id.clone(),
                report_digest: report_digest.to_owned(),
                execution_id: execution.id.clone(),
                authorization_version: request.authorization_version,
                disposition: db_disposition(aggregate_disposition),
                rationale: aggregate_rationale,
                instructions: String::new(),
                decision_digest,
                review_work_unit_id: Some(report.review_work_unit_id.clone()),
                created_at: now,
            },
            findings: finding_rows,
            corrections,
            recheck,
            expected_request_version: fresh_request.version,
            events,
        };
        let human_decisions =
            HumanDecisionService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let raised = if human_finding_ids.is_empty() {
            None
        } else {
            let mut question = format!(
                "Choose how to handle the Human-routed findings in ReviewReport {report_id}:"
            );
            let mut listed = 0;
            for finding_id in &human_finding_ids {
                let entry = format!(" {finding_id}");
                if question.len() + entry.len() > 900 {
                    break;
                }
                question.push_str(&entry);
                listed += 1;
            }
            if listed < human_finding_ids.len() {
                question.push_str(&format!(
                    "; {} more are included in the linked report.",
                    human_finding_ids.len() - listed
                ));
            }
            let ask = crate::coordinator_outputs::AskHumanOutput {
                question,
                options: vec![
                    crate::coordinator_outputs::HumanDecisionOption {
                        id: "correct".to_owned(),
                        label: "Create corrections".to_owned(),
                        description: "Create new correction WorkUnits for the exact Human-routed findings, together with findings already marked correct.".to_owned(),
                        consequence: "New WorkUnits start from the current integrated HEAD and remain linked to this ReviewReport.".to_owned(),
                        authority_effect: db::AuthorityEffect::Scope,
                    },
                    crate::coordinator_outputs::HumanDecisionOption {
                        id: "keep_open".to_owned(),
                        label: "Keep them open".to_owned(),
                        description: "Leave Human-routed findings open while applying any findings already marked correct.".to_owned(),
                        consequence: "Open P0/P1 findings continue to block review coverage.".to_owned(),
                        authority_effect: db::AuthorityEffect::Scope,
                    },
                ],
                recommended_option_id: None,
                recommendation_rationale: None,
                evidence_links: vec![crate::coordinator_outputs::EvidenceLink {
                    kind: crate::coordinator_outputs::EvidenceKind::ReviewReport,
                    id: report_id.to_owned(),
                    digest: Some(report_digest.to_owned()),
                }],
            };
            Some(
                human_decisions
                    .raise(RaiseHumanDecisionInput {
                        task_id: request.task_id.clone(),
                        source_kind: HumanDecisionSourceKind::CoordinationRequest,
                        source_id: request.id.clone(),
                        raised_by_execution_id: Some(execution.id.clone()),
                        authorization_version: Some(request.authorization_version),
                        ask,
                        resume_binding: ResumeBinding {
                            request_id: request.id.clone(),
                            request_kind: TaskCoordinationRequestKind::ReviewFindings,
                            authorization_version: request.authorization_version,
                            source_subject: json!({
                                "kind": "commit_range",
                                "review_report_artifact_id": report_id,
                                "report_digest": report_digest,
                                "human_finding_ids": human_finding_ids,
                                "correct_finding_ids": correct_finding_ids,
                            }),
                        },
                    })
                    .await?,
            )
        };
        let write_result = async {
            self.apply_request_range_review_outputs(request, execution, &outputs)
                .await?;
            ReviewLedgerRepo::apply_review_triage(&*self.db, write).await?;
            Ok::<(), ServiceError>(())
        }
        .await;
        if let (Err(error), Some(raised)) = (&write_result, raised.as_ref()) {
            let paused = matches!(
                error,
                ServiceError::Coordination {
                    code: "coordination.paused",
                    ..
                }
            );
            if raised.created && !paused {
                human_decisions
                    .close(
                        &raised.decision.id,
                        raised.decision.version,
                        db::HumanDecisionState::Cancelled,
                        "the range triage receipt could not be recorded",
                    )
                    .await
                    .ok();
            }
        }
        write_result?;
        for dedupe_key in event_dedupe_keys {
            self.domain_events.publish_by_dedupe(&dedupe_key).await?;
        }
        if let Some(output) = outputs.iter().find_map(|output| match output {
            CoordinatorOutput::ProposeReplan(value) => Some(value.clone()),
            _ => None,
        }) {
            self.apply_propose_replan(request, execution, output)
                .await?;
        }
        Ok(())
    }

    async fn build_range_corrections(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
        report: &db::ReviewRangeReport,
        context: &Value,
        ordered: &[(i64, RangeFindingDecisionV3)],
        now: &str,
    ) -> Result<ApplyReviewCorrections> {
        #[derive(Clone)]
        struct Group {
            repo_id: String,
            author_work_unit_id: Option<String>,
            integration_id: Option<String>,
            actor_candidates: Vec<Option<db::ActorRef>>,
            findings: Vec<(String, String, String, String)>,
        }
        let context_findings = context
            .get("findings")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_plan("range correction context has no findings"))?;
        let rows = ReviewLedgerRepo::load_review_coverage_rows(
            &*self.db,
            &request.task_id,
            &report.repo_id,
        )
        .await?;
        let integration_started_at = rows
            .integrations
            .iter()
            .map(|integration| (integration.id.as_str(), integration.started_at.as_str()))
            .collect::<BTreeMap<_, _>>();
        let mut groups = BTreeMap::<(String, Option<String>), Group>::new();
        for (_, decision) in ordered.iter().filter(|(_, decision)| {
            decision.disposition == api_types::TaskReviewDisposition::Correct
        }) {
            let finding = context_findings
                .iter()
                .find(|finding| {
                    finding.get("finding_id").and_then(Value::as_str)
                        == Some(decision.finding_id.as_str())
                })
                .ok_or_else(|| invalid_plan("corrected finding is absent from exact context"))?;
            let locations = finding
                .get("locations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let author = locations
                .iter()
                .filter_map(|location| {
                    let attribution = location.get("attribution")?;
                    let work_unit_id = attribution.get("work_unit_id")?.as_str()?;
                    let line_count = attribution.get("line_count")?.as_i64()?;
                    let integration_id = attribution
                        .get("integration_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let integration_started_at = integration_id
                        .as_deref()
                        .and_then(|id| integration_started_at.get(id))
                        .copied()
                        .unwrap_or("");
                    let actor = attribution
                        .get("actor")
                        .cloned()
                        .and_then(|actor| serde_json::from_value::<db::ActorRef>(actor).ok());
                    Some((
                        line_count,
                        integration_started_at.to_owned(),
                        work_unit_id.to_owned(),
                        actor,
                        integration_id,
                    ))
                })
                .max_by(|left, right| {
                    left.0
                        .cmp(&right.0)
                        .then_with(|| left.1.cmp(&right.1))
                        .then_with(|| left.4.cmp(&right.4))
                });
            let author_work_unit_id = author.as_ref().map(|(_, _, id, _, _)| id.clone());
            let actor = author
                .as_ref()
                .and_then(|(_, _, _, actor, _)| actor.clone());
            let integration_id = author.and_then(|(_, _, _, _, integration_id)| integration_id);
            let key = (report.repo_id.clone(), author_work_unit_id.clone());
            let group = groups.entry(key).or_insert_with(|| Group {
                repo_id: report.repo_id.clone(),
                author_work_unit_id,
                integration_id,
                actor_candidates: Vec::new(),
                findings: Vec::new(),
            });
            group.actor_candidates.push(actor);
            let title = finding
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("range finding");
            let path_lines = locations
                .iter()
                .filter_map(|location| {
                    let path = location.get("path")?.as_str()?;
                    let start = location.get("line_start").and_then(Value::as_i64);
                    let end = location.get("line_end").and_then(Value::as_i64);
                    Some(match (start, end) {
                        (Some(start), Some(end)) => format!("{path}:{start}-{end}"),
                        _ => path.to_owned(),
                    })
                })
                .collect::<Vec<_>>()
                .join(", ");
            group.findings.push((
                decision.finding_id.clone(),
                title.to_owned(),
                decision.instructions.clone(),
                path_lines,
            ));
        }
        if groups.is_empty() {
            return Err(invalid_plan(
                "correct dispositions produced no correction groups",
            ));
        }
        let base_commit = rows
            .integrations
            .iter()
            .filter(|integration| {
                integration.outcome == WorkUnitRepositoryIntegrationOutcome::Success
            })
            .max_by(|left, right| (&left.started_at, &left.id).cmp(&(&right.started_at, &right.id)))
            .and_then(|integration| integration.target_after_sha.clone())
            .ok_or_else(|| invalid_plan("range correction requires a current integrated HEAD"))?;
        let roles = db::TaskRoleRepo::list_by_task(&*self.db, &request.task_id).await?;
        let memberships =
            db::RoleMembershipRepo::list_by_task(&*self.db, &request.task_id, false).await?;
        let mut fallback_roles = Vec::new();
        for role in &roles {
            let configuration =
                crate::role_capabilities::current_task_role_configuration(&self.db, &role.id)
                    .await?
                    .unwrap_or_default();
            if db::canonical_task_role_name(&role.role).as_deref() == Some("implementer")
                && !configuration
                    .capabilities
                    .contains(&api_types::TaskRoleCapability::FormalReview)
                && !configuration
                    .capabilities
                    .contains(&api_types::TaskRoleCapability::Orchestration)
            {
                fallback_roles.push(role.clone());
            }
        }
        groups.retain(|_, group| !group.findings.is_empty());
        let mut ordered_groups = groups.into_values().collect::<Vec<_>>();
        ordered_groups.sort_by(|left, right| {
            let order = |group: &Group| {
                group
                    .integration_id
                    .as_deref()
                    .and_then(|integration_id| {
                        rows.integrations
                            .iter()
                            .find(|integration| integration.id == integration_id)
                    })
                    .map(|integration| (integration.started_at.as_str(), integration.id.as_str()))
            };
            order(left)
                .cmp(&order(right))
                .then_with(|| left.repo_id.cmp(&right.repo_id))
        });
        let creator = execution
            .actor_ref()
            .ok_or_else(|| invalid_plan("coordinator Execution has no Actor"))?;
        let creator_kind = creator.kind().to_string();
        let mut work_units = Vec::with_capacity(ordered_groups.len());
        let mut dependencies = Vec::new();
        let mut links = Vec::new();
        let mut previous_by_repo = BTreeMap::<String, String>::new();
        for group in ordered_groups {
            let (role_name, assigned_actor) =
                if let Some(author_work_unit_id) = group.author_work_unit_id.as_deref() {
                    let author = db::WorkUnitRepo::get_by_id(&*self.db, author_work_unit_id)
                        .await?
                        .filter(|unit| unit.task_id == request.task_id);
                    if let Some(author) = author {
                        let candidate_actor = match group.actor_candidates.first() {
                            Some(Some(first))
                                if group
                                    .actor_candidates
                                    .iter()
                                    .all(|candidate| candidate.as_ref() == Some(first)) =>
                            {
                                Some(first.clone())
                            }
                            _ => None,
                        };
                        let role = db::TaskRoleRepo::get_by_task_and_role(
                            &*self.db,
                            &request.task_id,
                            &author.role,
                        )
                        .await?;
                        let eligible = role.as_ref().is_some_and(|role| {
                            candidate_actor.as_ref().is_some_and(|actor| {
                                memberships.iter().any(|(membership_role, membership)| {
                                    membership_role.id == role.id
                                        && membership.status == db::RoleMembershipStatus::Active
                                        && membership.actor_ref() == *actor
                                })
                            })
                        });
                        if eligible {
                            (author.role, candidate_actor)
                        } else {
                            fallback_role(&fallback_roles)?
                        }
                    } else {
                        fallback_role(&fallback_roles)?
                    }
                } else {
                    fallback_role(&fallback_roles)?
                };
            let id = new_uuid_v4();
            let finding_titles = group
                .findings
                .iter()
                .map(|(id, title, _, _)| format!("{id}: {title}"))
                .collect::<Vec<_>>()
                .join("; ");
            let instructions = group
                .findings
                .iter()
                .map(|(finding_id, _, instructions, paths)| {
                    format!("{finding_id}: {instructions}\nLocations: {paths}")
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            let scope = format!(
                "Correct the attributed commit-range findings {finding_titles} in repository {}. Work from the current integrated HEAD; do not reopen or modify the original WorkUnit.",
                group.repo_id
            );
            let work_unit = CreateWorkUnit {
                id: id.clone(),
                task_id: request.task_id.clone(),
                parent_work_unit_id: None,
                title: format!("Correct range findings: {}", group.findings[0].1),
                scope,
                execution_instructions: Some(instructions),
                role: role_name,
                assigned_actor,
                requires_integration: true,
                provenance: Some(db::WorkUnitProvenance::Artifact(
                    report.report_artifact_id.clone(),
                )),
                artifact_input_ids: vec![report.report_artifact_id.clone()],
                repository_ids: vec![group.repo_id.clone()],
                review_approval_gate_id: None,
                review_approval_line_id: None,
                created_by: creator.clone(),
                created_at: now.to_owned(),
            };
            if let Some(previous) = previous_by_repo.insert(group.repo_id.clone(), id.clone()) {
                dependencies.push(AddWorkUnitDependency {
                    work_unit_id: id.clone(),
                    depends_on_work_unit_id: previous,
                    expected_version: 1,
                    created_by: creator.clone(),
                    created_at: now.to_owned(),
                });
            }
            for (finding_id, _, _, _) in group.findings {
                links.push(ReviewCorrectionLink {
                    correction_work_unit_id: id.clone(),
                    finding_id,
                    task_id: request.task_id.clone(),
                    repo_id: group.repo_id.clone(),
                    report_artifact_id: report.report_artifact_id.clone(),
                    base_commit: base_commit.clone(),
                    created_at: now.to_owned(),
                });
            }
            work_units.push(work_unit);
        }
        let consumed: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM task_rework_receipt WHERE task_id = ?)
                  + (SELECT COUNT(*) FROM review_rework_round WHERE task_id = ?)
                  + (SELECT COUNT(*) FROM task_review_triage triage WHERE triage.task_id = ?
                       AND (triage.disposition = 'recheck' OR EXISTS (
                           SELECT 1 FROM review_finding_triage finding_triage
                           WHERE finding_triage.report_artifact_id = triage.review_report_artifact_id
                             AND finding_triage.disposition = 'recheck'
                       )))",
        )
        .bind(&request.task_id)
        .bind(&request.task_id)
        .bind(&request.task_id)
        .fetch_one(self.db.pool())
        .await?;
        let current_recheck = ordered
            .iter()
            .any(|(_, decision)| decision.disposition == api_types::TaskReviewDisposition::Recheck);
        let round_number = consumed + if current_recheck { 2 } else { 1 };
        let max_rounds: i64 = sqlx::query_scalar(
            "SELECT max_rework_rounds FROM task_autonomy_limits WHERE task_id = ?",
        )
        .bind(&request.task_id)
        .fetch_one(self.db.pool())
        .await?;
        if round_number > max_rounds {
            return Err(ServiceError::conflict(
                "range correction and recheck round budget is exhausted",
            ));
        }
        let round = ReviewReworkRound {
            report_artifact_id: report.report_artifact_id.clone(),
            task_id: request.task_id.clone(),
            round: round_number,
            request_id: Some(request.id.clone()),
            created_by: creator.clone(),
            idempotency_key: format!("review-range-correction:{}", report.report_artifact_id),
            created_at: now.to_owned(),
        };
        let mut events = work_units
            .iter()
            .map(|unit| {
                self.event(
                    "work_unit.created",
                    "work_unit",
                    &unit.id,
                    &creator_kind,
                    Some(creator.id()),
                    &request.task_id,
                    json!({
                        "task_id": request.task_id,
                        "work_unit_id": unit.id,
                        "review_report_artifact_id": report.report_artifact_id,
                    }),
                    now,
                )
            })
            .collect::<Vec<_>>();
        events.push(self.event(
            "task.rework_requested",
            "artifact",
            &report.report_artifact_id,
            &creator_kind,
            Some(creator.id()),
            &request.task_id,
            json!({
                "task_id": request.task_id,
                "review_report_artifact_id": report.report_artifact_id,
                "round": round.round,
            }),
            now,
        ));
        Ok(ApplyReviewCorrections {
            round,
            work_units,
            dependencies,
            links,
            max_rework_rounds: max_rounds,
            events,
        })
    }
}

fn fallback_role(roles: &[db::TaskRole]) -> Result<(String, Option<db::ActorRef>)> {
    let mut roles = roles.iter();
    let Some(role) = roles.next() else {
        return Err(ServiceError::Coordination {
            code: "review.finding.rework_requires_human",
            message: "no implementer TaskRole can own this unattributed correction".to_owned(),
        });
    };
    if roles.next().is_some() {
        return Err(ServiceError::Coordination {
            code: "review.finding.rework_requires_human",
            message: "multiple implementer TaskRoles can own this unattributed correction"
                .to_owned(),
        });
    }
    Ok((role.role.clone(), None))
}
