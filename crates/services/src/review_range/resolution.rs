//! Open findings and the Human decisions that change them.
//!
//! Waiver authority (C0.13): any Human with an active RoleMembership in the
//! Task, or a Project owner/admin, always with a non-empty reference. P0 is
//! never waivable. The database writer rechecks authority and ledger versions
//! in the same transaction that records each decision.

use super::{
    attribution::ReviewAttribution, canonical_digest, coverage::ReviewCoverageService,
    scheduler::ReviewScheduler,
};
use crate::{
    project_authority::user_can_administer_project, task_service::workspace::resolve_repo_source,
    Result, ServiceError,
};
use db::{
    new_uuid_v4, ActorRef, CreateDecision, CreateDomainEvent, CreateProposal, DecisionOutcome,
    ProjectRepo, ProposalTarget, ProposalTargetKind, RepoRepo, ReviewBaseSource, ReviewCheckpoint,
    ReviewCheckpointState, ReviewCommitAttribution, ReviewFinding, ReviewFindingResolution,
    ReviewHumanDecision, ReviewLedgerRepo, ReviewResolutionOutcome, RoleMembershipRepo,
    RoleMembershipStatus, SqliteDb, TaskRepo, TaskRepositoryRole, TaskReviewBase,
    WorkUnitRepositoryIntegration, WorkUnitRepositoryIntegrationOutcome,
};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

const NOT_WAIVABLE: &str = "review.finding.not_waivable";
const WAIVER_NOT_AUTHORIZED: &str = "review.finding.waiver_not_authorized";
const COVERAGE_VERSION_CONFLICT: &str = "review.coverage.version_conflict";
const CHECKPOINT_VERSION_CONFLICT: &str = "review.checkpoint.version_conflict";
const UNRECORDED_COMMITS: &str = "review.range.unrecorded_commits";

#[derive(Serialize)]
struct RebaselineSubject<'a> {
    repo_id: &'a str,
    previous_revision: i64,
    candidate_commit: &'a str,
    base_commit: &'a str,
    base_tree: &'a str,
}

pub struct ReviewFindingService {
    db: Arc<SqliteDb>,
    workspace_root: PathBuf,
}

impl ReviewFindingService {
    pub fn new(db: Arc<SqliteDb>, workspace_root: PathBuf) -> Self {
        Self { db, workspace_root }
    }

    pub async fn open_findings(
        &self,
        task_id: &str,
        repo_id: &str,
        candidate: &str,
    ) -> Result<Vec<ReviewFinding>> {
        let coverage =
            ReviewCoverageService::new(Arc::clone(&self.db), self.workspace_root.clone())
                .compute(task_id, repo_id, Some(candidate))
                .await?;
        let open = coverage
            .open_finding_ids
            .into_iter()
            .collect::<BTreeSet<_>>();
        let rows = self.db.load_review_coverage_rows(task_id, repo_id).await?;
        let mut findings = rows
            .findings
            .into_iter()
            .filter(|finding| open.contains(&finding.id))
            .collect::<Vec<_>>();
        findings.sort_by(|left, right| {
            left.severity
                .cmp(&right.severity)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(findings)
    }

    pub async fn waive(
        &self,
        user_id: &str,
        task_id: &str,
        finding_id: &str,
        expected_coverage_revision: i64,
        rationale: String,
        reference: String,
    ) -> Result<ReviewFindingResolution> {
        validate_text(&rationale, 4096, "waiver rationale")?;
        validate_text(&reference, 512, "waiver reference")?;
        let finding = self
            .db
            .get_review_finding(finding_id)
            .await?
            .filter(|finding| finding.task_id == task_id)
            .ok_or_else(|| ServiceError::not_found("review finding", finding_id.to_owned()))?;
        if finding.severity == db::ReviewFindingSeverity::P0 {
            return Err(coordination(NOT_WAIVABLE, "P0 findings cannot be waived"));
        }
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        self.require_waiver_authority(&task.project_id, task_id, user_id)
            .await?;
        let coverage =
            ReviewCoverageService::new(Arc::clone(&self.db), self.workspace_root.clone())
                .compute(task_id, &finding.repo_id, None)
                .await?;
        let Some(candidate) = coverage.candidate_commit else {
            return Err(coordination(
                "review.range.head_not_integrated",
                "a finding can be waived only against an integrated candidate",
            ));
        };
        if !coverage.open_finding_ids.iter().any(|id| id == finding_id) {
            return Err(coordination(
                NOT_WAIVABLE,
                "the finding is no longer open for the current candidate",
            ));
        }
        let now = db::now_rfc3339();
        let decision = human_decision(
            task_id,
            user_id,
            ProposalTargetKind::ReviewFinding,
            finding_id,
            "waive_review_finding",
            &rationale,
            None,
            Some(finding.digest.clone()),
            &now,
        );
        let resolution = ReviewFindingResolution {
            id: new_uuid_v4(),
            finding_id: finding.id.clone(),
            task_id: task_id.to_owned(),
            repo_id: finding.repo_id.clone(),
            outcome: ReviewResolutionOutcome::Waived,
            anchor_commit: candidate,
            report_artifact_id: None,
            decision_id: Some(decision.decision.id.clone()),
            waiver_reference: Some(reference),
            evidence: rationale,
            actor: ActorRef::Human(user_id.to_owned()),
            created_at: now.clone(),
        };
        let mut events = decision_events(&decision, &now);
        events.push(review_event(
            "review.finding_resolved",
            "review_finding",
            finding_id,
            task_id,
            json!({
                "task_id": task_id,
                "repo_id": finding.repo_id,
                "finding_id": finding_id,
                "severity": finding.severity.to_string(),
                "outcome": "waived",
                "resolution_id": resolution.id,
            }),
            user_id,
            &now,
            Some(format!("review-finding-waived:{}", resolution.id)),
        ));
        let resolution = self
            .db
            .waive_review_finding(expected_coverage_revision, decision, resolution, events)
            .await
            .map_err(map_waiver_error)?;
        let _ = ReviewScheduler::new(Arc::clone(&self.db), self.workspace_root.clone())
            .reconcile_task(task_id)
            .await;
        Ok(resolution)
    }

    /// The new base is the merge-base of the repository default branch and the
    /// latest successfully integrated candidate. Authority and coverage CAS
    /// are revalidated by the database writer.
    pub async fn rebaseline(
        &self,
        user_id: &str,
        task_id: &str,
        repo_id: &str,
        expected_coverage_revision: i64,
        rationale: String,
    ) -> Result<TaskReviewBase> {
        validate_text(&rationale, 4096, "re-baseline rationale")?;
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        self.require_project_admin(&task.project_id, user_id)
            .await?;
        self.require_writable_repo(task_id, repo_id).await?;
        let rows = self.db.load_review_coverage_rows(task_id, repo_id).await?;
        let current_base = rows.base.as_ref().ok_or_else(|| {
            coordination(
                "review.coverage.base_unknown",
                "the initial review base must be pinned before re-baselining",
            )
        })?;
        let latest = latest_success(&rows.integrations).ok_or_else(|| {
            coordination(
                "review.range.head_not_integrated",
                "re-baselining requires a successful repository integration",
            )
        })?;
        let candidate = latest.target_after_sha.as_deref().ok_or_else(|| {
            coordination(
                "review.range.head_not_integrated",
                "the latest successful integration has no target commit",
            )
        })?;
        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
        let repo_path = resolve_repo_source(&repo, &self.workspace_root).await?;
        if !git::commit_exists(Path::new(&repo_path), candidate).await? {
            return Err(coordination(
                "review.coverage.base_not_ancestor",
                "the integrated candidate is unavailable in the repository",
            ));
        }
        let base_commit = git::merge_base(Path::new(&repo_path), &repo.default_branch, candidate)
            .await
            .map_err(|_| {
                coordination(
                    "review.coverage.base_not_ancestor",
                    "the default branch and integrated candidate have no available common base",
                )
            })?;
        let base_tree = git::resolve_tree(Path::new(&repo_path), &base_commit).await?;
        let now = db::now_rfc3339();
        let revision = current_base.revision.saturating_add(1);
        let subject_digest = canonical_digest(&RebaselineSubject {
            repo_id,
            previous_revision: current_base.revision,
            candidate_commit: candidate,
            base_commit: &base_commit,
            base_tree: &base_tree,
        })?;
        let decision = human_decision(
            task_id,
            user_id,
            ProposalTargetKind::ReviewBase,
            repo_id,
            "rebaseline_review",
            &rationale,
            Some(current_base.revision),
            Some(subject_digest),
            &now,
        );
        let base = TaskReviewBase {
            task_id: task_id.to_owned(),
            repo_id: repo_id.to_owned(),
            revision,
            base_commit,
            base_tree,
            source: ReviewBaseSource::HumanRebaseline,
            decision_id: Some(decision.decision.id.clone()),
            pinned_at: now.clone(),
        };
        self.db
            .rebaseline_review(
                expected_coverage_revision,
                decision.clone(),
                base.clone(),
                decision_events(&decision, &now),
            )
            .await
            .map_err(map_coverage_write_error)?;
        let _ = ReviewScheduler::new(Arc::clone(&self.db), self.workspace_root.clone())
            .reconcile_task(task_id)
            .await;
        Ok(base)
    }

    /// Only commits currently classified as unrecorded for this exact blocked
    /// range can receive an explicit Human attribution. After the immutable
    /// decision is stored, the blocked checkpoint is retried through the
    /// scheduler's owner/admin route.
    pub async fn attribute_commits(
        &self,
        user_id: &str,
        task_id: &str,
        checkpoint_id: &str,
        expected_version: i64,
        attributions: Vec<(String, ActorRef)>,
        rationale: String,
    ) -> Result<ReviewCheckpoint> {
        validate_text(&rationale, 4096, "commit attribution rationale")?;
        if attributions.is_empty() {
            return Err(coordination(
                UNRECORDED_COMMITS,
                "at least one currently unrecorded commit must be attributed",
            ));
        }
        let checkpoint = self
            .db
            .get_review_checkpoint(checkpoint_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("review checkpoint", checkpoint_id.to_owned())
            })?;
        if checkpoint.task_id != task_id
            || checkpoint.version != expected_version
            || checkpoint.state != ReviewCheckpointState::Blocked
            || checkpoint.blocked_code.as_deref() != Some(UNRECORDED_COMMITS)
        {
            return Err(coordination(
                CHECKPOINT_VERSION_CONFLICT,
                "the checkpoint is no longer awaiting attribution",
            ));
        }
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        self.require_project_admin(&task.project_id, user_id)
            .await?;
        self.require_writable_repo(task_id, &checkpoint.repo_id)
            .await?;
        let coverage =
            ReviewCoverageService::new(Arc::clone(&self.db), self.workspace_root.clone())
                .compute(task_id, &checkpoint.repo_id, Some(&checkpoint.head_commit))
                .await?;
        let base = coverage
            .frontier_commit
            .as_deref()
            .or(coverage.base_commit.as_deref())
            .ok_or_else(|| {
                coordination(
                    "review.coverage.base_unknown",
                    "the blocked range has no known review base",
                )
            })?;
        let facts = ReviewAttribution::new(Arc::clone(&self.db), self.workspace_root.clone())
            .contributors(task_id, &checkpoint.repo_id, base, &checkpoint.head_commit)
            .await?;
        let allowed = facts
            .unrecorded_commits
            .into_iter()
            .collect::<BTreeSet<_>>();
        let submitted = attributions
            .iter()
            .map(|(commit, _)| commit.clone())
            .collect::<BTreeSet<_>>();
        if submitted.len() != attributions.len()
            || submitted.is_empty()
            || submitted.iter().any(|commit| !allowed.contains(commit))
        {
            return Err(coordination(
                UNRECORDED_COMMITS,
                "attributions must name distinct commits still unrecorded in this range",
            ));
        }
        let mut sorted = attributions.clone();
        sorted.sort_by(|left, right| left.0.cmp(&right.0));
        let attribution_digest = canonical_digest(&sorted)?;
        let now = db::now_rfc3339();
        let decision = human_decision(
            task_id,
            user_id,
            ProposalTargetKind::ReviewCommit,
            &checkpoint.repo_id,
            "attribute_review_commits",
            &rationale,
            Some(checkpoint.version),
            Some(attribution_digest),
            &now,
        );
        let commit_rows = sorted
            .into_iter()
            .map(|(commit_sha, actor)| ReviewCommitAttribution {
                id: new_uuid_v4(),
                task_id: task_id.to_owned(),
                repo_id: checkpoint.repo_id.clone(),
                commit_sha,
                actor,
                decision_id: decision.decision.id.clone(),
                created_at: now.clone(),
            })
            .collect();
        self.db
            .attribute_review_commits(
                checkpoint_id,
                expected_version,
                decision.clone(),
                commit_rows,
                decision_events(&decision, &now),
            )
            .await
            .map_err(map_checkpoint_write_error)?;
        ReviewScheduler::new(Arc::clone(&self.db), self.workspace_root.clone())
            .retry(user_id, checkpoint_id, expected_version)
            .await
    }

    async fn require_waiver_authority(
        &self,
        project_id: &str,
        task_id: &str,
        user_id: &str,
    ) -> Result<()> {
        let project = ProjectRepo::get_by_id(&*self.db, project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))?;
        if user_can_administer_project(&self.db, &project, user_id).await? {
            return Ok(());
        }
        let member = RoleMembershipRepo::list_by_task(&*self.db, task_id, false)
            .await?
            .into_iter()
            .any(|(_, membership)| {
                membership.status == RoleMembershipStatus::Active
                    && membership.actor_kind == db::ActorKind::Human
                    && membership.actor_id == user_id
            });
        if member {
            Ok(())
        } else {
            Err(coordination(
                WAIVER_NOT_AUTHORIZED,
                "waiving a finding requires an active Human Task membership or Project owner/admin authority",
            ))
        }
    }

    async fn require_project_admin(&self, project_id: &str, user_id: &str) -> Result<()> {
        let project = ProjectRepo::get_by_id(&*self.db, project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))?;
        if user_can_administer_project(&self.db, &project, user_id).await? {
            Ok(())
        } else {
            Err(coordination(
                "review.policy.not_authorized",
                "re-baselining or attributing commits requires a Project owner or admin",
            ))
        }
    }

    async fn require_writable_repo(&self, task_id: &str, repo_id: &str) -> Result<()> {
        if TaskRepo::list_repositories(&*self.db, task_id)
            .await?
            .iter()
            .any(|repository| {
                repository.repo_id == repo_id
                    && matches!(
                        repository.role,
                        TaskRepositoryRole::Primary | TaskRepositoryRole::Affected
                    )
            })
        {
            Ok(())
        } else {
            Err(ServiceError::not_found(
                "Task repository",
                repo_id.to_owned(),
            ))
        }
    }
}

fn human_decision(
    task_id: &str,
    user_id: &str,
    target_kind: ProposalTargetKind,
    target_id: &str,
    action: &str,
    rationale: &str,
    target_version: Option<i64>,
    target_digest: Option<String>,
    now: &str,
) -> ReviewHumanDecision {
    let proposal_id = new_uuid_v4();
    let decision_id = new_uuid_v4();
    let actor = ActorRef::Human(user_id.to_owned());
    ReviewHumanDecision {
        proposal: CreateProposal {
            id: proposal_id.clone(),
            task_id: task_id.to_owned(),
            proposer: actor.clone(),
            target: ProposalTarget {
                kind: target_kind,
                id: target_id.to_owned(),
            },
            action: action.to_owned(),
            reason: rationale.to_owned(),
            target_version,
            target_digest,
            required_policy_ref: None,
            required_policy_version: None,
            required_policy_digest: None,
            supersedes_proposal_id: None,
            artifact_ids: Vec::new(),
            created_at: now.to_owned(),
        },
        decision: CreateDecision {
            id: decision_id,
            task_id: task_id.to_owned(),
            proposal_id,
            proposal_version: 1,
            outcome: DecisionOutcome::Approve,
            rationale: rationale.to_owned(),
            policy_ref: None,
            policy_version: None,
            policy_digest: None,
            actors: vec![actor],
            created_at: now.to_owned(),
        },
    }
}

fn decision_events(decision: &ReviewHumanDecision, now: &str) -> Vec<CreateDomainEvent> {
    let task_id = &decision.proposal.task_id;
    let actor_id = decision.decision.actors[0].id();
    vec![
        review_event(
            "proposal.created",
            "proposal",
            &decision.proposal.id,
            task_id,
            json!({
                "proposal_id": decision.proposal.id,
                "task_id": task_id,
                "target_kind": decision.proposal.target.kind.to_string(),
            }),
            actor_id,
            now,
            Some(format!("review-proposal:{}", decision.proposal.id)),
        ),
        review_event(
            "decision.recorded",
            "decision",
            &decision.decision.id,
            task_id,
            json!({
                "decision_id": decision.decision.id,
                "task_id": task_id,
                "proposal_id": decision.decision.proposal_id,
                "outcome": decision.decision.outcome.to_string(),
            }),
            actor_id,
            now,
            Some(format!("review-decision:{}", decision.decision.id)),
        ),
    ]
}

fn review_event(
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    task_id: &str,
    payload: serde_json::Value,
    actor_id: &str,
    now: &str,
    dedupe_key: Option<String>,
) -> CreateDomainEvent {
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: entity_type.to_owned(),
        entity_id: entity_id.to_owned(),
        actor_type: "human".to_owned(),
        actor_id: Some(actor_id.to_owned()),
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: task_id.to_owned(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key,
        payload_json: payload.to_string(),
        created_at: now.to_owned(),
    }
}

fn validate_text(value: &str, max_bytes: usize, label: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > max_bytes {
        return Err(ServiceError::invalid_operation(format!(
            "{label} must contain 1 to {max_bytes} UTF-8 bytes"
        )));
    }
    Ok(())
}

fn latest_success(
    integrations: &[WorkUnitRepositoryIntegration],
) -> Option<&WorkUnitRepositoryIntegration> {
    integrations
        .iter()
        .filter(|integration| {
            integration.outcome == WorkUnitRepositoryIntegrationOutcome::Success
                && integration.target_after_sha.is_some()
        })
        .max_by(|left, right| (&left.started_at, &left.id).cmp(&(&right.started_at, &right.id)))
}

fn map_waiver_error(error: db::DbError) -> ServiceError {
    match error {
        db::DbError::VersionConflict => coordination(
            COVERAGE_VERSION_CONFLICT,
            "review coverage changed; reload before recording the waiver",
        ),
        db::DbError::ProjectAdminRequired => coordination(
            WAIVER_NOT_AUTHORIZED,
            "waiving a finding requires an active Human Task member or Project owner/admin",
        ),
        db::DbError::Check(message) if message == NOT_WAIVABLE => {
            coordination(NOT_WAIVABLE, "P0 findings cannot be waived")
        }
        db::DbError::Check(message)
            if message.contains("waiver requires an active Task member") =>
        {
            coordination(WAIVER_NOT_AUTHORIZED, message)
        }
        error => error.into(),
    }
}

fn map_coverage_write_error(error: db::DbError) -> ServiceError {
    match error {
        db::DbError::VersionConflict => coordination(
            COVERAGE_VERSION_CONFLICT,
            "review coverage changed; reload before re-baselining",
        ),
        db::DbError::ProjectAdminRequired => coordination(
            "review.policy.not_authorized",
            "re-baselining requires a current Project owner or admin",
        ),
        error => error.into(),
    }
}

fn map_checkpoint_write_error(error: db::DbError) -> ServiceError {
    match error {
        db::DbError::VersionConflict | db::DbError::InvalidTransition => coordination(
            CHECKPOINT_VERSION_CONFLICT,
            "the blocked checkpoint changed; reload before recording attribution",
        ),
        db::DbError::ProjectAdminRequired => coordination(
            "review.policy.not_authorized",
            "commit attribution requires a current Project owner or admin",
        ),
        error => error.into(),
    }
}

fn coordination(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::Coordination {
        code,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_range_resolution_rejects_empty_and_oversized_text() {
        assert!(validate_text("   ", 16, "rationale").is_err());
        assert!(validate_text("x".repeat(17).as_str(), 16, "rationale").is_err());
        assert!(validate_text("valid", 16, "rationale").is_ok());
    }

    #[test]
    fn review_range_resolution_decision_events_omit_rationale_and_references() {
        let now = "2026-11-27T00:00:00Z";
        let decision = human_decision(
            "task",
            "human",
            ProposalTargetKind::ReviewFinding,
            "finding",
            "waive_review_finding",
            "private rationale",
            None,
            Some("digest".to_owned()),
            now,
        );
        let events = decision_events(&decision, now);
        let serialized = events
            .iter()
            .map(|event| event.payload_json.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!serialized.contains("private rationale"));
        assert!(!serialized.contains("waiver_reference"));
    }
}
