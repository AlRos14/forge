//! Node RT (phase 1): completion of `validation_findings` and
//! `human_decision_resume` turns, typed `outputs` (`request_validation`,
//! `ask_human`) and the answered-decision resume scanner.
//!
//! Output effects are idempotent by their own exact keys (check request
//! idempotency key; one question per source/authorization), so a replay of a
//! completed turn converges without a second effect or budget charge. The
//! `orchestrator_action` receipt table only admits the four historical action
//! types, so these outputs are applied here rather than through it.

use db::{
    HumanDecisionRepo, HumanDecisionSourceKind, HumanDecisionState, ValidationCheckRequestRepo,
    ValidationCheckSubjectKind, ValidationRunRepo, ValidationTriageDisposition, WorkUnitRepo,
};

use super::*;
use crate::{
    coordinator_outputs::{
        self, AskHumanOutput, CoordinatorOutput, CoordinatorOutputKind, EvidenceKind, EvidenceLink,
        HumanDecisionOption, ValidationSubjectKind,
    },
    human_decision_service::{
        HumanDecisionService, RaiseHumanDecisionInput, ResumeBinding, ResumeOutcome,
    },
    task_rework::ValidationFindingsDecision,
    validation_check_runner::EnqueueCheckRequest,
};

const MAX_TURN_BYTES: usize = 64 * 1024;

pub(crate) fn output_not_permitted(message: impl Into<String>) -> ServiceError {
    coordination_error("coordination.output_not_permitted", message)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationFindingsTurn {
    schema_version: u32,
    request_id: String,
    task_id: String,
    authorization_version: i64,
    validation_findings: Vec<ValidationFindingsDecision>,
    #[serde(default)]
    outputs: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HumanDecisionResumeTurn {
    schema_version: u32,
    request_id: String,
    task_id: String,
    authorization_version: i64,
    resumes_request_id: String,
    human_decision_id: String,
    answered_option_id: String,
    #[serde(default)]
    validation_findings: Vec<ValidationFindingsDecision>,
    #[serde(default)]
    coordinator_replan: Option<CoordinatorReplanResumeDecision>,
    #[serde(default)]
    outputs: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoordinatorReplanResumeDecision {
    decision: String,
}

fn turn_text(execution: &db::Execution, what: &str) -> Result<String> {
    let output = execution
        .summary
        .as_deref()
        .ok_or_else(|| invalid_plan(format!("the {what} Execution has no typed output")))?;
    if output.len() > MAX_TURN_BYTES {
        return Err(invalid_plan(format!("the {what} output exceeds 64 KiB")));
    }
    Ok(output.to_owned())
}

fn single_decision(
    mut decisions: Vec<ValidationFindingsDecision>,
) -> Result<ValidationFindingsDecision> {
    if decisions.len() != 1 {
        return Err(invalid_plan(
            "validation triage requires exactly one decision",
        ));
    }
    let decision = decisions.remove(0);
    Ok(decision)
}

impl TaskCoordinationService {
    /// Like `complete_turn`, with the TaskRole's explicitly permitted new
    /// output kinds (snapshot and current policy intersection).
    pub async fn complete_turn_with_outputs(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
        allowed_outputs: Option<&[CoordinatorOutputKind]>,
    ) -> Result<()> {
        match request.kind {
            TaskCoordinationRequestKind::ReviewFindings => {
                self.complete_review_findings_turn(request, execution, allowed_outputs)
                    .await
            }
            TaskCoordinationRequestKind::ValidationFindings => {
                self.complete_validation_findings_turn(request, execution, allowed_outputs)
                    .await
            }
            TaskCoordinationRequestKind::HumanDecisionResume => {
                self.complete_human_decision_resume_turn(request, execution)
                    .await
            }
            TaskCoordinationRequestKind::ExternalFeedback => {
                self.complete_external_feedback_turn(request, execution, allowed_outputs)
                    .await
            }
            _ => self.complete_turn(request, execution).await,
        }
    }

    /// Turn completion for kinds that `complete_turn` cannot know the policy of.
    pub(crate) async fn complete_new_kind_turn(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
    ) -> Result<()> {
        match request.kind {
            TaskCoordinationRequestKind::ValidationFindings => {
                self.complete_validation_findings_turn(request, execution, None)
                    .await
            }
            TaskCoordinationRequestKind::HumanDecisionResume => {
                self.complete_human_decision_resume_turn(request, execution)
                    .await
            }
            TaskCoordinationRequestKind::ExternalFeedback => {
                self.complete_external_feedback_turn(request, execution, None)
                    .await
            }
            _ => Err(output_not_permitted(
                "this coordination request kind is not available in this phase",
            )),
        }
    }

    async fn ensure_decision_matches_source(
        &self,
        source: &TaskCoordinationRequest,
        decision: &ValidationFindingsDecision,
    ) -> Result<()> {
        let (kind, id, digest) = match (
            source.source_subject_kind.as_deref(),
            source.source_subject_id.as_deref(),
            source.source_subject_digest.as_deref(),
        ) {
            (Some(kind), Some(id), Some(digest)) => (kind, id, digest),
            _ => return Err(invalid_plan("request has no exact validation source")),
        };
        let matches = match kind {
            "validation_run" => {
                decision.comparison_id.is_none() && decision.validation_run_id == id
            }
            "validation_comparison" => {
                let comparison =
                    db::ValidationComparisonRepo::get_validation_comparison(&*self.db, id).await?;
                decision.comparison_id.as_deref() == Some(id)
                    && comparison.is_some_and(|comparison| {
                        comparison.candidate_run_id == decision.validation_run_id
                    })
            }
            _ => false,
        };
        if !matches || decision.run_digest != digest {
            return Err(invalid_plan(
                "validation decision does not match its exact run, comparison and digest",
            ));
        }
        Ok(())
    }

    async fn apply_request_validation(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
        output: coordinator_outputs::RequestValidationOutput,
    ) -> Result<()> {
        let subject_kind = match output.subject.kind {
            ValidationSubjectKind::WorkUnitResult => ValidationCheckSubjectKind::WorkUnitResult,
            ValidationSubjectKind::TaskIntegration => ValidationCheckSubjectKind::TaskIntegration,
        };
        crate::validation_check_runner::enqueue_check_request(
            &self.db,
            &self.event_bus,
            EnqueueCheckRequest {
                task_id: request.task_id.clone(),
                repo_id: output.subject.repo_id,
                check_key: output.check_key,
                trigger: db::ValidationCheckRequestTrigger::CoordinatorRequest,
                subject_kind,
                subject_id: output.subject.id,
                subject_version: output.subject.version,
                baseline: output.baseline,
                requested_by_request_id: Some(request.id.clone()),
                requested_by_execution_id: Some(execution.id.clone()),
                reason: Some(output.reason),
            },
        )
        .await?;
        Ok(())
    }

    /// Re-enqueue the same pinned check on the current exact subject. When the
    /// subject did not change the exact idempotency key replays the existing
    /// request (no second run).
    async fn enqueue_recheck(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
        decision: &ValidationFindingsDecision,
    ) -> Result<()> {
        let run = ValidationRunRepo::get_validation_run(&*self.db, &decision.validation_run_id)
            .await?
            .filter(|run| run.task_id == request.task_id)
            .ok_or_else(|| {
                ServiceError::not_found("validation_run", decision.validation_run_id.clone())
            })?;
        let check = match run.check_request_id.as_deref() {
            Some(id) => {
                ValidationCheckRequestRepo::get_validation_check_request(&*self.db, id).await?
            }
            None => None,
        }
        .ok_or_else(|| output_not_permitted("recheck requires a catalog check request"))?;
        let definition = db::ValidationCatalogRepo::get_validation_check_definition(
            &*self.db,
            &check.check_definition_id,
        )
        .await?
        .ok_or_else(|| {
            ServiceError::not_found(
                "validation_check_definition",
                check.check_definition_id.clone(),
            )
        })?;
        let workspace = db::WorkspaceRepo::get_by_id(&*self.db, &check.workspace_id)
            .await?
            .filter(|workspace| workspace.task_id == request.task_id)
            .ok_or_else(|| ServiceError::not_found("workspace", check.workspace_id.clone()))?;
        let subject_version = match check.subject_kind {
            ValidationCheckSubjectKind::WorkUnitResult => {
                WorkUnitRepo::get_by_id(&*self.db, &check.subject_id)
                    .await?
                    .filter(|unit| unit.task_id == request.task_id)
                    .ok_or_else(|| ServiceError::not_found("work_unit", check.subject_id.clone()))?
                    .version
            }
            ValidationCheckSubjectKind::TaskIntegration => {
                TaskRepo::get_by_id(&*self.db, &request.task_id, false)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("task", request.task_id.clone()))?
                    .version
            }
        };
        crate::validation_check_runner::enqueue_check_request(
            &self.db,
            &self.event_bus,
            EnqueueCheckRequest {
                task_id: request.task_id.clone(),
                repo_id: workspace.repo_id,
                check_key: definition.check_key,
                trigger: db::ValidationCheckRequestTrigger::CoordinatorRequest,
                subject_kind: check.subject_kind,
                subject_id: check.subject_id,
                subject_version,
                baseline: check.baseline,
                requested_by_request_id: Some(request.id.clone()),
                requested_by_execution_id: Some(execution.id.clone()),
                reason: Some(format!("recheck of validation run {}", run.id)),
            },
        )
        .await?;
        Ok(())
    }

    fn synthesized_human_question(decision: &ValidationFindingsDecision) -> AskHumanOutput {
        let option = |id: &str, label: &str, description: &str| HumanDecisionOption {
            id: id.to_owned(),
            label: label.to_owned(),
            description: description.to_owned(),
            consequence: "The coordinator continues with this exact choice.".to_owned(),
            authority_effect: db::AuthorityEffect::None,
        };
        AskHumanOutput {
            question: format!(
                "Validation run {} needs a Human decision: {}",
                decision.validation_run_id,
                decision.rationale.chars().take(600).collect::<String>()
            ),
            options: vec![
                option(
                    "correct",
                    "Correct the defect",
                    "Create a bounded correction for the checked WorkUnit.",
                ),
                option(
                    "recheck",
                    "Run the check again",
                    "Run the pinned check again on the current exact subject.",
                ),
            ],
            recommended_option_id: None,
            recommendation_rationale: None,
            evidence_links: vec![EvidenceLink {
                kind: EvidenceKind::ValidationRun,
                id: decision.validation_run_id.clone(),
                digest: None,
            }],
        }
    }

    async fn complete_validation_findings_turn(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
        allowed_outputs: Option<&[CoordinatorOutputKind]>,
    ) -> Result<()> {
        let text = turn_text(execution, "ValidationFindings coordination")?;
        let turn: ValidationFindingsTurn = serde_json::from_str(&text).map_err(|_| {
            invalid_plan("the ValidationFindings output is not its typed JSON envelope")
        })?;
        if turn.schema_version != 2
            || turn.request_id != request.id
            || turn.task_id != request.task_id
            || turn.authorization_version != request.authorization_version
        {
            return Err(invalid_plan(
                "ValidationFindings requires schema 2 for its exact request and authorization",
            ));
        }
        let mut decision = single_decision(turn.validation_findings)?;
        self.ensure_decision_matches_source(request, &decision)
            .await?;
        let coordination = self.require_coordination(&request.task_id).await?;
        let envelope = self.envelope(&coordination)?;
        let micro_fix = decision.micro_fix.clone();
        if let Some(micro_fix) = micro_fix.as_ref() {
            if decision.disposition != ValidationTriageDisposition::Correct {
                return Err(output_not_permitted(
                    "micro_fix requires validation disposition correct",
                ));
            }
            coordinator_outputs::ensure_new_kind_permitted(
                CoordinatorOutputKind::MicroFix,
                allowed_outputs,
            )
            .map_err(ServiceError::from)?;
            if !envelope
                .micro_fix
                .as_ref()
                .is_some_and(|grant| grant.enabled)
            {
                return Err(output_not_permitted(
                    "micro_fix requires envelope.micro_fix.enabled",
                ));
            }
            let context = self.validation_source_context(request).await?;
            if context
                .pointer("/check/subject/kind")
                .and_then(Value::as_str)
                != Some("work_unit_result")
                || context
                    .pointer("/run/commit_sha")
                    .and_then(Value::as_str)
                    .is_none()
                || context
                    .pointer("/run/workspace_id")
                    .and_then(Value::as_str)
                    .is_none()
            {
                return Err(output_not_permitted(
                    "micro_fix requires a checked WorkUnit result with an exact repository commit and Workspace",
                ));
            }
            let stdout = context
                .pointer("/run/stdout_tail")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let stderr = context
                .pointer("/run/stderr_tail")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if micro_fix
                .finding_refs
                .iter()
                .any(|finding| !stdout.contains(finding) && !stderr.contains(finding))
                || micro_fix.paths.iter().any(|path| {
                    !micro_fix
                        .finding_refs
                        .iter()
                        .any(|finding| finding.contains(path))
                })
            {
                return Err(output_not_permitted(
                    "micro_fix findings and paths must be exact subsets of this validation output",
                ));
            }
            let used = db::MicroFixReceiptRepo::list_micro_fix_receipts_by_task(
                &*self.db,
                &request.task_id,
            )
            .await?
            .len();
            if used
                >= envelope
                    .micro_fix
                    .as_ref()
                    .map_or(0, |grant| grant.max_per_task as usize)
            {
                return Err(output_not_permitted(
                    "micro_fix.max_per_task is exhausted; route the finding through ordinary triage or a Human",
                ));
            }
            decision.instructions = format!(
                "{}\nDeclared micro-fix paths: {}",
                micro_fix.instructions,
                micro_fix.paths.join(", ")
            );
        }
        let outputs = coordinator_outputs::parse_outputs(turn.outputs, &envelope, allowed_outputs)?;
        let mut ask: Option<AskHumanOutput> = None;
        let mut validations = Vec::new();
        let mut propose_replan = None;
        for output in outputs {
            match output {
                CoordinatorOutput::RequestValidation(value) => validations.push(value),
                CoordinatorOutput::AskHuman(value) => {
                    if ask.replace(value).is_some() {
                        return Err(invalid_plan("a turn may carry at most one ask_human"));
                    }
                }
                CoordinatorOutput::ProposeReplan(value) => {
                    if propose_replan.replace(value).is_some() {
                        return Err(invalid_plan("a turn may carry at most one propose_replan"));
                    }
                }
            }
        }
        if ask.is_some() && decision.disposition != ValidationTriageDisposition::Human {
            return Err(invalid_plan(
                "ask_human requires the human disposition in a validation triage turn",
            ));
        }
        for value in validations {
            self.apply_request_validation(request, execution, value)
                .await?;
        }
        let service = HumanDecisionService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let mut raised = None;
        match decision.disposition {
            ValidationTriageDisposition::Recheck => {
                self.enqueue_recheck(request, execution, &decision).await?;
            }
            ValidationTriageDisposition::Human => {
                let ask = ask.unwrap_or_else(|| Self::synthesized_human_question(&decision));
                raised = Some(
                    service
                        .raise(RaiseHumanDecisionInput {
                            task_id: request.task_id.clone(),
                            source_kind: HumanDecisionSourceKind::ValidationRun,
                            source_id: decision.validation_run_id.clone(),
                            raised_by_execution_id: Some(execution.id.clone()),
                            authorization_version: Some(request.authorization_version),
                            ask,
                            resume_binding: ResumeBinding {
                                request_id: request.id.clone(),
                                request_kind: TaskCoordinationRequestKind::ValidationFindings,
                                authorization_version: request.authorization_version,
                                source_subject: json!({
                                    "source_subject_kind": request.source_subject_kind,
                                    "source_subject_id": request.source_subject_id,
                                    "source_subject_digest": request.source_subject_digest,
                                }),
                            },
                        })
                        .await?,
                );
            }
            ValidationTriageDisposition::Correct => {}
        }
        let validation_run_id = decision.validation_run_id.clone();
        let result = crate::task_rework::TaskReworkService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .record_validation_triage(&execution.id, &request.id, decision)
        .await;
        if let (Err(error), Some(raised)) = (&result, raised.as_ref()) {
            let paused = matches!(
                error,
                ServiceError::Coordination {
                    code: "coordination.paused",
                    ..
                }
            );
            if raised.created && !paused {
                service
                    .close(
                        &raised.decision.id,
                        raised.decision.version,
                        HumanDecisionState::Cancelled,
                        "the triage receipt could not be recorded",
                    )
                    .await
                    .ok();
            }
        }
        result?;
        if let Some(micro_fix) = micro_fix.as_ref() {
            crate::task_rework::TaskReworkService::new(
                Arc::clone(&self.db),
                Arc::clone(&self.event_bus),
            )
            .ensure_validation_micro_fix_receipt(
                &request.task_id,
                &request.id,
                request.authorization_version,
                &validation_run_id,
                micro_fix,
            )
            .await?;
        }
        if let Some(output) = propose_replan {
            self.apply_propose_replan(request, execution, output)
                .await?;
        }
        Ok(())
    }

    async fn complete_human_decision_resume_turn(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
    ) -> Result<()> {
        let text = turn_text(execution, "HumanDecisionResume coordination")?;
        let turn: HumanDecisionResumeTurn = serde_json::from_str(&text).map_err(|_| {
            invalid_plan("the HumanDecisionResume output is not its typed JSON envelope")
        })?;
        if turn.schema_version != 2
            || turn.request_id != request.id
            || turn.task_id != request.task_id
            || turn.authorization_version != request.authorization_version
            || request.resumes_request_id.as_deref() != Some(turn.resumes_request_id.as_str())
            || request.source_subject_id.as_deref() != Some(turn.human_decision_id.as_str())
        {
            return Err(invalid_plan(
                "HumanDecisionResume requires schema 2 for its exact request, decision and paused request",
            ));
        }
        if !turn.outputs.is_empty() {
            return Err(invalid_plan(
                "a HumanDecisionResume turn must carry an empty outputs list",
            ));
        }
        let decision_row =
            HumanDecisionRepo::get_human_decision_request(&*self.db, &turn.human_decision_id)
                .await?
                .filter(|row| row.task_id == request.task_id)
                .ok_or_else(|| {
                    ServiceError::not_found("human decision", turn.human_decision_id.clone())
                })?;
        if decision_row.state != HumanDecisionState::Answered
            || decision_row.answered_option_id.as_deref() != Some(turn.answered_option_id.as_str())
            || decision_row.resume_request_id.as_deref() != Some(request.id.as_str())
        {
            return Err(invalid_plan(
                "the resumed human decision does not match its exact answer",
            ));
        }
        let paused = TaskCoordinationRepo::get_task_coordination_request(
            &*self.db,
            &turn.resumes_request_id,
        )
        .await?
        .filter(|paused| paused.task_id == request.task_id)
        .ok_or_else(|| {
            ServiceError::not_found("coordination request", turn.resumes_request_id.clone())
        })?;
        if paused.kind == TaskCoordinationRequestKind::WorkUnitPlan
            && decision_row.source_kind == HumanDecisionSourceKind::CoordinatorReplan
        {
            if !turn.validation_findings.is_empty() {
                return Err(invalid_plan(
                    "a coordinator replan resume cannot contain validation findings",
                ));
            }
            let output = turn.coordinator_replan.ok_or_else(|| {
                invalid_plan("a coordinator replan resume must echo the exact Human option")
            })?;
            return self
                .complete_replan_human_resume(request, &decision_row, &paused, &output.decision)
                .await;
        }
        if turn.coordinator_replan.is_some() {
            return Err(invalid_plan(
                "coordinator_replan output is only valid for an exact replan resume",
            ));
        }
        if paused.kind != TaskCoordinationRequestKind::ValidationFindings {
            return Err(output_not_permitted(
                "only a paused validation_findings request can be resumed in this phase",
            ));
        }
        let decision = single_decision(turn.validation_findings)?;
        if decision.micro_fix.is_some() {
            return Err(output_not_permitted(
                "micro_fix is only valid on the original authorized triage request",
            ));
        }
        if decision.disposition == ValidationTriageDisposition::Human {
            return Err(invalid_plan(
                "a resumed validation decision must be correct or recheck",
            ));
        }
        self.ensure_decision_matches_source(&paused, &decision)
            .await?;
        if decision.disposition == ValidationTriageDisposition::Recheck {
            self.enqueue_recheck(request, execution, &decision).await?;
        }
        crate::task_rework::TaskReworkService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .record_validation_resume(&execution.id, &request.id, decision)
        .await
    }

    pub(crate) async fn exact_human_decision_resume_context(
        &self,
        request: &TaskCoordinationRequest,
    ) -> Result<Value> {
        if request.kind != TaskCoordinationRequestKind::HumanDecisionResume
            || !matches!(
                request.state,
                TaskCoordinationRequestState::Requested
                    | TaskCoordinationRequestState::Running
                    | TaskCoordinationRequestState::Uncertain
            )
        {
            return Err(ServiceError::invalid_operation(
                "resume context requires a runnable HumanDecisionResume request",
            ));
        }
        let decision_id = request
            .source_subject_id
            .as_deref()
            .ok_or_else(|| invalid_plan("resume request has no exact decision"))?;
        let decision = HumanDecisionRepo::get_human_decision_request(&*self.db, decision_id)
            .await?
            .filter(|row| {
                row.task_id == request.task_id
                    && row.state == HumanDecisionState::Answered
                    && row.resume_request_id.as_deref() == Some(request.id.as_str())
            })
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "the answered human decision no longer matches its resume request",
                )
            })?;
        let paused_id = request
            .resumes_request_id
            .as_deref()
            .ok_or_else(|| invalid_plan("resume request has no paused request"))?;
        let paused = TaskCoordinationRepo::get_task_coordination_request(&*self.db, paused_id)
            .await?
            .filter(|paused| paused.task_id == request.task_id)
            .ok_or_else(|| ServiceError::not_found("coordination request", paused_id.to_owned()))?;
        let paused_context = if paused.kind == TaskCoordinationRequestKind::ValidationFindings {
            self.validation_source_context(&paused).await?
        } else if paused.kind == TaskCoordinationRequestKind::WorkUnitPlan
            && decision.source_kind == HumanDecisionSourceKind::CoordinatorReplan
        {
            let replan = db::TaskCoordinatorReplanRepo::get_coordinator_replan(
                &*self.db,
                &decision.source_id,
            )
            .await?
            .filter(|row| row.task_id == request.task_id)
            .ok_or_else(|| invalid_plan("resume no longer matches its coordinator replan"))?;
            let plan = TaskCoordinationRepo::list_task_coordination_plans(
                &*self.db,
                &request.task_id,
                100,
            )
            .await?
            .into_iter()
            .find(|plan| plan.request_id == paused.id)
            .ok_or_else(|| invalid_plan("replan resume has no exact plan Artifact"))?;
            json!({
                "coordinator_replan_id": replan.id,
                "plan_request_id": paused.id,
                "plan_artifact_id": plan.artifact_id,
                "plan_digest": plan.plan_digest,
                "classification": replan.classification,
                "classification_facts_json": replan.classification_facts_json,
            })
        } else {
            Value::Null
        };
        let options: Value = serde_json::from_str(&decision.options_json).unwrap_or(Value::Null);
        let evidence: Value =
            serde_json::from_str(&decision.evidence_links_json).unwrap_or(Value::Null);
        Ok(json!({
            "human_decision_id": decision.id,
            "question": decision.question,
            "options": options,
            "evidence_links": evidence,
            "answered_option_id": decision.answered_option_id,
            "answer_note": decision.answer_note,
            "answered_by_user_id": decision.answered_by_user_id,
            "answered_at": decision.answered_at,
            "resumes_request_id": paused.id,
            "resumes_request_kind": paused.kind.to_string(),
            "paused_source": paused_context,
        }))
    }

    pub(crate) fn render_human_decision_resume_prompt(
        &self,
        request: &TaskCoordinationRequest,
        context: &Value,
    ) -> Result<String> {
        let decision_id = request
            .source_subject_id
            .as_deref()
            .ok_or_else(|| invalid_plan("resume request has no exact decision"))?;
        let paused_id = request
            .resumes_request_id
            .as_deref()
            .ok_or_else(|| invalid_plan("resume request has no paused request"))?;
        let answered = context
            .get("human_decision")
            .or_else(|| context.get("human_decision_resume"))
            .and_then(|value| value.get("answered_option_id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let resume_context = context.get("human_decision_resume").unwrap_or(context);
        if resume_context
            .get("human_decision_source_kind")
            .and_then(Value::as_str)
            == Some("coordinator_replan")
        {
            return Ok(format!(
                "Return exactly one JSON object matching `{{\"schema_version\":2,\"request_id\":{request_id},\"task_id\":{task_id},\"authorization_version\":{authorization_version},\"resumes_request_id\":{paused},\"human_decision_id\":{decision},\"answered_option_id\":{answered},\"coordinator_replan\":{{\"decision\":\"approve_replan|reject|revise_scope\"}},\"outputs\":[]}}`. Echo the exact Human option as `coordinator_replan.decision`; this is read-only and must not change scope. Context: {context}",
                request_id = serde_json::to_string(&request.id).unwrap(),
                task_id = serde_json::to_string(&request.task_id).unwrap(),
                authorization_version = request.authorization_version,
                paused = serde_json::to_string(paused_id).unwrap(),
                decision = serde_json::to_string(decision_id).unwrap(),
                answered = serde_json::to_string(answered).unwrap(),
                context = serde_json::to_string(context).unwrap_or_else(|_| "{}".to_owned()),
            ));
        }
        let digest = context
            .get("human_decision_resume")
            .and_then(|value| value.get("paused_source"))
            .and_then(|value| value.get("run_digest"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(format!(
            "You are the selected Task coordinator resuming one paused validation triage after a Human answered. This execution is read-only.\n\
Return exactly one JSON object and no markdown fences matching:\n\
{{\"schema_version\":2,\"request_id\":{request_id},\"task_id\":{task_id},\"authorization_version\":{authorization_version},\"resumes_request_id\":{paused},\"human_decision_id\":{decision},\"answered_option_id\":{answered},\"validation_findings\":[{{\"validation_run_id\":\"...\",\"comparison_id\":null,\"run_digest\":{digest},\"disposition\":\"correct|recheck\",\"rationale\":\"...\",\"instructions\":\"...\",\"micro_fix\":null}}],\"outputs\":[]}}\n\
Exactly one decision for the paused run (set comparison_id to the paused comparison id when the source is a comparison). Follow the Human's exact answer; do not re-ask the question, widen scope, or exceed the answered option's authority. `outputs` must be empty.\n\
Context: {context}",
            request_id = serde_json::to_string(&request.id).unwrap(),
            task_id = serde_json::to_string(&request.task_id).unwrap(),
            authorization_version = request.authorization_version,
            paused = serde_json::to_string(paused_id).unwrap(),
            decision = serde_json::to_string(decision_id).unwrap(),
            answered = serde_json::to_string(answered).unwrap(),
            digest = serde_json::to_string(digest).unwrap(),
            context = serde_json::to_string(context).unwrap_or_else(|_| "{}".to_owned()),
        ))
    }

    /// Durable reconcile scanner: answered decisions without a resume request.
    /// Creating the request consumes the coordination turn budget (repository
    /// transaction) and its durable event wakes the coordinator. Batch-bound
    /// Tasks resume through the exact `human_decision_resume` batch origin.
    pub async fn process_pending_human_decision_resumes(&self, limit: i64) -> Result<usize> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT d.id FROM human_decision_request d
             JOIN task_coordination c ON c.task_id=d.task_id
             WHERE d.state='answered' AND d.resume_request_id IS NULL
               AND d.source_kind!='external_feedback'
               AND c.turn_count < c.max_turns AND c.mode!='manual'
               AND c.revoked_at IS NULL AND c.team_stale=0
             ORDER BY d.answered_at, d.id LIMIT ?",
        )
        .bind(limit.clamp(1, 100))
        .fetch_all(self.db.pool())
        .await?;
        let service = HumanDecisionService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let mut created = 0;
        for id in ids {
            let outcome = match service.ensure_resume_request(&id).await {
                Ok(ResumeOutcome::Deferred {
                    code: "human_decision.batch_resume_unsupported",
                    ..
                }) => self.resume_batch_decision(&service, &id).await,
                other => other,
            };
            match outcome {
                Ok(ResumeOutcome::Created(_)) => created += 1,
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    decision_id = %id,
                    %error,
                    "human decision resume remains pending after a source-local error"
                ),
            }
        }
        Ok(created)
    }

    async fn resume_batch_decision(
        &self,
        service: &HumanDecisionService,
        decision_id: &str,
    ) -> Result<ResumeOutcome> {
        let deferred = |message: &str| {
            Ok(ResumeOutcome::Deferred {
                code: "coordination.paused",
                message: message.to_owned(),
            })
        };
        let Some(decision) =
            HumanDecisionRepo::get_human_decision_request(&*self.db, decision_id).await?
        else {
            return Err(ServiceError::not_found(
                "human decision",
                decision_id.to_owned(),
            ));
        };
        let Some(coordination) =
            TaskCoordinationRepo::get_task_coordination(&*self.db, &decision.task_id).await?
        else {
            return deferred("the Task has no coordination");
        };
        let Some(requested_by) = coordination.requested_by.clone() else {
            return deferred("coordination has no authorizer");
        };
        let task = TaskRepo::get_by_id(&*self.db, &decision.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", decision.task_id.clone()))?;
        let Some(binding) = self
            .project_batch_binding_for_task_origin(
                &decision.task_id,
                &requested_by,
                task.version,
                coordination.authorization_version,
                ProjectPlanBatchRequestOrigin::HumanDecisionResume,
            )
            .await?
        else {
            return deferred("the Project batch is paused or no longer current");
        };
        service
            .ensure_resume_request_for_batch(decision_id, binding)
            .await
    }
}

#[cfg(test)]
mod rt_tests {
    use super::*;
    use crate::coordinator_outputs::CheckPins;

    fn decision(disposition: &str, micro_fix: Value) -> Value {
        json!({
            "validation_run_id": "run-1",
            "comparison_id": null,
            "run_digest": "a".repeat(64),
            "disposition": disposition,
            "rationale": "reproducible failure",
            "instructions": "fix the failing assertion",
            "micro_fix": micro_fix,
        })
    }

    #[test]
    fn rt_validation_envelope_pins_only_on_request_checks() {
        let mut envelope = CoordinationEnvelope {
            schema_version: 2,
            ..CoordinationEnvelope::default()
        };
        envelope.validation_checks = vec![
            EnvelopeValidationCheck {
                check_key: "unit".to_owned(),
                definition_digest: "b".repeat(64),
                trigger: db::EnvelopeCheckTrigger::OnRequest,
            },
            EnvelopeValidationCheck {
                check_key: "post".to_owned(),
                definition_digest: "c".repeat(64),
                trigger: db::EnvelopeCheckTrigger::AfterIntegration,
            },
        ];
        assert!(envelope.pins_coordinator_request("unit"));
        assert!(!envelope.pins_coordinator_request("post"));
        assert!(!envelope.pins_coordinator_request("missing"));
    }

    #[test]
    fn rt_validation_new_outputs_need_an_explicit_allowed_list() {
        let envelope = CoordinationEnvelope {
            schema_version: 2,
            validation_checks: vec![EnvelopeValidationCheck {
                check_key: "unit".to_owned(),
                definition_digest: "b".repeat(64),
                trigger: db::EnvelopeCheckTrigger::OnRequest,
            }],
            ..CoordinationEnvelope::default()
        };
        let output = json!({
            "type": "request_validation",
            "check_key": "unit",
            "subject": {"kind": "work_unit_result", "id": "wu", "version": 3, "repo_id": "repo"},
            "baseline": true,
            "reason": "touches validation",
        });
        let denied =
            coordinator_outputs::parse_outputs(vec![output.clone()], &envelope, None).unwrap_err();
        assert_eq!(denied.code, "coordination.output_not_permitted");
        let allowed = [CoordinatorOutputKind::RequestValidation];
        let parsed =
            coordinator_outputs::parse_outputs(vec![output.clone()], &envelope, Some(&allowed))
                .unwrap();
        assert_eq!(parsed.len(), 1);
        let mut smuggled = output;
        smuggled["command"] = json!("rm -rf /");
        assert!(
            coordinator_outputs::parse_outputs(vec![smuggled], &envelope, Some(&allowed)).is_err()
        );
    }

    #[test]
    fn rt_validation_turn_requires_one_decision_and_preserves_micro_fix_shape() {
        let one: Vec<ValidationFindingsDecision> =
            serde_json::from_value(json!([decision("correct", Value::Null)])).unwrap();
        assert!(single_decision(one).is_ok());
        let two: Vec<ValidationFindingsDecision> = serde_json::from_value(json!([
            decision("correct", Value::Null),
            decision("recheck", Value::Null)
        ]))
        .unwrap();
        assert!(single_decision(two).is_err());
        let micro: Vec<ValidationFindingsDecision> = serde_json::from_value(json!([decision(
            "correct",
            json!({
                "finding_refs": ["missing assertion"],
                "instructions": "fix the assertion",
                "paths": ["src/check.rs"],
            })
        )]))
        .unwrap();
        assert!(single_decision(micro).is_ok());
    }

    #[test]
    fn rt_validation_turn_envelope_denies_unknown_fields() {
        let valid = json!({
            "schema_version": 2, "request_id": "r", "task_id": "t", "authorization_version": 1,
            "validation_findings": [decision("recheck", Value::Null)], "outputs": [],
        });
        assert!(serde_json::from_value::<ValidationFindingsTurn>(valid.clone()).is_ok());
        let mut extra = valid;
        extra["command"] = json!("x");
        assert!(serde_json::from_value::<ValidationFindingsTurn>(extra).is_err());
    }

    #[test]
    fn rt_human_decision_synthesized_question_is_a_valid_ask() {
        let decision: ValidationFindingsDecision =
            serde_json::from_value(decision("human", Value::Null)).unwrap();
        let ask = TaskCoordinationService::synthesized_human_question(&decision);
        ask.validate().unwrap();
        assert_eq!(ask.max_authority_effect(), db::AuthorityEffect::None);
    }

    #[test]
    fn rt_human_decision_resume_turn_shape_is_closed() {
        let valid = json!({
            "schema_version": 2, "request_id": "r", "task_id": "t", "authorization_version": 1,
            "resumes_request_id": "p", "human_decision_id": "d", "answered_option_id": "correct",
            "validation_findings": [decision("correct", Value::Null)], "outputs": [],
        });
        assert!(serde_json::from_value::<HumanDecisionResumeTurn>(valid.clone()).is_ok());
        let mut missing = valid;
        missing.as_object_mut().unwrap().remove("human_decision_id");
        assert!(serde_json::from_value::<HumanDecisionResumeTurn>(missing).is_err());
    }
}
