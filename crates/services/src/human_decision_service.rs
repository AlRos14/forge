//! Structured human decisions (plan: coordinator-verification-and-pr-loop,
//! Congelado 2/4/9, node D1).
//!
//! The durable `human_decision_request` row (V174) is authoritative. This
//! service validates the source and evidence links before creation, enforces
//! answer authority from the *chosen option's* `authority_effect`, appends the
//! durable domain event in the same transaction as the write, publishes the
//! committed hint afterwards, and creates the exact `human_decision_resume`
//! coordination request that continues the paused request.
//!
//! Project Copilot has no entry point here: `answer` takes a human `user_id`
//! that the API lane must derive from the authenticated user.

use std::sync::Arc;

use db::{
    new_uuid_v4, now_rfc3339, AnswerHumanDecision, AuthorityEffect, CollaborationRepo,
    CreateDomainEvent, CreateHumanDecisionRequest, CreateTaskCoordinationRequestInput,
    ExecutionRepo, GateRepo, HumanDecisionRepo, HumanDecisionRequest, HumanDecisionSourceKind,
    HumanDecisionState, Page, PageRequest, SqliteDb, TaskCoordination, TaskCoordinationMode,
    TaskCoordinationRepo, TaskCoordinationRequest, TaskCoordinationRequestKind, TaskRepo,
    ValidationComparisonRepo, ValidationRunRepo, WorkUnitRepo,
};
use events::EventBus;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    coordinator_outputs::{
        AskHumanOutput, EvidenceKind, EvidenceLink, HumanDecisionOption, OutputError,
    },
    domain_event_service::DomainEventService,
    task_coordination::CoordinationBrief,
    Result, ServiceError,
};

pub const EVENT_HUMAN_DECISION_REQUESTED: &str = "task.human_decision_requested";
pub const EVENT_HUMAN_DECISION_ANSWERED: &str = "task.human_decision_answered";
pub const EVENT_HUMAN_DECISION_CLOSED: &str = "task.human_decision_closed";

pub const CODE_NOT_OPEN: &str = "human_decision.not_open";
pub const CODE_VERSION_CONFLICT: &str = "human_decision.version_conflict";
pub const CODE_AUTHORITY_REQUIRED: &str = "human_decision.authority_required";
pub const CODE_INVALID_OPTIONS: &str = "human_decision.invalid_options";
pub const CODE_EVIDENCE_INVALID: &str = "human_decision.evidence_invalid";

const MAX_NOTE_BYTES: usize = 2048;

fn decision_error(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::Coordination {
        code,
        message: message.into(),
    }
}

fn map_db(error: db::DbError) -> ServiceError {
    match error {
        db::DbError::VersionConflict => decision_error(
            CODE_VERSION_CONFLICT,
            "the human decision changed or is no longer open",
        ),
        other => other.into(),
    }
}

/// Exact paused request that an answer resumes. Mirrors the V174 binding
/// guard: the request must belong to the Task, with this kind and
/// authorization version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeBinding {
    pub request_id: String,
    pub request_kind: TaskCoordinationRequestKind,
    pub authorization_version: i64,
    /// Bounded exact subject descriptor of the paused request.
    pub source_subject: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaiseHumanDecisionInput {
    pub task_id: String,
    pub source_kind: HumanDecisionSourceKind,
    pub source_id: String,
    pub raised_by_execution_id: Option<String>,
    pub authorization_version: Option<i64>,
    pub ask: AskHumanOutput,
    pub resume_binding: ResumeBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaisedHumanDecision {
    pub decision: HumanDecisionRequest,
    /// `false` for an identical idempotent replay.
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerDecisionInput {
    pub task_id: String,
    pub decision_id: String,
    pub expected_version: i64,
    pub option_id: String,
    pub user_id: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// The exact `human_decision_resume` request now exists and is bound.
    Created(TaskCoordinationRequest),
    /// The decision was already bound to its resume request.
    AlreadyBound(String),
    /// The answer is durable but the resume request cannot be created now
    /// (revoked/stale authorization, manual mode, batch-bound request). The
    /// reconcile scanner retries `ensure_resume_request`.
    Deferred { code: &'static str, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerOutcome {
    pub decision: HumanDecisionRequest,
    pub resume: ResumeOutcome,
}

/// Minimum Project membership the answering user needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredAuthority {
    /// Authority to mutate the Task (explicit owner/admin/member).
    TaskMutation,
    /// Project owner/admin.
    ProjectAdministration,
}

/// `authority_effect != none` of the chosen option requires owner/admin.
pub fn required_authority(effect: AuthorityEffect) -> RequiredAuthority {
    match effect {
        AuthorityEffect::None => RequiredAuthority::TaskMutation,
        AuthorityEffect::Scope | AuthorityEffect::Authority => {
            RequiredAuthority::ProjectAdministration
        }
    }
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn parse_options(decision: &HumanDecisionRequest) -> Result<Vec<HumanDecisionOption>> {
    serde_json::from_str(&decision.options_json).map_err(|error| {
        decision_error(
            CODE_INVALID_OPTIONS,
            format!("stored decision options are unreadable: {error}"),
        )
    })
}

/// The chosen option must be one of the stored options; its effect decides
/// the required authority.
fn chosen_option_effect(
    decision: &HumanDecisionRequest,
    option_id: &str,
) -> Result<AuthorityEffect> {
    parse_options(decision)?
        .into_iter()
        .find(|option| option.id == option_id)
        .map(|option| option.authority_effect)
        .ok_or_else(|| {
            decision_error(
                CODE_INVALID_OPTIONS,
                "the answer must name one of the decision options",
            )
        })
}

fn validate_note(note: Option<&str>) -> Result<()> {
    if note.is_some_and(|note| note.len() > MAX_NOTE_BYTES) {
        return Err(ServiceError::invalid_operation(
            "a human decision note is limited to 2048 bytes",
        ));
    }
    Ok(())
}

/// Deterministic digest of the answered decision, stored as the resume
/// request's exact `source_subject_digest`.
fn decision_subject_digest(decision: &HumanDecisionRequest) -> String {
    sha256_hex(
        json!({
            "human_decision_id": decision.id,
            "source_kind": decision.source_kind.to_string(),
            "source_id": decision.source_id,
            "answered_option_id": decision.answered_option_id,
            "answered_by_user_id": decision.answered_by_user_id,
            "answered_at": decision.answered_at,
        })
        .to_string()
        .as_bytes(),
    )
}

fn resume_brief(
    decision: &HumanDecisionRequest,
    binding: &ResumeBinding,
    task_version: i64,
    coordination: &TaskCoordination,
) -> Result<String> {
    let brief = CoordinationBrief {
        objective: format!(
            "Resume {} request {} after human decision {}",
            binding.request_kind, binding.request_id, decision.id
        ),
        constraints: Some(
            "Continue only the paused request with the human's exact answer. Do not re-ask the answered question, widen scope, or exceed the answered option's authority.".to_owned(),
        ),
        acceptance_criteria: vec![
            "Decide the paused request using the exact human answer supplied in the request context".to_owned(),
        ],
        context: Some(
            json!({
                "task_version": task_version,
                "authorization_version": coordination.authorization_version,
                "coordinator_task_role_id": coordination.coordinator_task_role_id,
                "coordinator_actor": coordination.coordinator_actor,
                "team_digest": coordination.team_digest,
                "envelope_digest": coordination.envelope_digest,
                "human_decision_id": decision.id,
                "answered_option_id": decision.answered_option_id,
                "resumes_request_id": binding.request_id,
                "resumes_request_kind": binding.request_kind.to_string(),
            })
            .to_string(),
        ),
    };
    brief.validate()?;
    serde_json::to_string(&brief).map_err(|error| {
        ServiceError::invalid_operation(format!("resume brief serialization failed: {error}"))
    })
}

fn resume_request_digest(
    decision: &HumanDecisionRequest,
    binding: &ResumeBinding,
    task_version: i64,
    coordination: &TaskCoordination,
) -> String {
    sha256_hex(
        json!({
            "kind": "human_decision_resume",
            "task_id": decision.task_id,
            "task_version": task_version,
            "authorization_version": coordination.authorization_version,
            "team_generation": coordination.team_generation,
            "team_digest": coordination.team_digest,
            "envelope_digest": coordination.envelope_digest,
            "human_decision_id": decision.id,
            "answered_option_id": decision.answered_option_id,
            "resumes_request_id": binding.request_id,
        })
        .to_string()
        .as_bytes(),
    )
}

fn resume_idempotency_key(decision_id: &str, authorization_version: i64) -> String {
    format!("human-decision-resume:{decision_id}:{authorization_version}")
}

#[derive(Clone)]
pub struct HumanDecisionService {
    db: Arc<SqliteDb>,
    domain_events: DomainEventService,
}

impl HumanDecisionService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            domain_events: DomainEventService::new(Arc::clone(&db), event_bus),
            db,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn event(
        &self,
        event_type: &str,
        decision_id: &str,
        actor_type: &str,
        actor_id: Option<&str>,
        task_id: &str,
        payload: Value,
        now: &str,
    ) -> CreateDomainEvent {
        CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: event_type.to_owned(),
            entity_type: "human_decision_request".to_owned(),
            entity_id: decision_id.to_owned(),
            actor_type: actor_type.to_owned(),
            actor_id: actor_id.map(str::to_owned),
            scope_type: "task".to_owned(),
            scope_id: task_id.to_owned(),
            correlation_id: new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("{event_type}:{decision_id}")),
            payload_json: payload.to_string(),
            created_at: now.to_owned(),
        }
    }

    pub async fn get(&self, id: &str) -> Result<Option<HumanDecisionRequest>> {
        Ok(HumanDecisionRepo::get_human_decision_request(&*self.db, id).await?)
    }

    pub async fn list_by_task(
        &self,
        task_id: &str,
        state: Option<HumanDecisionState>,
        page: &PageRequest,
    ) -> Result<Page<HumanDecisionRequest>> {
        Ok(
            HumanDecisionRepo::list_human_decision_requests_by_task(
                &*self.db, task_id, state, page,
            )
            .await?,
        )
    }

    /// Create (or replay) one open decision after validating the exact source,
    /// every evidence link, the raising Execution's authority and the resume
    /// binding. Identical replays return the existing row with no new event.
    pub async fn raise(&self, input: RaiseHumanDecisionInput) -> Result<RaisedHumanDecision> {
        input.ask.validate().map_err(ServiceError::from)?;
        let task = TaskRepo::get_by_id(&*self.db, &input.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", input.task_id.clone()))?;
        self.validate_source(&input.task_id, input.source_kind, &input.source_id)
            .await?;
        for link in &input.ask.evidence_links {
            self.validate_evidence(&input.task_id, link).await?;
        }
        if let Some(execution_id) = input.raised_by_execution_id.as_deref() {
            self.validate_raiser(&input.task_id, execution_id, input.authorization_version)
                .await?;
        }
        self.validate_binding(
            &input.task_id,
            &input.resume_binding,
            input.authorization_version,
        )
        .await?;

        let id = new_uuid_v4();
        let now = now_rfc3339();
        let options_json = input.ask.options_json().map_err(ServiceError::from)?;
        let evidence_links_json = input
            .ask
            .evidence_links_json()
            .map_err(ServiceError::from)?;
        let max_authority_effect = input.ask.max_authority_effect();
        let resume_binding_json =
            serde_json::to_string(&input.resume_binding).map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "resume binding is not serializable: {error}"
                ))
            })?;
        let actor = input.raised_by_execution_id.as_deref();
        let event = self.event(
            EVENT_HUMAN_DECISION_REQUESTED,
            &id,
            if actor.is_some() { "agent" } else { "system" },
            actor,
            &task.id,
            json!({
                "task_id": task.id,
                "decision_id": id,
                "source_kind": input.source_kind.to_string(),
                "source_id": input.source_id,
                "authorization_version": input.authorization_version,
                "max_authority_effect": max_authority_effect.to_string(),
                "resumes_request_id": input.resume_binding.request_id,
                "state": HumanDecisionState::Open.to_string(),
            }),
            &now,
        );
        let write = HumanDecisionRepo::create_human_decision_request(
            &*self.db,
            CreateHumanDecisionRequest {
                id,
                task_id: task.id.clone(),
                source_kind: input.source_kind,
                source_id: input.source_id,
                raised_by_execution_id: input.raised_by_execution_id,
                authorization_version: input.authorization_version,
                question: input.ask.question,
                options_json,
                recommended_option_id: input.ask.recommended_option_id,
                recommendation_rationale: input.ask.recommendation_rationale,
                evidence_links_json,
                max_authority_effect,
                resume_binding_json,
                created_at: now,
            },
            Some(event),
        )
        .await
        .map_err(map_db)?;
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        Ok(RaisedHumanDecision {
            decision: write.decision,
            created: write.created,
        })
    }

    /// Record an answer (`expected_version`, authority by the chosen option's
    /// `authority_effect`) and then try to create the exact resume request.
    /// A resume that cannot be created yet never undoes the answer.
    pub async fn answer(&self, input: AnswerDecisionInput) -> Result<AnswerOutcome> {
        validate_note(input.note.as_deref())?;
        let decision = HumanDecisionRepo::get_human_decision_request(&*self.db, &input.decision_id)
            .await?
            .filter(|decision| decision.task_id == input.task_id)
            .ok_or_else(|| ServiceError::not_found("human decision", input.decision_id.clone()))?;
        if decision.state != HumanDecisionState::Open {
            return Err(decision_error(
                CODE_NOT_OPEN,
                "the human decision is no longer open",
            ));
        }
        if decision.version != input.expected_version {
            return Err(decision_error(
                CODE_VERSION_CONFLICT,
                "the human decision changed since it was read",
            ));
        }
        let effect = chosen_option_effect(&decision, &input.option_id)?;
        self.ensure_answer_authority(&decision.task_id, &input.user_id, effect)
            .await?;

        let now = now_rfc3339();
        let event = self.event(
            EVENT_HUMAN_DECISION_ANSWERED,
            &decision.id,
            "human",
            Some(&input.user_id),
            &decision.task_id,
            json!({
                "task_id": decision.task_id,
                "decision_id": decision.id,
                "answered_option_id": input.option_id,
                "authority_effect": effect.to_string(),
                "state": HumanDecisionState::Answered.to_string(),
                "resumes_request_id": serde_json::from_str::<ResumeBinding>(&decision.resume_binding_json)
                    .ok()
                    .map(|binding| binding.request_id),
            }),
            &now,
        );
        let write = HumanDecisionRepo::answer_human_decision(
            &*self.db,
            AnswerHumanDecision {
                id: decision.id.clone(),
                expected_version: input.expected_version,
                option_id: input.option_id,
                answered_by_user_id: input.user_id,
                note: input.note,
                answered_at: now,
            },
            Some(event),
        )
        .await
        .map_err(map_db)?;
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        let resume = match self.ensure_resume_request(&write.decision.id).await {
            Ok(outcome) => outcome,
            Err(ServiceError::Coordination { code, message }) => {
                ResumeOutcome::Deferred { code, message }
            }
            Err(ServiceError::Db(_)) => ResumeOutcome::Deferred {
                code: "human_decision.resume_failed",
                message: "the resume request could not be created; the reconcile scanner retries"
                    .to_owned(),
            },
            Err(error) => return Err(error),
        };
        let decision = HumanDecisionRepo::get_human_decision_request(&*self.db, &write.decision.id)
            .await?
            .unwrap_or(write.decision);
        Ok(AnswerOutcome { decision, resume })
    }

    /// `open -> superseded|cancelled` for a source that went stale. System
    /// operation: API routes must not expose it to arbitrary callers.
    pub async fn close(
        &self,
        decision_id: &str,
        expected_version: i64,
        to_state: HumanDecisionState,
        reason: &str,
    ) -> Result<HumanDecisionRequest> {
        if !matches!(
            to_state,
            HumanDecisionState::Superseded | HumanDecisionState::Cancelled
        ) {
            return Err(ServiceError::invalid_operation(
                "a decision closes only as superseded or cancelled",
            ));
        }
        let decision = HumanDecisionRepo::get_human_decision_request(&*self.db, decision_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("human decision", decision_id))?;
        let now = now_rfc3339();
        let event = self.event(
            EVENT_HUMAN_DECISION_CLOSED,
            decision_id,
            "system",
            None,
            &decision.task_id,
            json!({
                "task_id": decision.task_id,
                "decision_id": decision_id,
                "state": to_state.to_string(),
                "reason": reason.chars().take(256).collect::<String>(),
            }),
            &now,
        );
        let write = HumanDecisionRepo::close_human_decision(
            &*self.db,
            decision_id,
            expected_version,
            to_state,
            &now,
            Some(event),
        )
        .await
        .map_err(map_db)?;
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        Ok(write.decision)
    }

    /// Exact resume hook (idempotent; safe for the reconcile scanner). Creates
    /// the `human_decision_resume` coordination request bound to the answered
    /// decision under the current authorization and records it with
    /// `bind_human_decision_resume_request`.
    ///
    /// Not performed here (the wiring node RT owns it): consuming the
    /// coordination turn budget, waking/dispatching the coordinator for the
    /// new request, and batch-bound resumes.
    pub async fn ensure_resume_request(&self, decision_id: &str) -> Result<ResumeOutcome> {
        self.ensure_resume_request_inner(decision_id, None).await
    }

    /// Batch-bound resume: the caller (RT) resolved the exact Project batch
    /// binding with the `HumanDecisionResume` origin.
    pub async fn ensure_resume_request_for_batch(
        &self,
        decision_id: &str,
        binding: db::ProjectPlanBatchRequestBinding,
    ) -> Result<ResumeOutcome> {
        self.ensure_resume_request_inner(decision_id, Some(binding))
            .await
    }

    async fn ensure_resume_request_inner(
        &self,
        decision_id: &str,
        batch_binding: Option<db::ProjectPlanBatchRequestBinding>,
    ) -> Result<ResumeOutcome> {
        let decision = HumanDecisionRepo::get_human_decision_request(&*self.db, decision_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("human decision", decision_id))?;
        if decision.state != HumanDecisionState::Answered {
            return Err(decision_error(
                CODE_NOT_OPEN,
                "only an answered human decision can resume its request",
            ));
        }
        if let Some(resume_request_id) = decision.resume_request_id.clone() {
            return Ok(ResumeOutcome::AlreadyBound(resume_request_id));
        }
        let binding: ResumeBinding =
            serde_json::from_str(&decision.resume_binding_json).map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "stored resume binding is invalid: {error}"
                ))
            })?;
        let deferred = |code: &'static str, message: &str| {
            Ok(ResumeOutcome::Deferred {
                code,
                message: message.to_owned(),
            })
        };
        let Some(coordination) =
            TaskCoordinationRepo::get_task_coordination(&*self.db, &decision.task_id).await?
        else {
            return deferred(
                "coordination.not_authorized",
                "the Task has no coordination",
            );
        };
        let Some(requested_by) = coordination.requested_by.clone() else {
            return deferred(
                "coordination.not_authorized",
                "coordination has no authorizer",
            );
        };
        if coordination.revoked_at.is_some()
            || coordination.team_stale
            || coordination.mode == TaskCoordinationMode::Manual
            || coordination.authorization_version != binding.authorization_version
            || decision
                .authorization_version
                .is_some_and(|version| version != coordination.authorization_version)
        {
            return deferred(
                "coordination.authorization_stale",
                "the coordination authorization changed since the decision was raised",
            );
        }
        if batch_binding.is_none()
            && TaskCoordinationRepo::has_current_project_plan_batch_coordination_authorization(
                &*self.db,
                &decision.task_id,
            )
            .await?
        {
            return deferred(
                "human_decision.batch_resume_unsupported",
                "Project batch coordination resumes through the batch binding (RT)",
            );
        }
        let task = TaskRepo::get_by_id(&*self.db, &decision.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", decision.task_id.clone()))?;
        let brief_json = resume_brief(&decision, &binding, task.version, &coordination)?;
        let objective_digest = sha256_hex(brief_json.as_bytes());
        let request_digest =
            resume_request_digest(&decision, &binding, task.version, &coordination);
        let request_id = new_uuid_v4();
        let now = now_rfc3339();
        let idempotency_key =
            resume_idempotency_key(&decision.id, coordination.authorization_version);
        let subject_digest = decision_subject_digest(&decision);
        let mut event = self.event(
            "task.coordination_requested",
            &request_id,
            "system",
            None,
            &task.id,
            json!({
                "task_id": task.id,
                "request_id": request_id,
                "kind": TaskCoordinationRequestKind::HumanDecisionResume.to_string(),
                "authorization_version": coordination.authorization_version,
                "source_subject_kind": "human_decision",
                "source_subject_id": decision.id,
                "source_subject_digest": subject_digest,
                "resumes_request_id": binding.request_id,
                "objective_digest": objective_digest,
            }),
            &now,
        );
        event.dedupe_key = Some(format!(
            "human-decision-resume-request:{}:{}",
            coordination.authorization_version, decision.id
        ));
        let create_input = CreateTaskCoordinationRequestInput {
            id: request_id,
            task_id: task.id.clone(),
            kind: TaskCoordinationRequestKind::HumanDecisionResume,
            review_report_artifact_id: None,
            review_report_digest: None,
            review_producer_execution_id: None,
            source_subject_kind: Some("human_decision".to_owned()),
            source_subject_id: Some(decision.id.clone()),
            source_subject_digest: Some(subject_digest),
            resumes_request_id: Some(binding.request_id.clone()),
            requested_by,
            expected_task_version: task.version,
            authorization_version: coordination.authorization_version,
            mode: coordination.mode,
            brief_json,
            objective_digest,
            request_digest,
            idempotency_key,
            created_at: now.clone(),
            event,
        };
        let write = match batch_binding {
            Some(binding) => {
                TaskCoordinationRepo::create_project_plan_batch_coordination_request(
                    &*self.db,
                    create_input,
                    binding,
                )
                .await
            }
            None => {
                TaskCoordinationRepo::create_task_coordination_request(&*self.db, create_input)
                    .await
            }
        };
        let (request, request_event) = write.map_err(|error| match error {
            db::DbError::VersionConflict | db::DbError::TaskVersionConflict { .. } => {
                decision_error(
                    "coordination.authorization_stale",
                    "the Task or coordination changed before the resume request was created",
                )
            }
            other => other.into(),
        })?;
        if let Some(event) = request_event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        match HumanDecisionRepo::bind_human_decision_resume_request(
            &*self.db,
            &decision.id,
            decision.version,
            &request.id,
            &now,
        )
        .await
        {
            Ok(_) => {}
            // A concurrent/retried call may have bound it already; accept only
            // when it is bound to this exact request.
            Err(db::DbError::VersionConflict) => {
                let current =
                    HumanDecisionRepo::get_human_decision_request(&*self.db, &decision.id)
                        .await?
                        .ok_or_else(|| {
                            ServiceError::not_found("human decision", decision.id.clone())
                        })?;
                if current.resume_request_id.as_deref() != Some(request.id.as_str()) {
                    return Err(decision_error(
                        CODE_VERSION_CONFLICT,
                        "the human decision was bound to a different resume request",
                    ));
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(ResumeOutcome::Created(request))
    }

    async fn ensure_answer_authority(
        &self,
        task_id: &str,
        user_id: &str,
        effect: AuthorityEffect,
    ) -> Result<()> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id))?;
        let project = db::ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let allowed = match required_authority(effect) {
            RequiredAuthority::TaskMutation => {
                crate::project_authority::user_can_write_project(&self.db, &project, user_id)
                    .await?
            }
            RequiredAuthority::ProjectAdministration => {
                crate::project_authority::user_can_administer_project(&self.db, &project, user_id)
                    .await?
            }
        };
        if allowed {
            Ok(())
        } else {
            Err(decision_error(
                CODE_AUTHORITY_REQUIRED,
                match required_authority(effect) {
                    RequiredAuthority::TaskMutation => {
                        "answering requires authority to mutate the Task"
                    }
                    RequiredAuthority::ProjectAdministration => {
                        "this option changes scope or authority and requires a Project owner or admin"
                    }
                },
            ))
        }
    }

    async fn validate_source(
        &self,
        task_id: &str,
        kind: HumanDecisionSourceKind,
        source_id: &str,
    ) -> Result<()> {
        let source_task = match kind {
            HumanDecisionSourceKind::CoordinationRequest => {
                TaskCoordinationRepo::get_task_coordination_request(&*self.db, source_id)
                    .await?
                    .map(|source| source.task_id)
            }
            HumanDecisionSourceKind::QualityRequest => {
                TaskCoordinationRepo::get_task_quality_request(&*self.db, source_id)
                    .await?
                    .map(|source| source.task_id)
            }
            HumanDecisionSourceKind::GateEvaluation => {
                GateRepo::get_gate_evaluation(&*self.db, source_id)
                    .await?
                    .map(|source| source.task_id)
            }
            HumanDecisionSourceKind::ValidationRun => {
                ValidationRunRepo::get_validation_run(&*self.db, source_id)
                    .await?
                    .map(|source| source.task_id)
            }
            HumanDecisionSourceKind::ExternalFeedback
            | HumanDecisionSourceKind::CoordinatorReplan => {
                return Err(ServiceError::invalid_operation(format!(
                    "human decision source {kind} is not available in this phase"
                )));
            }
        };
        match source_task {
            Some(source_task) if source_task == task_id => Ok(()),
            _ => Err(ServiceError::not_found("human decision source", source_id)),
        }
    }

    async fn validate_evidence(&self, task_id: &str, link: &EvidenceLink) -> Result<()> {
        let invalid = |message: &str| decision_error(CODE_EVIDENCE_INVALID, message.to_owned());
        link.validate()
            .map_err(|error: OutputError| ServiceError::from(error))?;
        let (found_task, known_digest) = match link.kind {
            EvidenceKind::Artifact | EvidenceKind::ReviewReport => {
                let artifact = CollaborationRepo::get_artifact(&*self.db, &link.id).await?;
                if link.kind == EvidenceKind::ReviewReport
                    && artifact
                        .as_ref()
                        .is_some_and(|artifact| artifact.kind != db::ArtifactKind::ReviewReport)
                {
                    return Err(invalid(
                        "review_report evidence must be a ReviewReport Artifact",
                    ));
                }
                (
                    artifact.as_ref().map(|artifact| artifact.task_id.clone()),
                    artifact.and_then(|artifact| artifact.digest),
                )
            }
            EvidenceKind::ValidationRun => (
                ValidationRunRepo::get_validation_run(&*self.db, &link.id)
                    .await?
                    .map(|run| run.task_id),
                None,
            ),
            EvidenceKind::ValidationComparison => (
                ValidationComparisonRepo::get_validation_comparison(&*self.db, &link.id)
                    .await?
                    .map(|comparison| comparison.task_id),
                None,
            ),
            EvidenceKind::GateEvaluation => {
                let evaluation = GateRepo::get_gate_evaluation(&*self.db, &link.id).await?;
                (
                    evaluation
                        .as_ref()
                        .map(|evaluation| evaluation.task_id.clone()),
                    evaluation.map(|evaluation| evaluation.input_digest),
                )
            }
            EvidenceKind::Execution => (
                ExecutionRepo::get_by_id(&*self.db, &link.id)
                    .await?
                    .map(|execution| execution.task_id),
                None,
            ),
            EvidenceKind::WorkUnit => (
                WorkUnitRepo::get_by_id(&*self.db, &link.id)
                    .await?
                    .map(|work_unit| work_unit.task_id),
                None,
            ),
            EvidenceKind::ExternalFeedback => {
                return Err(invalid(
                    "external_feedback evidence is not available in this phase",
                ));
            }
        };
        if found_task.as_deref() != Some(task_id) {
            return Err(invalid(
                "evidence link does not name an artifact of this Task",
            ));
        }
        if let (Some(expected), Some(actual)) = (link.digest.as_deref(), known_digest.as_deref()) {
            if expected != actual {
                return Err(invalid(
                    "evidence link digest does not match the stored digest",
                ));
            }
        }
        Ok(())
    }

    /// The raising Execution must be the Task's current coordinator Actor under
    /// the exact authorization the decision records.
    async fn validate_raiser(
        &self,
        task_id: &str,
        execution_id: &str,
        authorization_version: Option<i64>,
    ) -> Result<()> {
        let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
            .await?
            .filter(|execution| execution.task_id == task_id)
            .ok_or_else(|| ServiceError::not_found("execution", execution_id))?;
        let coordination = TaskCoordinationRepo::get_task_coordination(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task coordination", task_id))?;
        if coordination.revoked_at.is_some()
            || coordination.team_stale
            || coordination.mode == TaskCoordinationMode::Manual
            || authorization_version
                .is_some_and(|version| version != coordination.authorization_version)
        {
            return Err(decision_error(
                "coordination.authorization_stale",
                "the coordination authorization is not current",
            ));
        }
        let actor = execution.actor_ref();
        if actor.is_none() || actor != coordination.coordinator_actor {
            return Err(decision_error(
                "coordination.output_not_permitted",
                "only the Task coordinator Actor may raise a human decision",
            ));
        }
        Ok(())
    }

    async fn validate_binding(
        &self,
        task_id: &str,
        binding: &ResumeBinding,
        authorization_version: Option<i64>,
    ) -> Result<()> {
        let request =
            TaskCoordinationRepo::get_task_coordination_request(&*self.db, &binding.request_id)
                .await?
                .filter(|request| request.task_id == task_id)
                .ok_or_else(|| {
                    ServiceError::not_found("coordination request", binding.request_id.clone())
                })?;
        if request.kind != binding.request_kind
            || request.authorization_version != binding.authorization_version
            || authorization_version.is_some_and(|version| version != binding.authorization_version)
        {
            return Err(decision_error(
                "coordination.authorization_stale",
                "the resume binding does not match the paused request",
            ));
        }
        if !binding.source_subject.is_object() {
            return Err(ServiceError::invalid_operation(
                "resume binding source_subject must be an object",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
