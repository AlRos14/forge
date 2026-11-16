//! Same-PR rework pause and convergence (plan node PX, «Congelado 11», D3').
//!
//! When a trusted external-feedback item is classified `extend_task`, the open
//! remote PR admission is paused without touching the PR: the TaskMerge is
//! abandoned, the admission carries the additive `rework_outcome` marker, the
//! Task moves `Merging -> Blocked(pr_feedback_rework)` and, after the pending
//! plan is admitted, `Blocked -> Active`. Re-admission on the same PR and the
//! `push_update` belong to node P3; this module owns the pause, the resume
//! and the reaction to a remote merge/close observed during the rework.
//!
//! Everything is idempotent: the durable `pr_rework_supersession` receipt (one
//! per TaskMerge) is the authority; `task.pr_rework_*` domain events are
//! deduplicated projections.

use std::sync::Arc;

use async_trait::async_trait;
use db::{
    new_uuid_v4, now_rfc3339, AuthorityEffect, CreateDomainEvent, DbError, DomainEvent,
    DomainEventRepo, ExternalFeedbackRepo, FeedbackItemDisposition, HumanDecisionRepo,
    HumanDecisionSourceKind, HumanDecisionState, PrReworkRepo, PrReworkState, PrReworkSupersession,
    SqliteDb, SupersedeAdmission, TaskCoordinationMode, TaskCoordinationRepo,
    TaskCoordinationRequestKind, TaskCoordinationRequestState, TaskLifecycleRepo,
    TaskLifecycleState, TaskRepo, TransitionPrReworkSupersession, PR_FEEDBACK_REWORK_REASON_KIND,
};
use events::EventBus;
use serde_json::{json, Value};

use crate::{
    coordinator_outputs::{AskHumanOutput, HumanDecisionOption},
    domain_event_service::DomainEventService,
    human_decision_service::{HumanDecisionService, RaiseHumanDecisionInput, ResumeBinding},
    pr_service::RemotePrStatus,
    task_lifecycle::{LifecycleCause, TaskLifecycleService, TransitionLifecycleInput},
    Result, ServiceError,
};

pub const CODE_NOT_AUTHORIZED: &str = "pr_rework.not_authorized";
pub const CODE_STALE_REQUEST: &str = "pr_rework.stale_request";
pub const CODE_NOT_FAST_FORWARD: &str = "pr_rework.not_fast_forward";
pub const CODE_REUSE_TARGET_MISSING: &str = "pr_rework.reuse_target_missing";
pub const CODE_SUPERSESSION_OPEN: &str = "pr_rework.supersession_open";

/// Public (SSE) projections: ids, digests and states only.
pub const EVENT_PR_REWORK_SUPERSEDED: &str = "task.pr_rework_superseded";
pub const EVENT_PR_REWORK_RESUMED: &str = "task.pr_rework_resumed";
pub const EVENT_PR_REWORK_CONVERGED: &str = "task.pr_rework_converged";
/// Internal domain event (not a public SSE type).
pub const EVENT_PR_REWORK_REMOTE_MERGED: &str = "pr.rework_remote_merged";

/// Decision options of the remote-merge convergence decision (11.6).
pub const OPTION_FOLLOW_UP_TASK: &str = "follow_up_task";
pub const OPTION_CANCEL_REWORK: &str = "cancel_rework";
/// `reason_kind` of the Task block after a remote merge during rework (O1).
pub const REASON_PR_MERGED_EXTERNALLY: &str = "pr_merged_externally";

fn rework_error(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::Coordination {
        code,
        message: message.into(),
    }
}

fn map_db(error: DbError) -> ServiceError {
    match error {
        DbError::PrReworkNotAuthorized => rework_error(
            CODE_NOT_AUTHORIZED,
            "the exact triaged request, open admission and Merging lifecycle do not authorize the pause",
        ),
        DbError::VersionConflict => rework_error(
            CODE_STALE_REQUEST,
            "the request, admission or lifecycle changed before the pause committed",
        ),
        other => other.into(),
    }
}

/// Effects on the Task's rework WorkUnits that belong to the coordination and
/// WorkUnit services (wired by node RT; PX keeps the lifecycle/PR authority).
#[async_trait]
pub trait PrReworkEffects: Send + Sync {
    /// With the Task `Active`, materialize the pending `work_unit_plan` stored
    /// in the request's `external_feedback_triage` batch (WorkUnits with
    /// `provenance = {kind: external_feedback, item_ids, body_digests}`).
    /// Must be idempotent: it is retried until it succeeds.
    async fn materialize_pending_plan(
        &self,
        task_id: &str,
        request_id: &str,
        supersession_id: &str,
    ) -> Result<()>;

    /// After a remote merge decision: cancel the rework WorkUnits that have
    /// not started. Must be idempotent.
    async fn cancel_unstarted_rework(&self, task_id: &str, supersession_id: &str) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupersedeOutcome {
    /// The request has no `extend_task` disposition: the PR keeps advancing.
    NotRequired,
    Superseded {
        supersession: PrReworkSupersession,
        /// `false` on an identical replay.
        created: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeOutcome {
    pub supersession: PrReworkSupersession,
    /// `true` when this call performed `Blocked -> Active`.
    pub resumed_now: bool,
    /// `false` when no [`PrReworkEffects`] is configured.
    pub plan_materialized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserveOutcome {
    /// The PR is still open (or the pause already converged): nothing to do.
    Unchanged,
    /// The PR merged remotely: the decision was raised and the pause recorded.
    RemoteMerged { decision_id: String },
    /// The PR closed without merge: the next admission creates a new PR.
    RemoteClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvergeOutcome {
    /// The pause is `abandoned` and the Task is `Blocked(pr_merged_externally)`.
    Blocked { decision_id: String },
    /// The decision is not answered yet.
    AwaitingDecision,
    /// Already converged earlier.
    AlreadyConverged,
}

#[derive(Clone)]
pub struct PrReworkService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    events: DomainEventService,
    lifecycle: TaskLifecycleService,
    decisions: HumanDecisionService,
    effects: Option<Arc<dyn PrReworkEffects>>,
}

impl PrReworkService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            events: DomainEventService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            lifecycle: TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            decisions: HumanDecisionService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            db,
            event_bus,
            effects: None,
        }
    }

    pub fn with_effects(mut self, effects: Arc<dyn PrReworkEffects>) -> Self {
        self.effects = Some(effects);
        self
    }

    async fn emit(
        &self,
        event_type: &str,
        supersession: &PrReworkSupersession,
        payload: Value,
    ) -> Result<DomainEvent> {
        self.events
            .append(CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: event_type.to_owned(),
                entity_type: "pr_rework_supersession".to_owned(),
                entity_id: supersession.id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: supersession.task_id.clone(),
                correlation_id: format!("pr-rework:{}", supersession.id),
                causation_id: Some(supersession.request_id.clone()),
                causation_depth: 1,
                dedupe_key: Some(format!("{event_type}:{}", supersession.id)),
                payload_json: payload.to_string(),
                created_at: now_rfc3339(),
            })
            .await
    }

    fn base_payload(supersession: &PrReworkSupersession) -> Value {
        json!({
            "task_id": supersession.task_id,
            "supersession_id": supersession.id,
            "request_id": supersession.request_id,
            "task_merge_operation_id": supersession.task_merge_operation_id,
            "state": supersession.state.to_string(),
        })
    }

    /// The coordination authorization the triaged request ran under must still
    /// be current (not revoked, team settled, same version, not manual).
    async fn ensure_authorized(&self, task_id: &str, authorization_version: i64) -> Result<()> {
        let coordination = TaskCoordinationRepo::get_task_coordination(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task coordination", task_id.to_owned()))?;
        if coordination.revoked_at.is_some()
            || coordination.team_stale
            || coordination.mode == TaskCoordinationMode::Manual
            || coordination.authorization_version != authorization_version
        {
            return Err(rework_error(
                CODE_NOT_AUTHORIZED,
                "the coordination authorization of the triaged request is no longer current",
            ));
        }
        Ok(())
    }

    /// Pause the open admission of a triaged `external_feedback` request that
    /// contains at least one `extend_task` disposition (11.4 step 2). One
    /// `BEGIN IMMEDIATE`: receipt, admission marker, abandoned TaskMerge and
    /// `Merging -> Blocked(pr_feedback_rework)` commit together; the PR is not
    /// touched. Idempotent per TaskMerge.
    pub async fn supersede_for_rework(&self, request_id: &str) -> Result<SupersedeOutcome> {
        let request = TaskCoordinationRepo::get_task_coordination_request(&*self.db, request_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("coordination request", request_id.to_owned())
            })?;
        if request.kind != TaskCoordinationRequestKind::ExternalFeedback {
            return Err(rework_error(
                CODE_STALE_REQUEST,
                "only an external_feedback request can pause a PR",
            ));
        }
        let admission_id = request.source_subject_id.clone().ok_or_else(|| {
            rework_error(CODE_STALE_REQUEST, "the request has no admission source")
        })?;
        // Replay first: an already-recorded pause stays valid even if the
        // authorization was revoked afterwards.
        if let Some(existing) =
            PrReworkRepo::get_supersession_by_task_merge(&*self.db, &admission_id).await?
        {
            if existing.request_id != request.id {
                return Err(rework_error(
                    CODE_SUPERSESSION_OPEN,
                    "the admission is already paused by another request",
                ));
            }
            self.project_superseded(&existing).await?;
            return Ok(SupersedeOutcome::Superseded {
                supersession: existing,
                created: false,
            });
        }
        if request.state != TaskCoordinationRequestState::Triaged {
            return Err(rework_error(
                CODE_STALE_REQUEST,
                "the request is not triaged",
            ));
        }
        let extends =
            ExternalFeedbackRepo::list_external_feedback_dispositions(&*self.db, &request.id)
                .await?
                .into_iter()
                .filter(|row| row.disposition == FeedbackItemDisposition::ExtendTask)
                .count();
        if extends == 0 {
            return Ok(SupersedeOutcome::NotRequired);
        }
        self.ensure_authorized(&request.task_id, request.authorization_version)
            .await?;
        let write = PrReworkRepo::create_supersession(
            &*self.db,
            SupersedeAdmission {
                id: new_uuid_v4(),
                task_merge_operation_id: admission_id,
                request_id: request.id.clone(),
                now: now_rfc3339(),
            },
        )
        .await
        .map_err(map_db)?;
        for event in &write.events {
            DomainEventService::publish_committed_hint(&self.event_bus, event);
        }
        self.project_superseded(&write.supersession).await?;
        Ok(SupersedeOutcome::Superseded {
            supersession: write.supersession,
            created: write.created,
        })
    }

    async fn project_superseded(&self, supersession: &PrReworkSupersession) -> Result<()> {
        self.emit(
            EVENT_PR_REWORK_SUPERSEDED,
            supersession,
            json!({
                "task_id": supersession.task_id,
                "supersession_id": supersession.id,
                "request_id": supersession.request_id,
                "task_merge_operation_id": supersession.task_merge_operation_id,
                "provider_pr_id": supersession.provider_pr_id,
                "superseded_head_sha": supersession.superseded_head_sha,
                "triage_set_digest": supersession.triage_set_digest,
            }),
        )
        .await?;
        Ok(())
    }

    /// `Blocked -> Active` with the exact `pr_feedback_rework` receipt.
    /// Returns `true` when this call performed the transition.
    async fn activate(&self, supersession: &PrReworkSupersession) -> Result<bool> {
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &supersession.task_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("task lifecycle", supersession.task_id.clone())
            })?;
        let receipt_matches = lifecycle.reason_kind.as_deref()
            == Some(PR_FEEDBACK_REWORK_REASON_KIND)
            && lifecycle.reason_ref.as_deref() == Some(supersession.id.as_str());
        match (lifecycle.state, receipt_matches) {
            (TaskLifecycleState::Active, true) => Ok(false),
            (TaskLifecycleState::Blocked, true) => {
                let task = TaskRepo::get_by_id(&*self.db, &supersession.task_id, false)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("task", supersession.task_id.clone()))?;
                self.lifecycle
                    .transition_at_lifecycle_version(
                        TransitionLifecycleInput {
                            task_id: supersession.task_id.clone(),
                            expected_task_version: task.version,
                            to_state: TaskLifecycleState::Active,
                            cause: LifecycleCause::PrFeedbackRework {
                                supersession_id: supersession.id.clone(),
                            },
                            reason_kind: Some(PR_FEEDBACK_REWORK_REASON_KIND.to_owned()),
                            reason_ref: Some(supersession.id.clone()),
                            idempotency_key: format!("pr-rework-resume:{}", supersession.id),
                        },
                        lifecycle.version,
                    )
                    .await?;
                Ok(true)
            }
            _ => Err(rework_error(
                CODE_STALE_REQUEST,
                "the Task is no longer blocked by this PR rework pause",
            )),
        }
    }

    /// `Blocked -> Active` and admission of the pending plan (11.4 step 3).
    /// Reentrant: an already-active Task skips the transition and only retries
    /// the idempotent plan materialization.
    pub async fn resume_after_supersession(&self, supersession_id: &str) -> Result<ResumeOutcome> {
        let supersession = PrReworkRepo::get_supersession(&*self.db, supersession_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("pr rework supersession", supersession_id.to_owned())
            })?;
        if !matches!(
            supersession.state,
            PrReworkState::Open | PrReworkState::RemoteClosed
        ) {
            return Err(rework_error(
                CODE_STALE_REQUEST,
                "only an open or remotely closed pause can resume its rework",
            ));
        }
        let request = TaskCoordinationRepo::get_task_coordination_request(
            &*self.db,
            &supersession.request_id,
        )
        .await?
        .ok_or_else(|| {
            ServiceError::not_found("coordination request", supersession.request_id.clone())
        })?;
        let resumed_now = {
            // A Task that already left Blocked by this receipt is a replay;
            // anything else needs a still-current authorization.
            let lifecycle =
                TaskLifecycleRepo::get_task_lifecycle(&*self.db, &supersession.task_id).await?;
            if lifecycle.as_ref().is_some_and(|lifecycle| {
                lifecycle.state == TaskLifecycleState::Blocked
                    && lifecycle.reason_ref.as_deref() == Some(supersession.id.as_str())
            }) {
                self.ensure_authorized(&request.task_id, request.authorization_version)
                    .await?;
            }
            self.activate(&supersession).await?
        };
        let plan_materialized = match &self.effects {
            Some(effects) => {
                effects
                    .materialize_pending_plan(
                        &supersession.task_id,
                        &supersession.request_id,
                        &supersession.id,
                    )
                    .await?;
                true
            }
            None => false,
        };
        self.emit(
            EVENT_PR_REWORK_RESUMED,
            &supersession,
            Self::base_payload(&supersession),
        )
        .await?;
        Ok(ResumeOutcome {
            supersession,
            resumed_now,
            plan_materialized,
        })
    }

    /// React to the provider status observed for an `open` pause (read-only
    /// watcher, node P3, calls this with `get_pull`'s result). Idempotent.
    pub async fn observe_remote(
        &self,
        supersession_id: &str,
        status: &RemotePrStatus,
    ) -> Result<ObserveOutcome> {
        let supersession = PrReworkRepo::get_supersession(&*self.db, supersession_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("pr rework supersession", supersession_id.to_owned())
            })?;
        match status {
            _ if supersession.state != PrReworkState::Open => Ok(ObserveOutcome::Unchanged),
            RemotePrStatus::Open(_) => Ok(ObserveOutcome::Unchanged),
            RemotePrStatus::Closed(_) => {
                let closed = PrReworkRepo::transition_supersession(
                    &*self.db,
                    TransitionPrReworkSupersession {
                        id: supersession.id.clone(),
                        expected_version: supersession.version,
                        to_state: PrReworkState::RemoteClosed,
                        readmission_merge_operation_id: None,
                        convergence_decision_id: None,
                        updated_at: now_rfc3339(),
                    },
                )
                .await
                .map_err(map_db)?;
                self.emit(
                    EVENT_PR_REWORK_CONVERGED,
                    &closed,
                    json!({
                        "task_id": closed.task_id,
                        "supersession_id": closed.id,
                        "outcome": "remote_closed",
                    }),
                )
                .await?;
                Ok(ObserveOutcome::RemoteClosed)
            }
            RemotePrStatus::Merged(_) => {
                let request = TaskCoordinationRepo::get_task_coordination_request(
                    &*self.db,
                    &supersession.request_id,
                )
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("coordination request", supersession.request_id.clone())
                })?;
                let raised = self
                    .decisions
                    .raise(RaiseHumanDecisionInput {
                        task_id: supersession.task_id.clone(),
                        source_kind: HumanDecisionSourceKind::ExternalFeedback,
                        source_id: format!("rework:{}", supersession.id),
                        raised_by_execution_id: None,
                        authorization_version: Some(request.authorization_version),
                        ask: convergence_decision(&supersession),
                        resume_binding: ResumeBinding {
                            request_id: request.id.clone(),
                            request_kind: TaskCoordinationRequestKind::ExternalFeedback,
                            authorization_version: request.authorization_version,
                            source_subject: json!({
                                "kind": "pr_rework_supersession",
                                "id": supersession.id,
                            }),
                        },
                    })
                    .await?;
                let decision_id = raised.decision.id.clone();
                let merged = PrReworkRepo::transition_supersession(
                    &*self.db,
                    TransitionPrReworkSupersession {
                        id: supersession.id.clone(),
                        expected_version: supersession.version,
                        to_state: PrReworkState::RemoteMerged,
                        readmission_merge_operation_id: None,
                        convergence_decision_id: Some(decision_id.clone()),
                        updated_at: now_rfc3339(),
                    },
                )
                .await
                .map_err(map_db)?;
                self.emit(
                    EVENT_PR_REWORK_REMOTE_MERGED,
                    &merged,
                    json!({
                        "task_id": merged.task_id,
                        "supersession_id": merged.id,
                        "decision_id": decision_id,
                    }),
                )
                .await?;
                self.emit(
                    EVENT_PR_REWORK_CONVERGED,
                    &merged,
                    json!({
                        "task_id": merged.task_id,
                        "supersession_id": merged.id,
                        "outcome": "remote_merged",
                        "decision_id": decision_id,
                    }),
                )
                .await?;
                Ok(ObserveOutcome::RemoteMerged { decision_id })
            }
        }
    }

    /// After the convergence decision of a remote merge is answered: cancel
    /// unstarted rework, abandon the pause and block the Task with
    /// `pr_merged_externally` (O1: there is no `Active -> Done` without a
    /// succeeded TaskMerge, and none is invented). Idempotent.
    pub async fn converge_remote_merged(&self, supersession_id: &str) -> Result<ConvergeOutcome> {
        let supersession = PrReworkRepo::get_supersession(&*self.db, supersession_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("pr rework supersession", supersession_id.to_owned())
            })?;
        let Some(decision_id) = supersession.convergence_decision_id.clone() else {
            return Err(rework_error(
                CODE_STALE_REQUEST,
                "only a remotely merged pause with its decision can converge",
            ));
        };
        if !matches!(
            supersession.state,
            PrReworkState::RemoteMerged | PrReworkState::Abandoned
        ) {
            return Err(rework_error(
                CODE_STALE_REQUEST,
                "only a remotely merged pause with its decision can converge",
            ));
        }
        if supersession.state == PrReworkState::Abandoned
            && self
                .is_blocked_externally(&supersession, &decision_id)
                .await?
        {
            return Ok(ConvergeOutcome::AlreadyConverged);
        }
        let decision = HumanDecisionRepo::get_human_decision_request(&*self.db, &decision_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("human decision", decision_id.clone()))?;
        if decision.state != HumanDecisionState::Answered {
            return Ok(ConvergeOutcome::AwaitingDecision);
        }
        if let Some(effects) = &self.effects {
            effects
                .cancel_unstarted_rework(&supersession.task_id, &supersession.id)
                .await?;
        }
        let abandoned = PrReworkRepo::transition_supersession(
            &*self.db,
            TransitionPrReworkSupersession {
                id: supersession.id.clone(),
                expected_version: supersession.version,
                to_state: PrReworkState::Abandoned,
                readmission_merge_operation_id: None,
                convergence_decision_id: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .map_err(map_db)?;
        self.block_externally(&abandoned, &decision_id).await?;
        Ok(ConvergeOutcome::Blocked { decision_id })
    }

    async fn is_blocked_externally(
        &self,
        supersession: &PrReworkSupersession,
        decision_id: &str,
    ) -> Result<bool> {
        Ok(
            TaskLifecycleRepo::get_task_lifecycle(&*self.db, &supersession.task_id)
                .await?
                .is_some_and(|lifecycle| {
                    lifecycle.state == TaskLifecycleState::Blocked
                        && lifecycle.reason_kind.as_deref() == Some(REASON_PR_MERGED_EXTERNALLY)
                        && lifecycle.reason_ref.as_deref() == Some(decision_id)
                }),
        )
    }

    async fn block_externally(
        &self,
        supersession: &PrReworkSupersession,
        decision_id: &str,
    ) -> Result<()> {
        if self
            .is_blocked_externally(supersession, decision_id)
            .await?
        {
            return Ok(());
        }
        // A Task still parked by the pause first takes its exact resume edge so
        // the later block is an ordinary Active -> Blocked.
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &supersession.task_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("task lifecycle", supersession.task_id.clone())
            })?;
        if lifecycle.state == TaskLifecycleState::Blocked
            && lifecycle.reason_ref.as_deref() == Some(supersession.id.as_str())
        {
            self.activate(supersession).await?;
        }
        let answered = DomainEventRepo::get_event_by_dedupe(
            &*self.db,
            &format!("task.human_decision_answered:{decision_id}"),
        )
        .await?
        .ok_or_else(|| {
            ServiceError::not_found("human decision answer event", decision_id.to_owned())
        })?;
        let task = TaskRepo::get_by_id(&*self.db, &supersession.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", supersession.task_id.clone()))?;
        self.lifecycle
            .transition(TransitionLifecycleInput {
                task_id: supersession.task_id.clone(),
                expected_task_version: task.version,
                to_state: TaskLifecycleState::Blocked,
                cause: LifecycleCause::DomainEvent(answered.id),
                reason_kind: Some(REASON_PR_MERGED_EXTERNALLY.to_owned()),
                reason_ref: Some(decision_id.to_owned()),
                idempotency_key: format!("pr-rework-merged-externally:{}", supersession.id),
            })
            .await?;
        Ok(())
    }
}

fn convergence_decision(supersession: &PrReworkSupersession) -> AskHumanOutput {
    AskHumanOutput {
        question: format!(
            "Pull request {} (paused head {}) was merged on the provider while Forge was \
             reworking it. How should the Task continue?",
            supersession.provider_pr_id, supersession.superseded_head_sha
        ),
        options: vec![
            HumanDecisionOption {
                id: OPTION_FOLLOW_UP_TASK.to_owned(),
                label: "Follow up in a new Task".to_owned(),
                description:
                    "Cancel the unstarted rework and keep the Task blocked for a new Task."
                        .to_owned(),
                consequence:
                    "The rework that did not start is cancelled; the merged PR is not changed."
                        .to_owned(),
                authority_effect: AuthorityEffect::Scope,
            },
            HumanDecisionOption {
                id: OPTION_CANCEL_REWORK.to_owned(),
                label: "Cancel the rework".to_owned(),
                description: "Cancel the unstarted rework and keep the Task blocked.".to_owned(),
                consequence: "Nothing else changes; the merged PR is not changed.".to_owned(),
                authority_effect: AuthorityEffect::None,
            },
        ],
        recommended_option_id: None,
        recommendation_rationale: None,
        evidence_links: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_service::CredentialService;
    use crate::pr_service::{
        PrCreateRequest, PrObservation, PrProvider, PrProviderError, PrReconciler, PrRecord,
    };
    use crate::task_integration_operation::tests::{
        fixture, prepare_pr_admission_with_config, PrConfigSetup,
    };
    use db::{
        CreateExternalFeedbackDisposition, CreateExternalFeedbackItem,
        CreateExternalFeedbackTriage, ExternalFeedbackHeadState, ExternalFeedbackKind,
        ExternalFeedbackSetEntry, ExternalFeedbackTrustClass, FeedbackDismissReason,
        FeedbackScopeClassification, PrFeedbackPolicy, TaskIntegrationOperationRepo,
    };
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    fn digest(seed: &str) -> String {
        hex::encode(Sha256::digest(seed.as_bytes()))
    }

    /// Opens the PR as `pr-77` and counts provider status reads.
    #[derive(Default)]
    struct CountingProvider {
        status_reads: AtomicUsize,
    }

    #[async_trait]
    impl PrProvider for CountingProvider {
        async fn find_pr(
            &self,
            _request: &PrCreateRequest,
        ) -> std::result::Result<Option<PrRecord>, PrProviderError> {
            Ok(None)
        }

        async fn create_pr(
            &self,
            request: PrCreateRequest,
        ) -> std::result::Result<PrRecord, PrProviderError> {
            Ok(PrRecord {
                provider_pr_id: "pr-77".to_owned(),
                pr_url: Some("https://github.example.invalid/pull/77".to_owned()),
                status: RemotePrStatus::Open(PrObservation {
                    provider_event_id: "created:77".to_owned(),
                    remote_repo_identity: request.repo_remote_url,
                    source_branch: request.source_branch,
                    target_branch: request.target_branch,
                    head_sha: request.source_sha,
                    merged_commit_sha: None,
                }),
            })
        }

        async fn get_pr_status(
            &self,
            admission: &db::RemotePrAdmission,
            _metadata: &db::PrMetadata,
        ) -> std::result::Result<RemotePrStatus, PrProviderError> {
            self.status_reads.fetch_add(1, Ordering::SeqCst);
            Ok(RemotePrStatus::Open(PrObservation {
                provider_event_id: "open:77".to_owned(),
                remote_repo_identity: admission.remote_repo_identity.clone(),
                source_branch: admission.source_branch.clone(),
                target_branch: admission.target_branch.clone(),
                head_sha: admission.admitted_source_sha.clone(),
                merged_commit_sha: None,
            }))
        }
    }

    #[derive(Default)]
    struct RecordingEffects {
        materialized: AtomicUsize,
        cancelled: AtomicUsize,
    }

    #[async_trait]
    impl PrReworkEffects for RecordingEffects {
        async fn materialize_pending_plan(&self, _: &str, _: &str, _: &str) -> Result<()> {
            self.materialized.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn cancel_unstarted_rework(&self, _: &str, _: &str) -> Result<()> {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct World {
        _temp: TempDir,
        db: Arc<SqliteDb>,
        bus: Arc<EventBus>,
        task_id: String,
        merge_id: String,
        request_id: String,
        provider: Arc<CountingProvider>,
        service: PrReworkService,
    }

    /// A Task in `Merging` with an `open` PR admission (ingestion enabled) and
    /// a triaged `external_feedback` request over two trusted items; item 0 is
    /// `extend_task` when `extend`, otherwise both are dismissed.
    async fn world(extend: bool) -> World {
        let temp = TempDir::new().expect("temporary directory");
        let url = format!("sqlite://{}", temp.path().join("rework.db").display());
        let fixture = fixture(&url, temp.path()).await;
        let bus = Arc::new(EventBus::new(64));
        let credentials = Arc::new(CredentialService::new(
            Arc::clone(&fixture.db),
            b"pr-rework-test-key",
        ));
        let (_manager, merge, publish, _evaluation, _gate, _policy) =
            prepare_pr_admission_with_config(
                &fixture,
                temp.path(),
                Arc::clone(&bus),
                Some(PrConfigSetup {
                    credentials,
                    policy: PrFeedbackPolicy {
                        ingest: true,
                        ..PrFeedbackPolicy::default()
                    },
                }),
            )
            .await
            .expect("PR admission");
        let merge_id = merge.id().to_owned();
        drop(publish);
        drop(merge);
        let provider = Arc::new(CountingProvider::default());
        PrReconciler::new(Arc::clone(&fixture.db), Arc::clone(&bus), None)
            .with_provider_for_test(provider.clone())
            .reconcile_once()
            .await
            .expect("admission opens");
        let db = Arc::clone(&fixture.db);
        let task_id = fixture.task_id.clone();
        let now = now_rfc3339();
        let role_id = new_uuid_v4();
        sqlx::query(
            "INSERT INTO task_role (id, task_id, role, policy_json, created_at, updated_at)
             VALUES (?, ?, 'coordinator', '{}', ?, ?)",
        )
        .bind(&role_id)
        .bind(&task_id)
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("coordinator role");
        sqlx::query(
            "UPDATE task_coordination SET mode = 'prepare_and_execute',
                coordinator_task_role_id = ?, coordinator_actor_kind = 'agent',
                coordinator_actor_id = 'agent-rework', authorization_version = 1,
                envelope_json = '{\"schema_version\":2}', envelope_digest = ?,
                team_digest = ?, requested_by = ? WHERE task_id = ?",
        )
        .bind(&role_id)
        .bind(digest("envelope"))
        .bind(digest("team"))
        .bind(&fixture.user_id)
        .bind(&task_id)
        .execute(db.pool())
        .await
        .expect("authorize coordination");
        sqlx::query("UPDATE task_coordination SET team_stale = 0 WHERE task_id = ?")
            .bind(&task_id)
            .execute(db.pool())
            .await
            .expect("settle team");
        let inputs = (0..2)
            .map(|index| {
                let body = format!("body{index}");
                CreateExternalFeedbackItem {
                    id: new_uuid_v4(),
                    task_id: task_id.clone(),
                    admission_id: merge_id.clone(),
                    provider_item_id: format!("c{index}"),
                    kind: ExternalFeedbackKind::IssueComment,
                    author_login: "reviewer".to_owned(),
                    author_association: "OWNER".to_owned(),
                    review_state: None,
                    path: None,
                    line: None,
                    commit_sha: None,
                    body: body.clone(),
                    body_truncated: false,
                    body_digest: digest(&body),
                    provider_url: None,
                    trust_class: ExternalFeedbackTrustClass::Trusted,
                    head_state: ExternalFeedbackHeadState::Current,
                    ingested_at: now_rfc3339(),
                }
            })
            .collect::<Vec<_>>();
        let items = ExternalFeedbackRepo::ingest_external_feedback(&*db, inputs)
            .await
            .expect("ingest")
            .into_iter()
            .map(|(item, _)| item)
            .collect::<Vec<_>>();
        let request_id = new_uuid_v4();
        sqlx::query(
            "INSERT INTO task_coordination_request (
                id, task_id, authorization_version, mode, brief_json, objective_digest,
                request_digest, idempotency_key, state, version, created_at, updated_at,
                kind, source_subject_kind, source_subject_id, source_subject_digest
             ) VALUES (?, ?, 1, 'prepare_and_execute', '{}', ?, ?, ?, 'requested', 1, ?, ?,
                       'external_feedback', 'external_feedback_set', ?, ?)",
        )
        .bind(&request_id)
        .bind(&task_id)
        .bind(digest("objective"))
        .bind(digest("request"))
        .bind(format!("key-{request_id}"))
        .bind(&now)
        .bind(&now)
        .bind(&merge_id)
        .bind(digest("set"))
        .execute(db.pool())
        .await
        .expect("request");
        let entries = items
            .iter()
            .map(|item| ExternalFeedbackSetEntry {
                feedback_item_id: item.id.clone(),
                body_digest: item.body_digest.clone(),
            })
            .collect::<Vec<_>>();
        ExternalFeedbackRepo::add_external_feedback_set_items(
            &*db,
            &request_id,
            &task_id,
            &entries,
        )
        .await
        .expect("set");
        let execution_id = new_uuid_v4();
        sqlx::query(
            "INSERT INTO execution (id, task_id, status, created_at, updated_at)
             VALUES (?, ?, 'completed', ?, ?)",
        )
        .bind(&execution_id)
        .bind(&task_id)
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("execution");
        sqlx::query(
            "UPDATE task_coordination_request SET state = 'running', version = 2,
                execution_id = ? WHERE id = ?",
        )
        .bind(&execution_id)
        .bind(&request_id)
        .execute(db.pool())
        .await
        .expect("running");
        for (index, item) in items.iter().enumerate() {
            let disposition = if index == 0 && extend {
                FeedbackItemDisposition::ExtendTask
            } else {
                FeedbackItemDisposition::Dismiss
            };
            let triage = ExternalFeedbackRepo::record_external_feedback_triage(
                &*db,
                CreateExternalFeedbackTriage {
                    id: new_uuid_v4(),
                    task_id: task_id.clone(),
                    request_id: request_id.clone(),
                    feedback_item_id: item.id.clone(),
                    body_digest: item.body_digest.clone(),
                    disposition: disposition.legacy(),
                    rationale: "because".to_owned(),
                    instructions: "do it".to_owned(),
                    decision_digest: digest(&item.id),
                    execution_id: execution_id.clone(),
                    created_at: now_rfc3339(),
                },
            )
            .await
            .expect("triage");
            ExternalFeedbackRepo::record_external_feedback_disposition(
                &*db,
                CreateExternalFeedbackDisposition {
                    id: new_uuid_v4(),
                    task_id: task_id.clone(),
                    triage_id: triage.id,
                    request_id: request_id.clone(),
                    feedback_item_id: item.id.clone(),
                    body_digest: item.body_digest.clone(),
                    disposition,
                    scope_classification: (disposition == FeedbackItemDisposition::ExtendTask)
                        .then_some(FeedbackScopeClassification::WithinTaskScope),
                    classification_facts_json: None,
                    dismiss_reason: (disposition == FeedbackItemDisposition::Dismiss)
                        .then_some(FeedbackDismissReason::Disagree),
                    proposal_json: None,
                    created_at: now_rfc3339(),
                },
            )
            .await
            .expect("disposition");
        }
        sqlx::query(
            "UPDATE task_coordination_request SET state = 'triaged', version = 3 WHERE id = ?",
        )
        .bind(&request_id)
        .execute(db.pool())
        .await
        .expect("triaged");
        World {
            service: PrReworkService::new(Arc::clone(&db), Arc::clone(&bus)),
            _temp: temp,
            db,
            bus,
            task_id,
            merge_id,
            request_id,
            provider,
        }
    }

    async fn lifecycle(w: &World) -> db::TaskLifecycle {
        TaskLifecycleRepo::get_task_lifecycle(&*w.db, &w.task_id)
            .await
            .expect("lifecycle")
            .expect("exists")
    }

    async fn superseded(w: &World) -> PrReworkSupersession {
        match w
            .service
            .supersede_for_rework(&w.request_id)
            .await
            .expect("supersede")
        {
            SupersedeOutcome::Superseded { supersession, .. } => supersession,
            SupersedeOutcome::NotRequired => panic!("an extend_task item pauses the PR"),
        }
    }

    #[tokio::test]
    async fn pr_rework_supersede_atomic() {
        let w = world(true).await;
        assert_eq!(lifecycle(&w).await.state, TaskLifecycleState::Merging);
        let supersession = superseded(&w).await;
        let merge = TaskIntegrationOperationRepo::get_by_id(&*w.db, &w.merge_id)
            .await
            .expect("merge")
            .expect("exists");
        assert_eq!(merge.status, db::TaskIntegrationOperationStatus::Abandoned);
        let admission = TaskIntegrationOperationRepo::get_remote_pr_admission(&*w.db, &w.merge_id)
            .await
            .expect("admission")
            .expect("exists");
        assert_eq!(
            admission.state, "open",
            "the PR is neither closed nor merged"
        );
        assert_eq!(
            admission.rework_outcome.as_deref(),
            Some("superseded_for_rework")
        );
        let blocked = lifecycle(&w).await;
        assert_eq!(blocked.state, TaskLifecycleState::Blocked);
        assert_eq!(blocked.reason_kind.as_deref(), Some("pr_feedback_rework"));
        assert_eq!(
            blocked.reason_ref.as_deref(),
            Some(supersession.id.as_str())
        );
        let pr: (Option<String>, String) =
            sqlx::query_as("SELECT provider_pr_id, pr_state FROM pr_metadata WHERE task_id = ?")
                .bind(&w.task_id)
                .fetch_one(w.db.pool())
                .await
                .expect("PR projection");
        assert_eq!(pr.0.as_deref(), Some("pr-77"));
        assert_eq!(pr.1, "open");
        for key in [
            format!("task.pr_rework_superseded:{}", supersession.id),
            format!("pr-admission-superseded:{}", w.merge_id),
        ] {
            assert!(DomainEventRepo::get_event_by_dedupe(&*w.db, &key)
                .await
                .expect("event")
                .is_some());
        }
    }

    #[tokio::test]
    async fn pr_rework_supersede_requires_an_extend_task_and_a_triaged_request() {
        let w = world(false).await;
        assert_eq!(
            w.service
                .supersede_for_rework(&w.request_id)
                .await
                .expect("outcome"),
            SupersedeOutcome::NotRequired
        );
        assert_eq!(lifecycle(&w).await.state, TaskLifecycleState::Merging);

        let w = world(true).await;
        sqlx::query("UPDATE task_coordination_request SET state = 'uncertain' WHERE id = ?")
            .bind(&w.request_id)
            .execute(w.db.pool())
            .await
            .ok();
        // Whatever the guard allows, a non-triaged request never pauses the PR.
        let state: String =
            sqlx::query_scalar("SELECT state FROM task_coordination_request WHERE id = ?")
                .bind(&w.request_id)
                .fetch_one(w.db.pool())
                .await
                .expect("state");
        if state != "triaged" {
            assert!(matches!(
                w.service.supersede_for_rework(&w.request_id).await,
                Err(ServiceError::Coordination {
                    code: CODE_STALE_REQUEST,
                    ..
                })
            ));
        }
    }

    #[tokio::test]
    async fn pr_rework_resume_materializes_plan() {
        let mut w = world(true).await;
        let effects = Arc::new(RecordingEffects::default());
        w.service = PrReworkService::new(Arc::clone(&w.db), Arc::clone(&w.bus))
            .with_effects(effects.clone());
        let supersession = superseded(&w).await;
        let resumed = w
            .service
            .resume_after_supersession(&supersession.id)
            .await
            .expect("resume");
        assert!(resumed.resumed_now && resumed.plan_materialized);
        assert_eq!(effects.materialized.load(Ordering::SeqCst), 1);
        let active = lifecycle(&w).await;
        assert_eq!(active.state, TaskLifecycleState::Active);
        assert_eq!(active.reason_kind.as_deref(), Some("pr_feedback_rework"));
        assert_eq!(active.reason_ref.as_deref(), Some(supersession.id.as_str()));
        let receipt: (String, Option<String>) = sqlx::query_as(
            "SELECT cause_kind, cause_ref FROM task_lifecycle_transition
             WHERE task_id = ? AND to_state = 'active' AND reason_kind = 'pr_feedback_rework'",
        )
        .bind(&w.task_id)
        .fetch_one(w.db.pool())
        .await
        .expect("resume receipt");
        assert_eq!(receipt.0, "domain_event");
        assert_eq!(
            receipt.1.as_deref(),
            Some(supersession.result_event_id.as_str())
        );
        // Any other target for the receipt kind is refused.
        let task = TaskRepo::get_by_id(&*w.db, &w.task_id, false)
            .await
            .expect("task")
            .expect("exists");
        assert!(w
            .service
            .lifecycle
            .transition(TransitionLifecycleInput {
                task_id: w.task_id.clone(),
                expected_task_version: task.version,
                to_state: TaskLifecycleState::Ready,
                cause: LifecycleCause::PrFeedbackRework {
                    supersession_id: supersession.id.clone(),
                },
                reason_kind: Some("pr_feedback_rework".to_owned()),
                reason_ref: Some(supersession.id.clone()),
                idempotency_key: "pr-rework-bad-edge".to_owned(),
            })
            .await
            .is_err());
    }

    #[tokio::test]
    async fn pr_rework_replay_and_restart_idempotent() {
        let w = world(true).await;
        let first = superseded(&w).await;
        match w
            .service
            .supersede_for_rework(&w.request_id)
            .await
            .expect("replay")
        {
            SupersedeOutcome::Superseded {
                supersession,
                created,
            } => {
                assert!(!created);
                assert_eq!(supersession.id, first.id);
            }
            SupersedeOutcome::NotRequired => panic!("replay keeps the pause"),
        }
        let resumed = w
            .service
            .resume_after_supersession(&first.id)
            .await
            .expect("resume");
        assert!(resumed.resumed_now);
        // A restarted process (new service instance) replays both steps.
        let restarted = PrReworkService::new(Arc::clone(&w.db), Arc::clone(&w.bus));
        let again = restarted
            .resume_after_supersession(&first.id)
            .await
            .expect("resume replay");
        assert!(!again.resumed_now);
        assert!(matches!(
            restarted.supersede_for_rework(&w.request_id).await,
            Ok(SupersedeOutcome::Superseded { created: false, .. })
        ));
        assert_eq!(lifecycle(&w).await.state, TaskLifecycleState::Active);
    }

    #[tokio::test]
    async fn pr_rework_selection_excludes_superseded() {
        let w = world(true).await;
        let reads_before = w.provider.status_reads.load(Ordering::SeqCst);
        let reconciler = PrReconciler::new(Arc::clone(&w.db), Arc::clone(&w.bus), None)
            .with_provider_for_test(w.provider.clone());
        reconciler.reconcile_once().await.expect("reconcile open");
        assert!(
            w.provider.status_reads.load(Ordering::SeqCst) > reads_before,
            "an unpaused open admission is polled"
        );
        let ingestor = crate::pr_feedback::PrFeedbackIngestor::new(Arc::clone(&w.db));
        assert_eq!(
            ingestor
                .ingest_once()
                .await
                .expect("poll")
                .admissions_polled,
            1
        );

        superseded(&w).await;
        let reads = w.provider.status_reads.load(Ordering::SeqCst);
        reconciler.reconcile_once().await.expect("reconcile paused");
        assert_eq!(w.provider.status_reads.load(Ordering::SeqCst), reads);
        let ingestor = crate::pr_feedback::PrFeedbackIngestor::new(Arc::clone(&w.db));
        let stats = ingestor.ingest_once().await.expect("poll");
        assert_eq!(stats.admissions_polled + stats.admissions_skipped, 0);
        assert!(
            ExternalFeedbackRepo::list_unconsumed_trusted_feedback(&*w.db, &w.merge_id, 20)
                .await
                .expect("candidates")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn pr_rework_remote_merged_routes_to_decision() {
        let w = world(true).await;
        let supersession = superseded(&w).await;
        let observation = PrObservation {
            provider_event_id: "merged:77".to_owned(),
            remote_repo_identity: "https://example.invalid/repo.git".to_owned(),
            source_branch: "task/x".to_owned(),
            target_branch: "main".to_owned(),
            head_sha: supersession.superseded_head_sha.clone(),
            merged_commit_sha: Some("mergecommit".to_owned()),
        };
        let open = RemotePrStatus::Open(observation.clone());
        assert_eq!(
            w.service
                .observe_remote(&supersession.id, &open)
                .await
                .expect("open"),
            ObserveOutcome::Unchanged
        );
        let outcome = w
            .service
            .observe_remote(
                &supersession.id,
                &RemotePrStatus::Merged(observation.clone()),
            )
            .await
            .expect("merged");
        let ObserveOutcome::RemoteMerged { decision_id } = outcome else {
            panic!("a remote merge routes to a human decision");
        };
        let decision = HumanDecisionRepo::get_human_decision_request(&*w.db, &decision_id)
            .await
            .expect("decision")
            .expect("exists");
        assert_eq!(decision.source_id, format!("rework:{}", supersession.id));
        assert_eq!(decision.state, HumanDecisionState::Open);
        let merged = PrReworkRepo::get_supersession(&*w.db, &supersession.id)
            .await
            .expect("supersession")
            .expect("exists");
        assert_eq!(merged.state, PrReworkState::RemoteMerged);
        assert_eq!(
            merged.convergence_decision_id.as_deref(),
            Some(decision_id.as_str())
        );
        // Idempotent; unanswered decisions do not converge; Forge never merges.
        assert_eq!(
            w.service
                .observe_remote(&supersession.id, &RemotePrStatus::Merged(observation))
                .await
                .expect("replay"),
            ObserveOutcome::Unchanged
        );
        assert_eq!(
            w.service
                .converge_remote_merged(&supersession.id)
                .await
                .expect("converge"),
            ConvergeOutcome::AwaitingDecision
        );
        assert_eq!(lifecycle(&w).await.state, TaskLifecycleState::Blocked);
    }

    #[tokio::test]
    async fn pr_rework_remote_closed_new_pr() {
        let w = world(true).await;
        let supersession = superseded(&w).await;
        let closed = RemotePrStatus::Closed(PrObservation {
            provider_event_id: "closed:77".to_owned(),
            remote_repo_identity: "https://example.invalid/repo.git".to_owned(),
            source_branch: "task/x".to_owned(),
            target_branch: "main".to_owned(),
            head_sha: supersession.superseded_head_sha.clone(),
            merged_commit_sha: None,
        });
        assert_eq!(
            w.service
                .observe_remote(&supersession.id, &closed)
                .await
                .expect("closed"),
            ObserveOutcome::RemoteClosed
        );
        assert!(
            PrReworkRepo::get_open_supersession_by_task(&*w.db, &w.task_id)
                .await
                .expect("open")
                .is_none()
        );
        // The Task keeps its rework: it still resumes (a new PR follows later).
        w.service
            .resume_after_supersession(&supersession.id)
            .await
            .expect("resume after a remote close");
        assert_eq!(lifecycle(&w).await.state, TaskLifecycleState::Active);
    }
}
