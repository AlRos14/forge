//! Authorized external effects on the admitted remote PR (plan node P3,
//! «Congelado 7» and «Congelado 11» 11.1/11.5): `push_update` and
//! `post_comment`.
//!
//! Authority (D5): an action is born `approved` only when the policy FROZEN in
//! its admission says `automatic`; otherwise it is `proposed`, a collaborative
//! Proposal targets it and only an exact human Decision approves or rejects it.
//! Forge never merges.
//!
//! Effects are idempotent and recoverable:
//! - `push_update` fast-forwards the PR branch to the exact admitted SHA with a
//!   compare-and-swap on the remote head (`--force-with-lease=<ref>:<expected>`,
//!   never a plain force), after reading the PR head. The token travels only in
//!   the child process environment as an `http.extraheader`; it is never in a
//!   URL, an argument, a file, a log or an error.
//! - `post_comment` searches the PR conversation for the Artifact's anti-loop
//!   marker before posting, which also resolves `outcome_unknown`.
//! - A crash leaves the action `running`; the executor tick treats it as
//!   `outcome_unknown` and re-reads the provider state before repeating
//!   anything. An unresolved unknown ends in `reconciliation_required` (human).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use db::{
    new_uuid_v4, now_rfc3339, CollaborationRepo, CreateDomainEvent, CreatePrExternalAction,
    DbError, DecisionOutcome, DomainEvent, GateRepo, PrExternalAction, PrExternalActionKind,
    PrExternalActionRepo, PrExternalActionState, PrFeedbackAuthorization, PrFeedbackPolicy,
    PrFeedbackSummaryMode, PrMetadataRepo, PrReworkRepo, PrReworkState, ProposalTarget,
    ProposalTargetKind, RemotePrAdmission, SqliteDb, TaskIntegrationOperationRepo, TaskRepo,
    TransitionPrExternalAction, WorkspaceRepo,
};
use events::EventBus;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::process::Command;

use crate::collaboration_service::{
    CollaborationActorSource, CollaborationService, CreateDecisionInput, CreateProposalInput,
};
use crate::credential_service::Secret;
use crate::domain_event_service::DomainEventService;
use crate::pr_feedback::forge_comment_marker;
use crate::pr_github_client::{GitHubClient, GitHubClientError, GitHubRepoRef};
use crate::pr_service::{PrService, RemotePrAdmissionKey};
use crate::task_integration_operation::TaskIntegrationOperationManager;
use crate::{Result, ServiceError};

pub const CODE_NOT_AUTHORIZED: &str = "pr_action.not_authorized";
pub const CODE_REMOTE_HEAD_CHANGED: &str = "pr_action.remote_head_changed";
pub const CODE_PROVIDER_REJECTED: &str = "pr_action.provider_rejected";
pub const CODE_OUTCOME_UNKNOWN_EXPIRED: &str = "pr_action.outcome_unknown_expired";
pub const CODE_NOT_FAST_FORWARD: &str = crate::pr_rework::CODE_NOT_FAST_FORWARD;
pub const CODE_REUSE_TARGET_MISSING: &str = crate::pr_rework::CODE_REUSE_TARGET_MISSING;
const CODE_REVIEW_COVERAGE_STALE: &str = "review.coverage.facts_stale";

/// An `outcome_unknown` that stays unresolved this long needs a human.
const RECOVERY_WINDOW_SECONDS: i64 = 3600;
const MAX_COMMENT_SCAN_PAGES: u32 = 10;
const PUSH_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_SUMMARY_BYTES: usize = 16 * 1024;
const MAX_STORED_ERROR_CHARS: usize = 300;
const EXECUTOR_BATCH: i64 = 100;

/// One executor at a time per process: the CAS is the cross-process guard,
/// this only prevents two ticks from racing on the same action.
static EXECUTOR_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn action_error(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::Coordination {
        code,
        message: message.into(),
    }
}

fn map_db(error: DbError) -> ServiceError {
    match error {
        DbError::PrActionNotAuthorized => action_error(
            CODE_NOT_AUTHORIZED,
            "the exact admission, frozen policy, Gate evaluation or Decision does not authorize this action",
        ),
        DbError::VersionConflict => ServiceError::Db(DbError::VersionConflict),
        other => other.into(),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrExternalActionStats {
    /// Missing re-admission `push_update` actions created this pass.
    pub created: usize,
    /// Actions examined by the executor (any outcome).
    pub advanced: usize,
    /// Re-admissions completed (`open` + `readmitted`) after a pushed update.
    pub finalized: usize,
}

#[derive(Clone)]
pub struct PrExternalActionService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    pr: PrService,
    collaboration: CollaborationService,
    events: DomainEventService,
    /// Test seam: `(source directory, remote URL)` of the push.
    #[cfg(test)]
    push_source_override: Option<(PathBuf, String)>,
}

impl PrExternalActionService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>, pr: PrService) -> Self {
        Self {
            collaboration: CollaborationService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            events: DomainEventService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            db,
            event_bus,
            pr,
            #[cfg(test)]
            push_source_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_push_source_for_test(mut self, path: PathBuf, url: String) -> Self {
        self.push_source_override = Some((path, url));
        self
    }

    // ------------------------------------------------------------------
    // Creation
    // ------------------------------------------------------------------

    /// The exact `push_update` of a re-admission (plan 11.5). Idempotent per
    /// admission. Authorization follows the policy frozen in the admission.
    pub async fn ensure_push_update(&self, admission_id: &str) -> Result<PrExternalAction> {
        let admission = self.load_admission(admission_id).await?;
        let supersession_id = admission.readmits_supersession_id.clone().ok_or_else(|| {
            ServiceError::invalid_operation("admission does not re-admit a paused PR")
        })?;
        let supersession = PrReworkRepo::get_supersession(&*self.db, &supersession_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("pr rework supersession", supersession_id.clone())
            })?;
        if supersession.state != PrReworkState::Open {
            return Err(action_error(
                crate::pr_rework::CODE_SUPERSESSION_OPEN,
                "the paused PR no longer awaits a push_update",
            ));
        }
        let policy = frozen_policy(&admission)?;
        let mode = policy.push_corrections;
        let merge =
            TaskIntegrationOperationRepo::get_by_id(&*self.db, &admission.task_merge_operation_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found(
                        "TaskMerge admission",
                        admission.task_merge_operation_id.clone(),
                    )
                })?;
        let gate_evaluation_id = merge.gate_evaluation_id.clone().ok_or_else(|| {
            ServiceError::invalid_operation("the re-admission TaskMerge has no frozen Gate")
        })?;
        let key = format!("push_update:{admission_id}");
        if PrExternalActionRepo::get_pr_external_action_by_idempotency_key(&*self.db, &key)
            .await?
            .is_none()
        {
            self.make_room_for(&admission.task_id).await?;
        }
        let write = self
            .create(CreatePrExternalAction {
                id: new_uuid_v4(),
                task_id: admission.task_id.clone(),
                admission_id: admission.task_merge_operation_id.clone(),
                kind: PrExternalActionKind::PushUpdate,
                authorization_mode: mode,
                gate_evaluation_id: Some(gate_evaluation_id),
                subject_commit_sha: Some(admission.admitted_source_sha.clone()),
                expected_remote_head_sha: Some(supersession.superseded_head_sha.clone()),
                summary_artifact_id: None,
                body_digest: None,
                idempotency_key: key,
                created_at: now_rfc3339(),
            })
            .await?;
        self.after_create(write).await
    }

    /// The `post_comment` of a triage summary Artifact on the PR (plan 11.1).
    /// `Ok(None)` when the frozen policy says `post_summary = off`.
    pub async fn request_post_comment(
        &self,
        admission_id: &str,
        summary_artifact_id: &str,
    ) -> Result<Option<PrExternalAction>> {
        let admission = self.load_admission(admission_id).await?;
        let policy = frozen_policy(&admission)?;
        let mode = match policy.post_summary {
            PrFeedbackSummaryMode::Off => return Ok(None),
            PrFeedbackSummaryMode::HumanApproval => PrFeedbackAuthorization::HumanApproval,
            PrFeedbackSummaryMode::Automatic => PrFeedbackAuthorization::Automatic,
        };
        self.pull_number_for_admission(&admission).await?;
        let artifact = CollaborationRepo::get_artifact(&*self.db, summary_artifact_id)
            .await?
            .filter(|artifact| artifact.task_id == admission.task_id)
            .ok_or_else(|| ServiceError::not_found("artifact", summary_artifact_id.to_owned()))?;
        let content = artifact.content.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("the summary Artifact has no inline content")
        })?;
        let digest = verify_summary(&artifact.id, content)?;
        let key = format!("post_comment:{admission_id}:{}", artifact.id);
        if PrExternalActionRepo::get_pr_external_action_by_idempotency_key(&*self.db, &key)
            .await?
            .is_none()
        {
            self.make_room_for(&admission.task_id).await?;
        }
        let write = self
            .create(CreatePrExternalAction {
                id: new_uuid_v4(),
                task_id: admission.task_id.clone(),
                admission_id: admission.task_merge_operation_id.clone(),
                kind: PrExternalActionKind::PostComment,
                authorization_mode: mode,
                gate_evaluation_id: None,
                subject_commit_sha: None,
                expected_remote_head_sha: None,
                summary_artifact_id: Some(artifact.id.clone()),
                body_digest: Some(digest),
                idempotency_key: key,
                created_at: now_rfc3339(),
            })
            .await?;
        self.after_create(write).await.map(Some)
    }

    async fn create(&self, input: CreatePrExternalAction) -> Result<PrExternalAction> {
        let admission = self.load_admission(&input.admission_id).await?;
        let key = RemotePrAdmissionKey::from_admission(&admission)?;
        if key.task_id != input.task_id {
            return Err(action_error(
                CODE_NOT_AUTHORIZED,
                "the external action does not match its exact Task admission",
            ));
        }
        let event = action_event_parts(
            &input.id,
            &input.task_id,
            &input.admission_id,
            &key.repo_id,
            input.kind,
            "pr.external_action_created",
            if input.authorization_mode == PrFeedbackAuthorization::Automatic {
                "approved"
            } else {
                "proposed"
            },
        );
        let write = PrExternalActionRepo::create_pr_external_action(&*self.db, input, Some(event))
            .await
            .map_err(map_db)?;
        if let Some(event) = write.event.as_ref() {
            self.publish(event);
        }
        Ok(write.action)
    }

    /// A Task has at most one open action (V173). A not yet started
    /// `post_comment` is lower priority than the push that unblocks the PR
    /// and yields; anything running must finish first.
    async fn make_room_for(&self, task_id: &str) -> Result<()> {
        let Some(open) =
            PrExternalActionRepo::get_open_pr_external_action(&*self.db, task_id).await?
        else {
            return Ok(());
        };
        if open.kind == PrExternalActionKind::PostComment
            && matches!(
                open.state,
                PrExternalActionState::Proposed | PrExternalActionState::Approved
            )
        {
            self.transition(&open, PrExternalActionState::Abandoned, None, None, None)
                .await?;
            return Ok(());
        }
        Err(ServiceError::conflict(
            "another PR external action is still open for this Task",
        ))
    }

    async fn after_create(&self, action: PrExternalAction) -> Result<PrExternalAction> {
        if action.state == PrExternalActionState::Proposed && action.proposal_id.is_none() {
            return self.ensure_proposal(&action).await;
        }
        Ok(action)
    }

    /// Attach the collaborative Proposal that a human Decision resolves.
    async fn ensure_proposal(&self, action: &PrExternalAction) -> Result<PrExternalAction> {
        if action.proposal_id.is_some() || action.state != PrExternalActionState::Proposed {
            return Ok(action.clone());
        }
        let admission = self.load_admission(&action.admission_id).await?;
        // The frozen credential owner is the owner/admin on whose behalf
        // Forge would act; they are the proposer of record.
        let proposer = admission
            .credential_owner_user_id
            .clone()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ServiceError::invalid_operation("the admission has no frozen credential owner")
            })?;
        let reason = match action.kind {
            PrExternalActionKind::PushUpdate => format!(
                "Authorize Forge to fast-forward the pull request branch {} to the exact admitted head {}.",
                admission.source_branch, admission.admitted_source_sha
            ),
            PrExternalActionKind::PostComment => {
                "Authorize Forge to post the deterministic feedback triage summary on the pull request."
                    .to_owned()
            }
        };
        let proposal = self
            .collaboration
            .create_proposal_with_id(
                CollaborationActorSource::Human(proposer),
                CreateProposalInput {
                    task_id: action.task_id.clone(),
                    target: ProposalTarget {
                        kind: ProposalTargetKind::PrExternalAction,
                        id: action.id.clone(),
                    },
                    action: action.kind.to_string(),
                    reason,
                    target_version: None,
                    target_digest: action
                        .body_digest
                        .clone()
                        .or_else(|| action.subject_commit_sha.clone()),
                    required_policy_ref: None,
                    required_policy_version: None,
                    required_policy_digest: None,
                    supersedes_proposal_id: None,
                    artifact_ids: action.summary_artifact_id.iter().cloned().collect(),
                },
                format!("{}-proposal", action.id),
            )
            .await?;
        PrExternalActionRepo::attach_pr_external_action_proposal(
            &*self.db,
            &action.id,
            action.version,
            &proposal.id,
            &now_rfc3339(),
        )
        .await
        .map_err(map_db)
    }

    // ------------------------------------------------------------------
    // Human decision (D5)
    // ------------------------------------------------------------------

    /// Approve a `human_approval` action. Requires Project owner/admin; the
    /// Decision is the exact one on the action's Proposal. Idempotent.
    pub async fn approve(
        &self,
        action_id: &str,
        actor_user_id: &str,
        expected_version: i64,
        rationale: Option<String>,
    ) -> Result<PrExternalAction> {
        self.decide(
            action_id,
            actor_user_id,
            expected_version,
            DecisionOutcome::Approve,
            rationale,
        )
        .await
    }

    /// Reject a `human_approval` action. A rejected `push_update` fails the
    /// re-admission (`publication_failed`), which takes the existing
    /// merge-failure rework path.
    pub async fn reject(
        &self,
        action_id: &str,
        actor_user_id: &str,
        expected_version: i64,
        rationale: Option<String>,
    ) -> Result<PrExternalAction> {
        self.decide(
            action_id,
            actor_user_id,
            expected_version,
            DecisionOutcome::Reject,
            rationale,
        )
        .await
    }

    async fn decide(
        &self,
        action_id: &str,
        actor_user_id: &str,
        expected_version: i64,
        outcome: DecisionOutcome,
        rationale: Option<String>,
    ) -> Result<PrExternalAction> {
        let target_state = match outcome {
            DecisionOutcome::Approve => PrExternalActionState::Approved,
            DecisionOutcome::Reject => PrExternalActionState::Rejected,
            DecisionOutcome::Supersede => {
                return Err(ServiceError::invalid_operation(
                    "a PR external action is approved or rejected, never superseded",
                ))
            }
        };
        let action = PrExternalActionRepo::get_pr_external_action(&*self.db, action_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("pr external action", action_id.to_owned()))?;
        if action.version != expected_version {
            return Err(DbError::VersionConflict.into());
        }
        let task = TaskRepo::get_by_id(&*self.db, &action.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", action.task_id.clone()))?;
        let project = db::ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", action.task_id.clone()))?;
        if !crate::project_authority::user_can_administer_project(&self.db, &project, actor_user_id)
            .await?
        {
            return Err(ServiceError::AuthorizationDenied {
                message: "Project owner or admin role is required".to_owned(),
            });
        }
        if action.state == target_state {
            return Ok(action);
        }
        if action.state != PrExternalActionState::Proposed
            || action.authorization_mode != PrFeedbackAuthorization::HumanApproval
        {
            return Err(ServiceError::conflict(
                "only a proposed human_approval PR external action can be decided",
            ));
        }
        let action = self.ensure_proposal(&action).await?;
        // Proposal repair is itself versioned. A legacy action without its
        // Proposal must be reloaded before the caller can decide it, so the
        // final action UPDATE uses exactly the version the caller supplied.
        if action.version != expected_version {
            return Err(DbError::VersionConflict.into());
        }
        let proposal_id = action.proposal_id.clone().ok_or_else(|| {
            ServiceError::invalid_operation("the action has no Proposal to decide")
        })?;
        let existing = sqlx::query("SELECT id, outcome FROM decision WHERE proposal_id = ?")
            .bind(&proposal_id)
            .fetch_optional(self.db.pool())
            .await
            .map_err(DbError::from)?;
        let decision_id = match existing {
            Some(row) => {
                let recorded: String = row.try_get("outcome").map_err(DbError::from)?;
                if recorded != outcome.to_string() {
                    return Err(ServiceError::conflict(
                        "the Proposal of this action already has a different Decision",
                    ));
                }
                row.try_get::<String, _>("id").map_err(DbError::from)?
            }
            None => {
                let proposal = CollaborationRepo::get_proposal(&*self.db, &proposal_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("proposal", proposal_id.clone()))?;
                self.collaboration
                    .record_decision(
                        CreateDecisionInput {
                            task_id: action.task_id.clone(),
                            proposal_id: proposal_id.clone(),
                            proposal_version: proposal.content_version,
                            outcome,
                            rationale: rationale
                                .filter(|value| !value.trim().is_empty())
                                .unwrap_or_else(|| format!("PR external action {}", target_state)),
                            policy_ref: None,
                            policy_version: None,
                            policy_digest: None,
                        },
                        vec![CollaborationActorSource::Human(actor_user_id.to_owned())],
                    )
                    .await?
                    .id
            }
        };
        let decided = self
            .transition(&action, target_state, Some(decision_id), None, None)
            .await?;
        if target_state == PrExternalActionState::Rejected {
            self.fail_admission(&decided, "pr_action.rejected").await?;
        }
        Ok(decided)
    }

    // ------------------------------------------------------------------
    // Executor
    // ------------------------------------------------------------------

    /// One executor pass: create missing re-admission pushes, execute approved
    /// actions, recover `running` / `outcome_unknown` ones and finish pushed
    /// re-admissions. Never fails the caller's tick for one bad action.
    pub async fn run_once(&self) -> Result<PrExternalActionStats> {
        let _guard = EXECUTOR_LOCK.lock().await;
        let mut stats = PrExternalActionStats::default();
        for admission_id in self.readmissions_without_push().await? {
            match self.ensure_push_update(&admission_id).await {
                Ok(_) => stats.created += 1,
                Err(error) => {
                    tracing::warn!(%error, "could not create the re-admission push_update");
                }
            }
        }
        let actions = PrExternalActionRepo::list_pr_external_actions_in_states(
            &*self.db,
            &[
                PrExternalActionState::Proposed,
                PrExternalActionState::Approved,
                PrExternalActionState::Running,
                PrExternalActionState::OutcomeUnknown,
            ],
            EXECUTOR_BATCH,
        )
        .await?;
        for action in actions {
            stats.advanced += 1;
            if let Err(error) = self.advance(action).await {
                tracing::warn!(%error, "PR external action pass failed");
            }
        }
        for admission_id in self.pushed_awaiting_finalization().await? {
            let result = async {
                let admission = self.load_admission(&admission_id).await?;
                self.pr.finalize_readmission(&admission).await
            }
            .await;
            match result {
                Ok(()) => stats.finalized += 1,
                Err(error) => tracing::warn!(%error, "could not finalize the PR re-admission"),
            }
        }
        Ok(stats)
    }

    async fn readmissions_without_push(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT a.task_merge_operation_id FROM remote_pr_admission a
             WHERE a.state = 'admitted' AND a.readmits_supersession_id IS NOT NULL
               AND a.rework_outcome IS NULL
               AND NOT EXISTS (SELECT 1 FROM pr_external_action x
                               WHERE x.admission_id = a.task_merge_operation_id
                                 AND x.kind = 'push_update')
             ORDER BY a.created_at LIMIT 50",
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(DbError::from)?)
    }

    async fn pushed_awaiting_finalization(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT x.admission_id FROM pr_external_action x
             JOIN remote_pr_admission a ON a.task_merge_operation_id = x.admission_id
             WHERE x.kind = 'push_update' AND x.state = 'succeeded'
               AND a.readmits_supersession_id IS NOT NULL AND a.rework_outcome IS NULL
               AND a.state IN ('admitted', 'open')
               AND EXISTS (SELECT 1 FROM pr_rework_supersession s
                           WHERE s.id = a.readmits_supersession_id AND s.state = 'open')
             LIMIT 50",
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(DbError::from)?)
    }

    async fn advance(&self, action: PrExternalAction) -> Result<()> {
        match action.state {
            PrExternalActionState::Proposed => {
                let admission = self.load_admission(&action.admission_id).await?;
                if !self.relevant(&action, &admission).await? {
                    self.transition(&action, PrExternalActionState::Abandoned, None, None, None)
                        .await?;
                } else if action.proposal_id.is_none() {
                    self.ensure_proposal(&action).await?;
                }
                Ok(())
            }
            PrExternalActionState::Approved
            | PrExternalActionState::Running
            | PrExternalActionState::OutcomeUnknown => self.execute(action).await,
            _ => Ok(()),
        }
    }

    /// Execute (or recover) one action. Public for the API lane's "run now"
    /// and for tests; the tick uses it too.
    pub async fn execute(&self, action: PrExternalAction) -> Result<()> {
        let admission = self.load_admission(&action.admission_id).await?;
        if matches!(
            action.state,
            PrExternalActionState::Proposed | PrExternalActionState::Approved
        ) && !self.relevant(&action, &admission).await?
        {
            self.transition(&action, PrExternalActionState::Abandoned, None, None, None)
                .await?;
            return Ok(());
        }
        match action.kind {
            PrExternalActionKind::PushUpdate => self.execute_push(action, admission).await,
            PrExternalActionKind::PostComment => self.execute_comment(action, admission).await,
        }
    }

    /// Is the admission still in the shape the action was authorized for?
    async fn relevant(
        &self,
        action: &PrExternalAction,
        admission: &RemotePrAdmission,
    ) -> Result<bool> {
        match action.kind {
            PrExternalActionKind::PostComment => {
                if admission.state != "open" {
                    return Ok(false);
                }
                Ok(self.pull_number_for_admission(admission).await.is_ok())
            }
            PrExternalActionKind::PushUpdate => {
                let Some(supersession_id) = admission.readmits_supersession_id.as_deref() else {
                    return Ok(false);
                };
                let open = PrReworkRepo::get_supersession(&*self.db, supersession_id)
                    .await?
                    .is_some_and(|supersession| supersession.state == PrReworkState::Open);
                Ok(admission.state == "admitted" && admission.rework_outcome.is_none() && open)
            }
        }
    }

    // ---- push_update ---------------------------------------------------

    async fn execute_push(
        &self,
        action: PrExternalAction,
        admission: RemotePrAdmission,
    ) -> Result<()> {
        let supersession_id = admission.readmits_supersession_id.clone().ok_or_else(|| {
            action_error(CODE_NOT_AUTHORIZED, "the admission re-admits no paused PR")
        })?;
        let supersession = PrReworkRepo::get_supersession(&*self.db, &supersession_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("pr rework supersession", supersession_id.clone())
            })?;
        let (Some(subject), Some(expected)) = (
            action.subject_commit_sha.clone(),
            action.expected_remote_head_sha.clone(),
        ) else {
            return Err(action_error(CODE_NOT_AUTHORIZED, "the action pins no SHAs"));
        };
        if subject != admission.admitted_source_sha || expected != supersession.superseded_head_sha
        {
            return Err(action_error(
                CODE_NOT_AUTHORIZED,
                "the action no longer matches the admitted head or the paused head",
            ));
        }
        let number = supersession.provider_pr_id.parse::<u64>().map_err(|_| {
            ServiceError::invalid_operation("the paused PR has no numeric provider id")
        })?;
        let repo_ref = GitHubRepoRef::parse(&admission.remote_repo_identity)?;

        // 1. Read the remote head. A failed read has had no effect.
        let pull = match self.pr.client_for(&admission).await {
            Ok(client) => client
                .get_pull(&repo_ref, number)
                .await
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        let pull = match pull {
            Ok(pull) => pull,
            Err(reason) => return self.defer(&action, &reason).await,
        };
        if pull.state != "open" || pull.merged {
            return self
                .reconcile_push(
                    action,
                    &admission,
                    CODE_REUSE_TARGET_MISSING,
                    "the paused PR is no longer open",
                )
                .await;
        }
        // 2. Already there (a crash after the push, or a retry): success.
        if pull.head_sha == subject {
            let running = self.to_running(action).await?;
            let done = self
                .transition(
                    &running,
                    PrExternalActionState::Succeeded,
                    None,
                    Some(format!("github-pr:{number}:open:{subject}")),
                    Some(json!({"outcome": "already_at_subject", "head_sha": subject})),
                )
                .await?;
            return self.after_push_success(&done, &admission).await;
        }
        // 3. The remote moved: never force.
        if pull.head_sha != expected {
            return self
                .reconcile_push(
                    action,
                    &admission,
                    CODE_REMOTE_HEAD_CHANGED,
                    "the remote PR head is neither the paused head nor the admitted head",
                )
                .await;
        }
        if action.state == PrExternalActionState::OutcomeUnknown && expired(&action) {
            return self
                .reconcile_push(
                    action,
                    &admission,
                    CODE_OUTCOME_UNKNOWN_EXPIRED,
                    "the push outcome stayed unresolved past the recovery window",
                )
                .await;
        }
        // 4. Fast-forward only (local ancestry; the remote CAS is the guard).
        let (dir, url) = self.push_target(&admission).await?;
        if !matches!(git::is_ancestor(&dir, &expected, &subject).await, Ok(true)) {
            let running = self.to_running(action).await?;
            let failed = self
                .transition(
                    &running,
                    PrExternalActionState::Failed,
                    None,
                    None,
                    Some(json!({"code": CODE_NOT_FAST_FORWARD})),
                )
                .await?;
            return self.fail_admission(&failed, CODE_NOT_FAST_FORWARD).await;
        }
        let token = match self.pr.secret_for(&admission).await {
            Ok(secret) => Some(secret),
            Err(error) => return self.defer(&action, &error.to_string()).await,
        };
        let lock_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let integration_operations =
            TaskIntegrationOperationManager::new(Arc::clone(&self.db), lock_root);
        let Some(_integration_claim) = integration_operations
            .try_pr_recovery_lock(
                &admission.task_id,
                &admission.task_merge_operation_id,
                &admission.publish_operation_id,
            )
            .await?
        else {
            return self
                .defer(
                    &action,
                    "the exact Task PR integration claim is busy or no longer current",
                )
                .await;
        };
        if let Err(error) = self.ensure_review_coverage_before_push(&admission).await {
            return self
                .reconcile_push(
                    action,
                    &admission,
                    CODE_REVIEW_COVERAGE_STALE,
                    &error.to_string(),
                )
                .await;
        }
        let running = self.to_running(action).await?;
        match git_push_cas(
            &dir,
            &url,
            &admission.source_branch,
            &expected,
            &subject,
            token.as_ref(),
        )
        .await
        {
            Ok(()) => {
                let done = self
                    .transition(
                        &running,
                        PrExternalActionState::Succeeded,
                        None,
                        Some(format!("github-pr:{number}:open:{subject}")),
                        Some(json!({"outcome": "pushed", "head_sha": subject})),
                    )
                    .await?;
                self.after_push_success(&done, &admission).await
            }
            Err(failure) if failure.definitive => {
                let failed = self
                    .transition(
                        &running,
                        PrExternalActionState::Failed,
                        None,
                        None,
                        Some(json!({"code": CODE_PROVIDER_REJECTED, "message": failure.message})),
                    )
                    .await?;
                self.fail_admission(&failed, CODE_PROVIDER_REJECTED).await
            }
            Err(failure) => {
                // The remote may or may not have taken the update; the next
                // pass re-reads the head before anything is repeated.
                self.transition(
                    &running,
                    PrExternalActionState::OutcomeUnknown,
                    None,
                    None,
                    Some(json!({"message": failure.message})),
                )
                .await?;
                Ok(())
            }
        }
    }

    /// The local directory holding the admitted commit and the remote URL
    /// (the frozen repository identity) to push to.
    async fn push_target(&self, admission: &RemotePrAdmission) -> Result<(PathBuf, String)> {
        #[cfg(test)]
        if let Some(target) = &self.push_source_override {
            return Ok(target.clone());
        }
        let key = RemotePrAdmissionKey::from_admission(admission)?;
        let workspace =
            WorkspaceRepo::get_integration_by_task_repo(&*self.db, &key.task_id, &key.repo_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "the admitted repository has no integration workspace to push from",
                    )
                })?;
        if workspace.repo_id != key.repo_id {
            return Err(ServiceError::invalid_operation(
                "the integration workspace does not match the admitted repository",
            ));
        }
        Ok((
            PathBuf::from(workspace.worktree_path),
            admission.remote_repo_identity.clone(),
        ))
    }

    async fn ensure_review_coverage_before_push(
        &self,
        admission: &RemotePrAdmission,
    ) -> Result<()> {
        let merge =
            TaskIntegrationOperationRepo::get_by_id(&*self.db, &admission.task_merge_operation_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found(
                        "TaskMerge operation",
                        admission.task_merge_operation_id.clone(),
                    )
                })?;
        let evaluation_id = merge.gate_evaluation_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("PR rework admission has no GateEvaluation")
        })?;
        let evaluation = GateRepo::get_gate_evaluation(&*self.db, evaluation_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("GateEvaluation", evaluation_id.to_owned()))?;
        let gate = GateRepo::get_gate(&*self.db, &evaluation.gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("Gate", evaluation.gate_id.clone()))?;
        let active_revision = gate.active_policy_revision.ok_or_else(|| {
            ServiceError::invalid_operation("merge-readiness Gate has no active policy")
        })?;
        let active_policy =
            GateRepo::get_gate_policy_revision(&*self.db, &gate.id, active_revision)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found(
                        "gate policy revision",
                        format!("{}:{active_revision}", gate.id),
                    )
                })?;
        let workspace_root = if active_policy.schema_version == 4 {
            crate::gate_engine::infer_review_coverage_workspace_root(&self.db, &admission.task_id)
                .await?
        } else {
            PathBuf::from(".")
        };
        crate::merge_service::MergeService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
            workspace_root,
        )
        .ensure_current_gate_evaluation_for_pr_rework_push(&admission.task_id, evaluation_id)
        .await
    }

    async fn after_push_success(
        &self,
        _action: &PrExternalAction,
        admission: &RemotePrAdmission,
    ) -> Result<()> {
        // The action is terminal; a failure here is retried by the
        // "pushed awaiting finalization" scan of the next pass.
        if let Err(error) = self.pr.finalize_readmission(admission).await {
            tracing::warn!(%error, "could not finalize the PR re-admission after the push");
        }
        Ok(())
    }

    // ---- post_comment --------------------------------------------------

    async fn execute_comment(
        &self,
        action: PrExternalAction,
        admission: RemotePrAdmission,
    ) -> Result<()> {
        let (Some(artifact_id), Some(digest)) = (
            action.summary_artifact_id.clone(),
            action.body_digest.clone(),
        ) else {
            return Err(action_error(
                CODE_NOT_AUTHORIZED,
                "the action pins no summary Artifact",
            ));
        };
        let artifact = CollaborationRepo::get_artifact(&*self.db, &artifact_id)
            .await?
            .filter(|artifact| artifact.task_id == action.task_id)
            .ok_or_else(|| ServiceError::not_found("artifact", artifact_id.clone()))?;
        let content = artifact.content.clone().ok_or_else(|| {
            ServiceError::invalid_operation("the summary Artifact has no inline content")
        })?;
        if verify_summary(&artifact.id, &content)? != digest {
            return Err(action_error(
                CODE_NOT_AUTHORIZED,
                "the summary Artifact no longer matches the authorized digest",
            ));
        }
        let number = self.pull_number_for_admission(&admission).await?;
        let repo_ref = GitHubRepoRef::parse(&admission.remote_repo_identity)?;
        let client = match self.pr.client_for(&admission).await {
            Ok(client) => client,
            Err(error) => return self.defer(&action, &error.to_string()).await,
        };
        let marker = forge_comment_marker(&artifact.id, &digest);
        let found = match find_marker(&client, &repo_ref, number, &marker).await {
            Ok(found) => found,
            Err(reason) => return self.defer(&action, &reason).await,
        };
        let running = self.to_running(action).await?;
        if let Some(comment_id) = found {
            self.transition(
                &running,
                PrExternalActionState::Succeeded,
                None,
                Some(format!("github-comment:{comment_id}")),
                Some(json!({"outcome": "already_posted", "comment_id": comment_id})),
            )
            .await?;
            return Ok(());
        }
        match client
            .create_issue_comment(&repo_ref, number, &content)
            .await
        {
            Ok(comment) => {
                self.transition(
                    &running,
                    PrExternalActionState::Succeeded,
                    None,
                    Some(format!("github-comment:{}", comment.id)),
                    Some(json!({"outcome": "posted", "comment_id": comment.id})),
                )
                .await?;
                Ok(())
            }
            Err(
                error @ (GitHubClientError::Rejected { .. }
                | GitHubClientError::NotFound
                | GitHubClientError::Unauthorized { .. }
                | GitHubClientError::InvalidConfig(_)),
            ) => {
                self.transition(
                    &running,
                    PrExternalActionState::Failed,
                    None,
                    None,
                    Some(json!({
                        "code": CODE_PROVIDER_REJECTED,
                        "message": truncate(&error.to_string()),
                    })),
                )
                .await?;
                Ok(())
            }
            Err(error) => self.defer(&running, &error.to_string()).await,
        }
    }

    // ---- shared helpers -----------------------------------------------

    async fn load_admission(&self, admission_id: &str) -> Result<RemotePrAdmission> {
        let admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*self.db, admission_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("remote PR admission", admission_id.to_owned())
                })?;
        let key = RemotePrAdmissionKey::from_admission(&admission)?;
        if key.admission_id != admission_id {
            return Err(action_error(
                CODE_NOT_AUTHORIZED,
                "the remote admission key does not match the requested admission",
            ));
        }
        Ok(admission)
    }

    /// Resolve the PR number pinned by this admission. A superseded admission
    /// uses its immutable supersession receipt; a terminal historical
    /// admission uses its immutable remote history. Never let the mutable
    /// `pr_metadata` projection redirect a queued comment to a later PR.
    async fn pull_number_for_admission(&self, admission: &RemotePrAdmission) -> Result<u64> {
        let key = RemotePrAdmissionKey::from_admission(admission)?;
        let provider_pr_id = if let Some(supersession_id) = admission.supersession_id.as_deref() {
            PrReworkRepo::get_supersession(&*self.db, supersession_id)
                .await?
                .filter(|supersession| {
                    supersession.task_id == admission.task_id
                        && supersession.task_merge_operation_id == admission.task_merge_operation_id
                })
                .map(|supersession| supersession.provider_pr_id)
        } else {
            let metadata =
                PrMetadataRepo::get_by_task_repo(&*self.db, &key.task_id, &key.repo_id).await?;
            match metadata.filter(|metadata| {
                metadata.id == admission.metadata_id
                    && metadata.repo_id.as_deref() == Some(key.repo_id.as_str())
                    && metadata.task_merge_operation_id.as_deref()
                        == Some(key.admission_id.as_str())
            }) {
                Some(metadata) => metadata.provider_pr_id,
                None => sqlx::query_scalar::<_, Option<String>>(
                    "SELECT provider_pr_id FROM remote_pr_history
                         WHERE task_merge_operation_id = ?",
                )
                .bind(&admission.task_merge_operation_id)
                .fetch_optional(self.db.pool())
                .await
                .map_err(DbError::from)?
                .flatten(),
            }
        };
        provider_pr_id
            .as_deref()
            .and_then(|id| id.parse::<u64>().ok())
            .ok_or_else(|| ServiceError::invalid_operation("the admission has no numeric PR id"))
    }

    /// `approved -> running` (a recovered `running` / `outcome_unknown` action
    /// stays as it is).
    async fn to_running(&self, action: PrExternalAction) -> Result<PrExternalAction> {
        if action.state == PrExternalActionState::Approved {
            self.transition(&action, PrExternalActionState::Running, None, None, None)
                .await
        } else {
            Ok(action)
        }
    }

    /// A read failed or the effect may not have happened: an `approved` action
    /// simply waits; a started one becomes (or stays) `outcome_unknown`, and
    /// an unresolved unknown past the window needs a human.
    async fn defer(&self, action: &PrExternalAction, reason: &str) -> Result<()> {
        let reason = truncate(reason);
        match action.state {
            PrExternalActionState::Running => {
                self.transition(
                    action,
                    PrExternalActionState::OutcomeUnknown,
                    None,
                    None,
                    Some(json!({"message": reason})),
                )
                .await?;
            }
            PrExternalActionState::OutcomeUnknown if expired(action) => {
                self.transition(
                    action,
                    PrExternalActionState::ReconciliationRequired,
                    None,
                    None,
                    Some(json!({"code": CODE_OUTCOME_UNKNOWN_EXPIRED, "message": reason})),
                )
                .await?;
            }
            _ => tracing::debug!(action_id = %action.id, "PR external action deferred"),
        }
        Ok(())
    }

    /// Human attention: the action and its admission go to reconciliation.
    async fn reconcile_push(
        &self,
        action: PrExternalAction,
        admission: &RemotePrAdmission,
        code: &'static str,
        message: &str,
    ) -> Result<()> {
        let running = self.to_running(action).await?;
        self.transition(
            &running,
            PrExternalActionState::ReconciliationRequired,
            None,
            None,
            Some(json!({"code": code, "message": message})),
        )
        .await?;
        let key = RemotePrAdmissionKey::from_admission(admission)?;
        let metadata =
            PrMetadataRepo::get_by_task_repo(&*self.db, &key.task_id, &key.repo_id).await?;
        self.pr
            .mark_reconciliation_required(
                admission,
                metadata.as_ref(),
                &format!("{code}: {message}"),
            )
            .await
    }

    /// A definitively failed, rejected or abandoned `push_update` ends the
    /// re-admission as `publication_failed` (the existing merge-failure
    /// rework path).
    async fn fail_admission(&self, action: &PrExternalAction, reason: &str) -> Result<()> {
        if action.kind != PrExternalActionKind::PushUpdate {
            return Ok(());
        }
        let admission = self.load_admission(&action.admission_id).await?;
        if !matches!(
            admission.state.as_str(),
            "admitted" | "reconciliation_required"
        ) {
            return Ok(());
        }
        let key = RemotePrAdmissionKey::from_admission(&admission)?;
        let metadata = PrMetadataRepo::get_by_task_repo(&*self.db, &key.task_id, &key.repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("PR metadata", key.admission_id.clone()))?;
        self.pr
            .apply_publication_failure(&admission, &metadata, reason)
            .await
    }

    async fn transition(
        &self,
        action: &PrExternalAction,
        to: PrExternalActionState,
        decision_id: Option<String>,
        provider_event_id: Option<String>,
        result: Option<Value>,
    ) -> Result<PrExternalAction> {
        let state = to.to_string();
        let admission = self.load_admission(&action.admission_id).await?;
        let key = RemotePrAdmissionKey::from_admission(&admission)?;
        if key.task_id != action.task_id {
            return Err(action_error(
                CODE_NOT_AUTHORIZED,
                "the external action no longer matches its exact Task admission",
            ));
        }
        let event = action_event_parts(
            &action.id,
            &action.task_id,
            &action.admission_id,
            &key.repo_id,
            action.kind,
            &format!("pr.external_action_{state}"),
            &state,
        );
        let write = PrExternalActionRepo::transition_pr_external_action(
            &*self.db,
            TransitionPrExternalAction {
                id: action.id.clone(),
                expected_version: action.version,
                to_state: to,
                decision_id,
                provider_event_id,
                provider_result_json: result.map(|value| value.to_string()),
                updated_at: now_rfc3339(),
            },
            Some(event),
        )
        .await
        .map_err(map_db)?;
        if let Some(event) = write.event.as_ref() {
            self.publish(event);
        }
        Ok(write.action)
    }

    fn publish(&self, event: &DomainEvent) {
        self.events.publish_committed(event);
    }
}

fn frozen_policy(admission: &RemotePrAdmission) -> Result<PrFeedbackPolicy> {
    let json = admission.feedback_policy_json.as_deref().ok_or_else(|| {
        action_error(
            CODE_NOT_AUTHORIZED,
            "the admission froze no feedback policy; no external action is authorized",
        )
    })?;
    PrFeedbackPolicy::from_json(json).map_err(ServiceError::invalid_operation)
}

/// Domain event of an action (ids and states only).
fn action_event_parts(
    id: &str,
    task_id: &str,
    admission_id: &str,
    repo_id: &str,
    kind: PrExternalActionKind,
    event_type: &str,
    state: &str,
) -> CreateDomainEvent {
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: "pr_external_action".to_owned(),
        entity_id: id.to_owned(),
        actor_type: "system".to_owned(),
        actor_id: None,
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: format!("pr-external-action:{id}"),
        causation_id: None,
        causation_depth: 1,
        dedupe_key: Some(format!("{event_type}:{id}")),
        payload_json: json!({
            "task_id": task_id,
            "action_id": id,
            "admission_id": admission_id,
            "repo_id": repo_id,
            "kind": kind.to_string(),
            "state": state,
        })
        .to_string(),
        created_at: now_rfc3339(),
    }
}

/// `true` when the action has been `outcome_unknown` longer than the window.
fn expired(action: &PrExternalAction) -> bool {
    chrono::DateTime::parse_from_rfc3339(&action.updated_at)
        .map(|updated| {
            chrono::Utc::now()
                .signed_duration_since(updated)
                .num_seconds()
                > RECOVERY_WINDOW_SECONDS
        })
        .unwrap_or(false)
}

fn truncate(text: &str) -> String {
    text.chars().take(MAX_STORED_ERROR_CHARS).collect()
}

/// A summary Artifact is `<body>\n<marker>` where the marker names the
/// Artifact and the SHA-256 of the body (`build_pr_feedback_summary`).
/// Returns that digest after verifying it.
fn verify_summary(artifact_id: &str, content: &str) -> Result<String> {
    if content.len() > MAX_SUMMARY_BYTES {
        return Err(ServiceError::invalid_operation(
            "the summary Artifact exceeds the comment size limit",
        ));
    }
    let (body, marker) = content.rsplit_once('\n').ok_or_else(|| {
        ServiceError::invalid_operation("the summary Artifact carries no anti-loop marker")
    })?;
    let digest = hex::encode(Sha256::digest(body.as_bytes()));
    if marker != forge_comment_marker(artifact_id, &digest) {
        return Err(ServiceError::invalid_operation(
            "the summary Artifact marker does not match its body digest",
        ));
    }
    Ok(digest)
}

/// Id of an existing PR conversation comment carrying `marker`, if any.
/// `Err` when the scan could not be completed (never "not found" by guess).
async fn find_marker(
    client: &GitHubClient,
    repo: &GitHubRepoRef,
    number: u64,
    marker: &str,
) -> std::result::Result<Option<String>, String> {
    let mut page = 1;
    for _ in 0..MAX_COMMENT_SCAN_PAGES {
        let listed = client
            .list_issue_comments(repo, number, None, page)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(comment) = listed
            .items
            .iter()
            .find(|comment| comment.body.contains(marker))
        {
            return Ok(Some(comment.id.to_string()));
        }
        match listed.next_page {
            Some(next) => page = next,
            None => return Ok(None),
        }
    }
    Err("the PR conversation is too long to prove the comment is absent".to_owned())
}

// ----------------------------------------------------------------------
// git push (credentials only in the child environment)
// ----------------------------------------------------------------------

#[derive(Debug)]
struct PushFailure {
    /// The remote definitively refused (authentication, permission, protected
    /// branch); nothing was pushed.
    definitive: bool,
    message: String,
}

fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with('-')
        && !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch.ends_with(".lock")
        && !branch.contains("..")
        && !branch.contains("//")
        && !branch.contains("@{")
        && branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

fn valid_sha(sha: &str) -> bool {
    (7..=64).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit())
}

/// Replace every secret substring and bound the text.
fn redact(text: &str, secrets: &[&str]) -> String {
    let mut redacted = text.to_owned();
    for secret in secrets.iter().filter(|secret| secret.len() >= 4) {
        redacted = redacted.replace(secret, "[redacted]");
    }
    truncate(redacted.trim())
}

fn definitive_push_failure(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    [
        "authentication failed",
        "permission denied",
        "permission to",
        "protected branch",
        "gh006",
        "gh013",
        "could not read username",
        "repository not found",
        "returned error: 401",
        "returned error: 403",
        "returned error: 404",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// `git push --force-with-lease=refs/heads/<branch>:<expected> <url>
/// <sha>:refs/heads/<branch>`: a compare-and-swap that only moves the remote
/// branch from exactly `expected` to `sha`. The caller has proven that `sha`
/// descends from `expected`.
async fn git_push_cas(
    dir: &Path,
    url: &str,
    branch: &str,
    expected: &str,
    sha: &str,
    token: Option<&Secret>,
) -> std::result::Result<(), PushFailure> {
    let refuse = |message: &str| PushFailure {
        definitive: true,
        message: message.to_owned(),
    };
    if !valid_branch(branch) || !valid_sha(expected) || !valid_sha(sha) {
        return Err(refuse("refusing to push an invalid branch or SHA"));
    }
    if url.is_empty() || url.starts_with('-') {
        return Err(refuse("refusing to push to an invalid remote"));
    }
    let https = match url::Url::parse(url) {
        Ok(parsed) => {
            if !parsed.username().is_empty() || parsed.password().is_some() {
                return Err(refuse("the remote URL must not embed credentials"));
            }
            if parsed.scheme() != "https" {
                return Err(refuse("PR updates require a credential-free HTTPS remote"));
            }
            true
        }
        // Bare local remotes are used by isolated service tests only.
        Err(_) if cfg!(test) && Path::new(url).is_absolute() => false,
        Err(_) => {
            return Err(refuse("PR updates require a credential-free HTTPS remote"));
        }
    };
    let mut command = Command::new("git");
    command
        .arg("push")
        .arg("--porcelain")
        .arg("--no-verify")
        .arg(format!("--force-with-lease=refs/heads/{branch}:{expected}"))
        .arg(url)
        .arg(format!("{sha}:refs/heads/{branch}"))
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let mut secrets: Vec<String> = Vec::new();
    if let (Some(token), true) = (token, https) {
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("x-access-token:{}", token.expose()));
        command
            .env("GIT_CONFIG_COUNT", "3")
            .env("GIT_CONFIG_KEY_0", "http.extraheader")
            .env(
                "GIT_CONFIG_VALUE_0",
                format!("Authorization: Basic {basic}"),
            )
            .env("GIT_CONFIG_KEY_1", "http.followRedirects")
            .env("GIT_CONFIG_VALUE_1", "false")
            .env("GIT_CONFIG_KEY_2", "credential.helper")
            .env("GIT_CONFIG_VALUE_2", "");
        secrets.push(token.expose().to_owned());
        secrets.push(basic);
    }
    let secret_refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let output = match tokio::time::timeout(PUSH_TIMEOUT, command.output()).await {
        Err(_) => {
            return Err(PushFailure {
                definitive: false,
                message: "git push timed out".to_owned(),
            })
        }
        Ok(Err(error)) => {
            return Err(PushFailure {
                definitive: false,
                message: redact(&format!("could not run git push: {error}"), &secret_refs),
            })
        }
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(PushFailure {
        definitive: definitive_push_failure(&stderr),
        message: redact(&stderr, &secret_refs),
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod multirepo_lane_c_tests {
    use super::*;
    use crate::credential_service::CredentialService;
    use crate::task_integration_operation::tests::{
        fixture, prepare_pr_admission_with_config, record_test_remote_outcome, PrConfigSetup,
    };
    use db::{ExternalFeedbackRepo, PrFeedbackPolicy};
    use events::EventBus;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    #[tokio::test]
    async fn multirepo_pr_feedback_and_external_action_are_repo_bound() {
        let temp = TempDir::new().expect("temporary directory");
        let fixture = fixture(
            &format!(
                "sqlite://{}",
                temp.path().join("repo-bound-pr.db").display()
            ),
            temp.path(),
        )
        .await;
        let event_bus = Arc::new(EventBus::new(64));
        let credentials = Arc::new(CredentialService::new(
            Arc::clone(&fixture.db),
            b"lane-c-multirepo-test-key",
        ));
        let (_manager, merge, publish, _evaluation, _gate, _policy) =
            prepare_pr_admission_with_config(
                &fixture,
                temp.path(),
                Arc::clone(&event_bus),
                Some(PrConfigSetup {
                    credentials,
                    policy: PrFeedbackPolicy {
                        ingest: true,
                        post_summary: db::PrFeedbackSummaryMode::HumanApproval,
                        ..PrFeedbackPolicy::default()
                    },
                }),
            )
            .await
            .expect("admission freezes repository feedback and action policy");
        let merge_id = merge.id().to_owned();
        drop(publish);
        drop(merge);
        record_test_remote_outcome(
            &fixture,
            &merge_id,
            "open",
            Some("repo-bound-open-event"),
            Some("17"),
            Some("https://github.example.invalid/pull/17"),
            Some("source-commit"),
            None,
            None,
        )
        .await
        .expect("provider observation opens this admission");
        let admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("admission lookup")
                .expect("admission exists");
        let key = RemotePrAdmissionKey::from_admission(&admission)
            .expect("exact Task and repo admission key");
        let mut unscoped_admission = admission.clone();
        unscoped_admission.repo_id = None;
        assert!(RemotePrAdmissionKey::from_admission(&unscoped_admission).is_err());

        let feedback_body = "Please update this repository";
        let feedback_digest = crate::pr_feedback::body_digest(feedback_body);
        ExternalFeedbackRepo::ingest_external_feedback(
            &*fixture.db,
            vec![db::CreateExternalFeedbackItem {
                id: new_uuid_v4(),
                task_id: key.task_id.clone(),
                admission_id: key.admission_id.clone(),
                provider_item_id: "review-comment-17".to_owned(),
                kind: db::ExternalFeedbackKind::IssueComment,
                author_login: "reviewer".to_owned(),
                author_association: "MEMBER".to_owned(),
                review_state: None,
                path: None,
                line: None,
                commit_sha: Some("source-commit".to_owned()),
                body: feedback_body.to_owned(),
                body_truncated: false,
                body_digest: feedback_digest,
                provider_url: Some("https://github.example.invalid/pull/17#comment".to_owned()),
                trust_class: db::ExternalFeedbackTrustClass::Trusted,
                head_state: db::ExternalFeedbackHeadState::Current,
                ingested_at: db::now_rfc3339(),
            }],
        )
        .await
        .expect("feedback is accepted for its exact open admission");
        let feedback = PrService::new(Arc::clone(&fixture.db), Arc::clone(&event_bus))
            .list_task_external_feedback(&fixture.task_id, None, 20)
            .await
            .expect("Task feedback projection")
            .items;
        assert_eq!(feedback.len(), 1);
        assert_eq!(feedback[0].task_id, key.task_id);
        assert_eq!(feedback[0].repo_id, key.repo_id);
        assert_eq!(feedback[0].admission_id, key.admission_id);

        let artifact_id = new_uuid_v4();
        let producer_execution_id = new_uuid_v4();
        let body = "Repository-specific triage summary";
        let body_digest = hex::encode(Sha256::digest(body.as_bytes()));
        let content = format!(
            "{body}\n{}",
            crate::pr_feedback::forge_comment_marker(&artifact_id, &body_digest)
        );
        let content_digest = hex::encode(Sha256::digest(content.as_bytes()));
        let now = db::now_rfc3339();
        let mut tx = fixture
            .db
            .pool()
            .begin()
            .await
            .expect("Artifact transaction");
        sqlx::query(
            "INSERT INTO execution (
                 id, task_id, status, created_at, updated_at, actor_kind, actor_id
             ) VALUES (?, ?, 'completed', ?, ?, 'human', ?)",
        )
        .bind(&producer_execution_id)
        .bind(&fixture.task_id)
        .bind(&now)
        .bind(&now)
        .bind(&fixture.user_id)
        .execute(&mut *tx)
        .await
        .expect("summary producer Execution");
        sqlx::query(
            "INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
             VALUES (?, ?, ?)",
        )
        .bind(&artifact_id)
        .bind(&producer_execution_id)
        .bind(&fixture.task_id)
        .execute(&mut *tx)
        .await
        .expect("Artifact producer");
        sqlx::query(
            "INSERT INTO artifact (
                 id, task_id, kind, storage_kind, content, metadata_json, digest, created_at
             ) VALUES (?, ?, 'summary', 'inline', ?, '{}', ?, ?)",
        )
        .bind(&artifact_id)
        .bind(&fixture.task_id)
        .bind(&content)
        .bind(&content_digest)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .expect("repo-bound summary Artifact");
        tx.commit().await.expect("Artifact and producer commit");

        let service = PrExternalActionService::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
            PrService::new(Arc::clone(&fixture.db), Arc::clone(&event_bus)),
        );
        let action = service
            .request_post_comment(&key.admission_id, &artifact_id)
            .await
            .expect("repo-bound comment request")
            .expect("frozen HumanApproval policy creates a proposed action");
        assert_eq!(action.task_id, key.task_id);
        assert_eq!(action.admission_id, key.admission_id);
        assert_eq!(action.state, PrExternalActionState::Proposed);
        let payload_json: String = sqlx::query_scalar(
            "SELECT payload_json FROM domain_event
             WHERE entity_type = 'pr_external_action' AND entity_id = ?
               AND event_type = 'pr.external_action_created'",
        )
        .bind(&action.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("action event");
        let payload: Value = serde_json::from_str(&payload_json).expect("action payload JSON");
        assert_eq!(payload["task_id"], key.task_id);
        assert_eq!(payload["admission_id"], key.admission_id);
        assert_eq!(payload["repo_id"], key.repo_id);
    }
}
