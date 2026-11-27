//! Durable review triggers and bounded recovery scanning.
//!
//! The EventBus is only a wake-up hint. This service reads the Task, policy,
//! integration ledger, and review ledger before it creates any checkpoint.

use super::{attribution::ReviewAttribution, canonical_digest, coverage::ReviewCoverageService};
use crate::{
    domain_event_service::DomainEventService, project_authority::user_can_administer_project,
    task_service::workspace::resolve_repo_source, Result, ServiceError,
};
use db::{
    ActivateReviewCheckpoint, ActorKind, ActorRef, CreateDomainEvent, CreateWorkUnit, DomainEvent,
    DomainEventRepo, ExecutionRepo, ExecutionStatus, ProjectRepo, RepoRepo, ReviewCheckpoint,
    ReviewCheckpointReason, ReviewCheckpointRepo, ReviewCheckpointRequestRecord,
    ReviewCheckpointState, ReviewCoverageRows, ReviewFinding, ReviewInputFinding, ReviewLedgerRepo,
    ReviewPolicyDocument, ReviewPolicyRepo, ReviewRange, ReviewRangeContributor, ReviewRangeReport,
    ReviewRangeReportRepo, ReviewRequestSourceKind, ReviewTriggerMode, RoleMembershipRepo,
    RoleMembershipStatus, SqliteDb, TaskCoordinationMode, TaskCoordinationRepo, TaskLifecycleRepo,
    TaskRepo, TaskRepositoryRole, TaskRepositorySetRepo, TaskReviewPolicyRevision,
    TaskRoleCapabilityConfigurationRepo, TaskRoleRepo, TransitionReviewCheckpoint,
    UpsertQueuedReviewCheckpoint, WorkUnitRepositoryIntegration,
    WorkUnitRepositoryIntegrationOutcome, WorkUnitReviewRange,
};
use events::EventBus;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::Row;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

const NOTHING_TO_REVIEW: &str = "review.range.nothing_to_review";
const CHECKPOINT_ACTIVE: &str = "review.range.checkpoint_active";
const HEAD_NOT_INTEGRATED: &str = "review.range.head_not_integrated";
const UNRECORDED_COMMITS: &str = "review.range.unrecorded_commits";
const REVIEWER_UNAVAILABLE: &str = "review.checkpoint.reviewer_unavailable";
const CHECKPOINT_LIMIT: &str = "review.checkpoint.limit_reached";
const REVIEW_NOT_PERMITTED: &str = "review.range.not_permitted";
const BASE_UNKNOWN: &str = "review.coverage.base_unknown";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewRequestSource {
    Human {
        user_id: String,
    },
    Coordinator {
        execution_id: String,
        request_id: String,
    },
}

#[derive(Serialize)]
struct CanonicalRangeDigest<'a> {
    task_id: &'a str,
    repo_id: &'a str,
    repository_set_revision: i64,
    base_commit: &'a str,
    head_commit: &'a str,
    base_tree: &'a str,
    head_tree: &'a str,
    integration_workspace_id: &'a str,
    head_integration_id: &'a str,
}

pub struct ReviewScheduler {
    db: Arc<SqliteDb>,
    workspace_root: PathBuf,
    event_bus: Option<Arc<EventBus>>,
    recovery_after: tokio::sync::Mutex<Option<String>>,
}

impl ReviewScheduler {
    pub fn new(db: Arc<SqliteDb>, workspace_root: PathBuf) -> Self {
        Self {
            db,
            workspace_root,
            event_bus: None,
            recovery_after: tokio::sync::Mutex::new(None),
        }
    }

    /// Attach the in-process wakeup bus. The committed domain row remains the
    /// authority; this only wakes consumers after its transaction commits.
    pub fn with_event_bus(mut self, event_bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Reconcile only events that can change review coverage or its triggers.
    pub async fn process_domain_event(&self, event: &DomainEvent) -> Result<usize> {
        if event.scope_type != "task" || !self.is_review_trigger(event).await? {
            return Ok(0);
        }
        self.reconcile_task(&event.scope_id).await
    }

    /// Bounded durable scan; call periodically as well as on EventBus wakeups.
    pub async fn recover(&self, limit: u32) -> Result<usize> {
        let limit = limit.min(500);
        if limit == 0 {
            return Ok(0);
        }
        let mut recovery_after = self.recovery_after.lock().await;
        let mut after = (*recovery_after).clone();
        let mut scanned = 0_u32;
        let mut created = 0_usize;
        while scanned < limit {
            let page_limit = (limit - scanned).min(100);
            let batch = self
                .db
                .list_tasks_with_review_policy(after.as_deref(), page_limit)
                .await?;
            if batch.is_empty() {
                *recovery_after = None;
                break;
            }
            for task_id in &batch {
                created = created.saturating_add(self.reconcile_task(task_id).await?);
                scanned += 1;
                after = Some(task_id.clone());
                *recovery_after = after.clone();
            }
            if batch.len() < page_limit as usize {
                *recovery_after = None;
                break;
            }
        }
        Ok(created)
    }

    /// Reconcile one Task across its writable repositories.
    pub async fn reconcile_task(&self, task_id: &str) -> Result<usize> {
        let Some(task) = TaskRepo::get_by_id(&*self.db, task_id, false).await? else {
            return Ok(0);
        };
        let Some(policy) = self.db.get_current_task_review_policy(task_id).await? else {
            return Ok(0);
        };
        let lifecycle = task.lifecycle_state;
        let dispatch = TaskLifecycleRepo::get_task_dispatch(&*self.db, task_id).await?;
        let may_admit = !matches!(
            lifecycle,
            db::TaskLifecycleState::Done
                | db::TaskLifecycleState::Cancelled
                | db::TaskLifecycleState::Merging
        ) && dispatch
            .is_some_and(|dispatch| dispatch.state == db::TaskDispatchState::Running);
        let repositories = TaskRepo::list_repositories(&*self.db, task_id).await?;
        let mut count = 0;
        for repository in repositories.into_iter().filter(|repository| {
            matches!(
                repository.role,
                TaskRepositoryRole::Primary | TaskRepositoryRole::Affected
            )
        }) {
            let policy_service = super::policy::ReviewPolicyService::new(
                Arc::clone(&self.db),
                self.workspace_root.clone(),
            );
            if may_admit {
                let _ = policy_service
                    .pin_base(task_id, &repository.repo_id)
                    .await?;
            }
            let rows = self
                .db
                .load_review_coverage_rows(task_id, &repository.repo_id)
                .await?;
            let mut reconciliation_error = None;
            if may_admit {
                if let Some(latest) = latest_success(&rows.integrations) {
                    if let Some(latest_head) = latest
                        .target_after_sha
                        .as_deref()
                        .filter(|head| !head.is_empty())
                    {
                        self.supersede_stale_unstarted_pending(
                            task_id,
                            &repository.repo_id,
                            latest_head,
                        )
                        .await?;
                        if !self
                            .has_active_checkpoint(task_id, &repository.repo_id)
                            .await?
                        {
                            let queued_recheck = rows
                                .checkpoints
                                .iter()
                                .filter(|checkpoint| {
                                    checkpoint.state == ReviewCheckpointState::Queued
                                        && checkpoint
                                            .reasons
                                            .contains(&ReviewCheckpointReason::Recheck)
                                })
                                .min_by(|left, right| {
                                    (&left.created_at, &left.id)
                                        .cmp(&(&right.created_at, &right.id))
                                })
                                .cloned();
                            let trigger = if let Some(recheck) = queued_recheck {
                                Some((
                                    ReviewCheckpointReason::Recheck,
                                    recheck.head_commit.clone(),
                                    Some(recheck),
                                ))
                            } else {
                                self.automatic_trigger(
                                    task_id,
                                    &repository.repo_id,
                                    &policy,
                                    &rows,
                                    latest_head,
                                )
                                .await?
                                .map(|(reason, head)| (reason, head, None))
                            };
                            if let Some((reason, head, existing_checkpoint)) = trigger {
                                if self.checkpoint_count(task_id, &repository.repo_id).await?
                                    >= i64::from(policy.policy.max_checkpoints_per_repo)
                                    && existing_checkpoint.is_none()
                                {
                                    reconciliation_error = Some(coordination(
                                        CHECKPOINT_LIMIT,
                                        "the review checkpoint limit for this repository is exhausted",
                                    ));
                                } else {
                                    match self
                                        .queue_target(
                                            task_id,
                                            &repository.repo_id,
                                            &head,
                                            reason,
                                            None,
                                            None,
                                            None,
                                            existing_checkpoint,
                                        )
                                        .await
                                    {
                                        Ok(Some(_)) => count += 1,
                                        Ok(None) => {}
                                        Err(error)
                                            if error_code(&error) == Some(NOTHING_TO_REVIEW) => {}
                                        Err(error) => reconciliation_error = Some(error),
                                    }
                                }
                            }
                        }
                    }
                }
            }
            self.emit_coverage_changed(task_id, &repository.repo_id)
                .await?;
            if let Some(error) = reconciliation_error {
                return Err(error);
            }
        }
        Ok(count)
    }

    /// Human or coordinator request. Forge selects the latest successful
    /// integration; callers never supply commits.
    pub async fn request(
        &self,
        source: ReviewRequestSource,
        task_id: &str,
        repo_id: &str,
        reason: String,
        idempotency_key: &str,
    ) -> Result<ReviewCheckpoint> {
        if reason.trim().is_empty() || reason.len() > 1024 {
            return Err(coordination(
                NOTHING_TO_REVIEW,
                "review request reason must contain 1 to 1024 UTF-8 bytes",
            ));
        }
        if idempotency_key.trim().is_empty() || idempotency_key.len() > 256 {
            return Err(coordination(
                NOTHING_TO_REVIEW,
                "review request idempotency key must contain 1 to 256 UTF-8 bytes",
            ));
        }
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let Some(policy) = self.db.get_current_task_review_policy(task_id).await? else {
            return Err(coordination(
                NOTHING_TO_REVIEW,
                "the Task has no review policy",
            ));
        };
        let (source_kind, source_ref, actor) = self
            .authorize_request(&source, task_id, &task.project_id, &policy.policy)
            .await?;
        if let Some(existing) = self
            .request_replay(task_id, idempotency_key, source_kind, &source_ref, &reason)
            .await?
        {
            return Ok(existing);
        }
        let rows = self.db.load_review_coverage_rows(task_id, repo_id).await?;
        let Some(head) = latest_success(&rows.integrations)
            .and_then(|integration| integration.target_after_sha.as_deref())
        else {
            return Err(coordination(
                HEAD_NOT_INTEGRATED,
                "a range review can target only a successful integrated commit",
            ));
        };
        let request = ReviewCheckpointRequestRecord {
            idempotency_key: idempotency_key.to_owned(),
            source_kind,
            source_ref,
            reason: reason.clone(),
        };
        self.queue_target(
            task_id,
            repo_id,
            head,
            ReviewCheckpointReason::Requested,
            Some(request),
            actor,
            None,
            None,
        )
        .await?
        .ok_or_else(|| coordination(NOTHING_TO_REVIEW, "the integrated head is already covered"))
    }

    /// Supersedes a blocked checkpoint and creates its next attempt. Only a
    /// current Project owner/admin may retry a blocked checkpoint.
    pub async fn retry(
        &self,
        user_id: &str,
        checkpoint_id: &str,
        expected_version: i64,
    ) -> Result<ReviewCheckpoint> {
        let checkpoint = self
            .db
            .get_review_checkpoint(checkpoint_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("review checkpoint", checkpoint_id.to_owned())
            })?;
        let task = TaskRepo::get_by_id(&*self.db, &checkpoint.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", checkpoint.task_id.clone()))?;
        self.require_project_admin(&task.project_id, user_id)
            .await?;
        if checkpoint.version != expected_version {
            return Err(ServiceError::Coordination {
                code: "review.checkpoint.version_conflict",
                message: "the checkpoint changed; reload it and retry".to_owned(),
            });
        }
        if checkpoint.state != ReviewCheckpointState::Blocked {
            return Err(ServiceError::Coordination {
                code: "review.checkpoint.version_conflict",
                message: "only a blocked checkpoint can be retried".to_owned(),
            });
        }
        let Some(policy) = self
            .db
            .get_current_task_review_policy(&checkpoint.task_id)
            .await?
        else {
            return Err(coordination(
                NOTHING_TO_REVIEW,
                "the Task has no review policy",
            ));
        };
        let latest = self
            .db
            .load_review_coverage_rows(&checkpoint.task_id, &checkpoint.repo_id)
            .await?;
        let head = latest_success(&latest.integrations)
            .and_then(|integration| integration.target_after_sha.as_deref())
            .unwrap_or(&checkpoint.head_commit)
            .to_owned();
        let coverage =
            ReviewCoverageService::new(Arc::clone(&self.db), self.workspace_root.clone())
                .compute(&checkpoint.task_id, &checkpoint.repo_id, Some(&head))
                .await?;
        if coverage.status == super::ReviewCoverageStatus::Unchanged
            || (coverage.frontier_commit.as_deref() == Some(head.as_str())
                && coverage.ranges.iter().any(|range| range.valid_edge))
        {
            let now = db::now_rfc3339();
            let event = review_event(
                "review.checkpoint_state_changed",
                "review_checkpoint",
                &checkpoint.id,
                &checkpoint.task_id,
                json!({
                    "task_id": checkpoint.task_id,
                    "repo_id": checkpoint.repo_id,
                    "checkpoint_id": checkpoint.id,
                    "from_state": "blocked",
                    "state": "superseded",
                    "version": checkpoint.version + 1,
                    "blocked_code": checkpoint.blocked_code,
                }),
                &now,
                Some(("human", user_id)),
                Some(format!(
                    "review-checkpoint-transition:{}:{}",
                    checkpoint.id,
                    expected_version + 1
                )),
            );
            return Ok(self
                .db
                .transition_review_checkpoint(TransitionReviewCheckpoint {
                    checkpoint_id: checkpoint.id,
                    expected_version,
                    state: ReviewCheckpointState::Superseded,
                    add_reasons: Vec::new(),
                    blocked_code: None,
                    blocked_detail_json: None,
                    cancel_review_work_units: false,
                    updated_at: now,
                    events: vec![event],
                })
                .await?);
        }
        if self
            .checkpoint_count(&checkpoint.task_id, &checkpoint.repo_id)
            .await?
            >= i64::from(policy.policy.max_checkpoints_per_repo)
        {
            return Err(coordination(
                CHECKPOINT_LIMIT,
                "the review checkpoint limit for this repository is exhausted",
            ));
        }
        let now = db::now_rfc3339();
        let key = format!("review-retry:{}:{expected_version}", checkpoint.id);
        let request = ReviewCheckpointRequestRecord {
            idempotency_key: key,
            source_kind: ReviewRequestSourceKind::Human,
            source_ref: user_id.to_owned(),
            reason: "retry blocked review checkpoint".to_owned(),
        };
        let old_event = review_event(
            "review.checkpoint_state_changed",
            "review_checkpoint",
            &checkpoint.id,
            &checkpoint.task_id,
            json!({
                "task_id": checkpoint.task_id,
                "repo_id": checkpoint.repo_id,
                "checkpoint_id": checkpoint.id,
                "from_state": "blocked",
                "state": "superseded",
                "version": checkpoint.version + 1,
                "blocked_code": checkpoint.blocked_code,
            }),
            &now,
            Some(("human", user_id)),
            Some(format!(
                "review-checkpoint-transition:{}:{}",
                checkpoint.id,
                expected_version + 1
            )),
        );
        let id = db::new_uuid_v4();
        let attempt = checkpoint.attempt.saturating_add(1);
        let queued = self
            .queue_target(
                &checkpoint.task_id,
                &checkpoint.repo_id,
                &head,
                ReviewCheckpointReason::Recheck,
                Some(request),
                Some(ActorRef::Human(user_id.to_owned())),
                Some((
                    checkpoint.id.clone(),
                    checkpoint.version,
                    attempt,
                    id,
                    vec![old_event],
                )),
                None,
            )
            .await?
            .ok_or_else(|| {
                coordination(NOTHING_TO_REVIEW, "the integrated head is already covered")
            })?;
        Ok(queued)
    }

    /// The recheck checkpoint of a report. It loads the exact parent
    /// checkpoint because the report row intentionally does not duplicate its
    /// head, policy revision, or attempt.
    pub async fn recheck_input(
        &self,
        report: &ReviewRangeReport,
        now: &str,
    ) -> Result<UpsertQueuedReviewCheckpoint> {
        let parent = self
            .db
            .get_review_checkpoint(&report.checkpoint_id)
            .await?
            .filter(|checkpoint| {
                checkpoint.task_id == report.task_id
                    && checkpoint.repo_id == report.repo_id
                    && checkpoint.range_id.as_deref() == Some(report.range_id.as_str())
            })
            .ok_or_else(|| {
                coordination(
                    "review.report.subject_mismatch",
                    "report checkpoint is stale",
                )
            })?;
        let range = self
            .db
            .get_review_range(&report.range_id)
            .await?
            .filter(|range| range.task_id == report.task_id && range.repo_id == report.repo_id)
            .ok_or_else(|| {
                coordination("review.report.subject_mismatch", "report range is stale")
            })?;
        let policy = self
            .db
            .get_current_task_review_policy(&report.task_id)
            .await?
            .ok_or_else(|| coordination(NOTHING_TO_REVIEW, "the Task has no review policy"))?;
        let attempt = self
            .next_attempt(
                &parent.task_id,
                &parent.repo_id,
                Some(&range.base_commit),
                &range.head_commit,
            )
            .await?;
        let id = format!("review-recheck:{}:{attempt}", parent.id);
        let event = checkpoint_created_event(
            &id,
            &parent.task_id,
            &parent.repo_id,
            ReviewCheckpointState::Queued,
            &[ReviewCheckpointReason::Recheck],
            attempt,
            now,
            None,
        );
        Ok(UpsertQueuedReviewCheckpoint {
            checkpoint: ReviewCheckpoint {
                id,
                task_id: parent.task_id,
                repo_id: parent.repo_id,
                state: ReviewCheckpointState::Queued,
                reasons: vec![ReviewCheckpointReason::Recheck],
                attempt,
                base_commit: Some(range.base_commit),
                head_commit: range.head_commit,
                range_id: None,
                policy_revision: policy.revision,
                blocked_code: None,
                blocked_detail_json: None,
                version: 1,
                created_at: now.to_owned(),
                updated_at: now.to_owned(),
            },
            request: None,
            supersede: Vec::new(),
            events: vec![event],
        })
    }

    async fn is_review_trigger(&self, event: &DomainEvent) -> Result<bool> {
        Ok(match event.event_type.as_str() {
            "work_unit.integration_succeeded"
            | "work_unit.repository_integration_changed"
            | "work_unit.completed"
            | "work_unit.cancelled"
            | "task.lifecycle_changed"
            | "task.review_policy_updated"
            | "task.repository_set_changed"
            | "task.coordinator_plan_materialized"
            | "task.coordinator_replanned"
            | "review.range_reported"
            | "review.findings_triaged"
            | "review.finding_resolved"
            | "review.checkpoint_state_changed" => true,
            "artifact.created" if event.entity_type == "artifact" => self
                .db
                .get_review_range_report(&event.entity_id)
                .await?
                .is_some(),
            _ => false,
        })
    }

    async fn automatic_trigger(
        &self,
        task_id: &str,
        repo_id: &str,
        policy: &TaskReviewPolicyRevision,
        rows: &ReviewCoverageRows,
        latest_head: &str,
    ) -> Result<Option<(ReviewCheckpointReason, String)>> {
        let coverage_service =
            ReviewCoverageService::new(Arc::clone(&self.db), self.workspace_root.clone());
        let coverage = coverage_service
            .compute(task_id, repo_id, Some(latest_head))
            .await?;
        let Some(frontier) = coverage
            .frontier_commit
            .as_deref()
            .or(coverage.base_commit.as_deref())
        else {
            if policy.policy.risk_class != db::ReviewRiskClass::R0
                && !self
                    .has_unfinished_repository_work(task_id, repo_id)
                    .await?
            {
                return Ok(Some((
                    ReviewCheckpointReason::Baseline,
                    latest_head.to_owned(),
                )));
            }
            return Ok(None);
        };

        if policy.policy.risk_class == db::ReviewRiskClass::R2 {
            for checkpoint in self.db.list_active_review_plan_checkpoints(task_id).await? {
                if !checkpoint.repository_ids.iter().any(|id| id == repo_id) {
                    continue;
                }
                let mut head = None;
                let mut all_integrated = true;
                for work_unit_id in &checkpoint.work_unit_ids {
                    let integration = rows
                        .integrations
                        .iter()
                        .filter(|row| {
                            row.work_unit_id == *work_unit_id
                                && row.outcome == WorkUnitRepositoryIntegrationOutcome::Success
                                && row.target_after_sha.is_some()
                        })
                        .max_by(|left, right| {
                            (&left.started_at, &left.id).cmp(&(&right.started_at, &right.id))
                        });
                    if let Some(integration) = integration {
                        head = integration.target_after_sha.as_deref();
                    } else {
                        all_integrated = false;
                        break;
                    }
                }
                if all_integrated {
                    if let Some(head) = head.filter(|head| *head != frontier) {
                        return Ok(Some((
                            ReviewCheckpointReason::AfterWorkUnit,
                            head.to_owned(),
                        )));
                    }
                }
            }
        }

        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
        let source = resolve_repo_source(&repo, &self.workspace_root).await?;
        if git::commit_exists(Path::new(&source), frontier).await?
            && git::commit_exists(Path::new(&source), latest_head).await?
        {
            let size = git::numstat(Path::new(&source), frontier, latest_head).await?;
            if size_threshold_reached(&size, &policy.policy.size_threshold) {
                return Ok(Some((
                    ReviewCheckpointReason::SizeThreshold,
                    latest_head.to_owned(),
                )));
            }
        }

        if !self
            .has_unfinished_repository_work(task_id, repo_id)
            .await?
            && frontier != latest_head
            && policy.policy.risk_class != db::ReviewRiskClass::R0
        {
            let reason = if rows.ranges.is_empty() {
                ReviewCheckpointReason::Baseline
            } else if matches!(
                TaskRepo::get_by_id(&*self.db, task_id, false)
                    .await?
                    .map(|task| task.lifecycle_state),
                Some(db::TaskLifecycleState::ReadyToMerge)
            ) {
                ReviewCheckpointReason::Final
            } else {
                ReviewCheckpointReason::Delta
            };
            return Ok(Some((reason, latest_head.to_owned())));
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    async fn queue_target(
        &self,
        task_id: &str,
        repo_id: &str,
        head: &str,
        reason: ReviewCheckpointReason,
        request: Option<ReviewCheckpointRequestRecord>,
        created_by: Option<ActorRef>,
        retry: Option<(String, i64, i64, String, Vec<CreateDomainEvent>)>,
        existing_checkpoint: Option<ReviewCheckpoint>,
    ) -> Result<Option<ReviewCheckpoint>> {
        let Some(task) = TaskRepo::get_by_id(&*self.db, task_id, false).await? else {
            return Err(coordination(
                NOTHING_TO_REVIEW,
                "review checkpoints require a current runnable Task",
            ));
        };
        let dispatch = TaskLifecycleRepo::get_task_dispatch(&*self.db, task_id).await?;
        if matches!(
            task.lifecycle_state,
            db::TaskLifecycleState::Done
                | db::TaskLifecycleState::Cancelled
                | db::TaskLifecycleState::Merging
        ) || dispatch.map_or(true, |dispatch| {
            dispatch.state != db::TaskDispatchState::Running
        }) {
            return Err(coordination(
                NOTHING_TO_REVIEW,
                "review checkpoints cannot be admitted while the Task is not running",
            ));
        }
        let Some(policy) = self.db.get_current_task_review_policy(task_id).await? else {
            return Err(coordination(
                NOTHING_TO_REVIEW,
                "the Task has no review policy",
            ));
        };
        let repositories = TaskRepo::list_repositories(&*self.db, task_id).await?;
        if !repositories.iter().any(|repository| {
            repository.repo_id == repo_id
                && matches!(
                    repository.role,
                    TaskRepositoryRole::Primary | TaskRepositoryRole::Affected
                )
        }) {
            return Err(coordination(
                "review.range.repo_not_writable",
                "review checkpoints require a current writable Task repository",
            ));
        }
        let rows = self.db.load_review_coverage_rows(task_id, repo_id).await?;
        if rows.integrations.iter().all(|integration| {
            integration.outcome != WorkUnitRepositoryIntegrationOutcome::Success
                || integration.target_after_sha.as_deref() != Some(head)
        }) {
            return Err(coordination(
                HEAD_NOT_INTEGRATED,
                "a range review can target only a successful integrated commit",
            ));
        }
        let coverage_service =
            ReviewCoverageService::new(Arc::clone(&self.db), self.workspace_root.clone());
        let coverage = coverage_service
            .compute(task_id, repo_id, Some(head))
            .await?;
        if existing_checkpoint.is_none()
            && reason != ReviewCheckpointReason::Recheck
            && (coverage.status == super::ReviewCoverageStatus::Unchanged
                || (coverage.frontier_commit.as_deref() == Some(head)
                    && coverage.ranges.iter().any(|range| range.valid_edge)))
        {
            return Ok(None);
        }

        if let Some(checkpoint) = existing_checkpoint.as_ref() {
            if checkpoint.task_id != task_id
                || checkpoint.repo_id != repo_id
                || checkpoint.head_commit != head
                || checkpoint.state != ReviewCheckpointState::Queued
            {
                return Err(ServiceError::Conflict(
                    "queued review checkpoint no longer matches its exact range".to_owned(),
                ));
            }
            if checkpoint.policy_revision != policy.revision {
                return Err(coordination(
                    "review.policy.version_conflict",
                    "the review policy changed before the queued recheck could be admitted",
                ));
            }
        }
        let base_commit = existing_checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.base_commit.clone())
            .or_else(|| {
                coverage
                    .frontier_commit
                    .as_deref()
                    .or(coverage.base_commit.as_deref())
                    .map(str::to_owned)
            });
        let mut blocked_code = coverage.reason_code.clone();
        let mut blocked_detail_json = None;
        let mut contributors = None;
        if blocked_code.is_none() {
            if let Some(base) = base_commit.as_deref() {
                let attribution =
                    ReviewAttribution::new(Arc::clone(&self.db), self.workspace_root.clone());
                let facts = attribution
                    .contributors(task_id, repo_id, base, head)
                    .await?;
                if !facts.unrecorded_commits.is_empty() {
                    blocked_code = Some(UNRECORDED_COMMITS.to_owned());
                    blocked_detail_json =
                        Some(json!({"commits": facts.unrecorded_commits}).to_string());
                } else {
                    contributors = Some(facts);
                }
            } else {
                blocked_code = Some(BASE_UNKNOWN.to_owned());
            }
        }

        let mut assigned_actor = None;
        let role = if let Some(facts) = contributors.as_ref() {
            match self
                .reviewer_assignment(task_id, repo_id, &policy.policy, &facts.contributors)
                .await?
            {
                Ok(actor) => {
                    assigned_actor = actor;
                    Some(policy.policy.reviewer_role.clone())
                }
                Err(()) => {
                    blocked_code = Some(REVIEWER_UNAVAILABLE.to_owned());
                    blocked_detail_json = None;
                    None
                }
            }
        } else {
            None
        };

        let now = db::now_rfc3339();
        let requested_attempt = retry
            .as_ref()
            .map(|(_, _, attempt, _, _)| *attempt)
            .or_else(|| {
                existing_checkpoint
                    .as_ref()
                    .map(|checkpoint| checkpoint.attempt)
            });
        let attempt = match requested_attempt {
            Some(attempt) => attempt,
            None => {
                self.next_attempt(task_id, repo_id, base_commit.as_deref(), head)
                    .await?
            }
        };
        if self.checkpoint_count(task_id, repo_id).await?
            >= i64::from(policy.policy.max_checkpoints_per_repo)
            && retry.is_none()
            && existing_checkpoint.is_none()
        {
            return Err(coordination(
                CHECKPOINT_LIMIT,
                "the review checkpoint limit for this repository is exhausted",
            ));
        }
        let checkpoint_id = retry
            .as_ref()
            .map(|(_, _, _, checkpoint_id, _)| checkpoint_id.clone())
            .or_else(|| {
                existing_checkpoint
                    .as_ref()
                    .map(|checkpoint| checkpoint.id.clone())
            })
            .unwrap_or_else(db::new_uuid_v4);
        let state = if blocked_code.is_some() {
            ReviewCheckpointState::Blocked
        } else {
            ReviewCheckpointState::Queued
        };
        if state == ReviewCheckpointState::Blocked
            && request.is_none()
            && retry.is_none()
            && existing_checkpoint.is_none()
        {
            let previous: Option<String> = sqlx::query_scalar(
                "SELECT id FROM review_checkpoint
                 WHERE task_id = ? AND repo_id = ? AND head_commit = ?
                   AND policy_revision = ? AND state = 'blocked'
                   AND blocked_code IS ? AND blocked_detail_json IS ?
                 ORDER BY attempt DESC, created_at DESC LIMIT 1",
            )
            .bind(task_id)
            .bind(repo_id)
            .bind(head)
            .bind(policy.revision)
            .bind(blocked_code.as_deref())
            .bind(blocked_detail_json.as_deref())
            .fetch_optional(self.db.pool())
            .await?;
            if previous.is_some() {
                return Ok(None);
            }
        }
        let source_actor = created_by
            .as_ref()
            .map(|actor| (actor.kind().to_string(), actor.id().to_owned()));
        let mut events = retry
            .as_ref()
            .map(|(_, _, _, _, events)| events.clone())
            .unwrap_or_default();
        if existing_checkpoint.is_none() {
            events.push(checkpoint_created_event(
                &checkpoint_id,
                task_id,
                repo_id,
                state,
                &[reason],
                attempt,
                &now,
                source_actor
                    .as_ref()
                    .map(|(kind, id)| (kind.as_str(), id.as_str())),
            ));
        }
        let checkpoint = existing_checkpoint.clone().unwrap_or(ReviewCheckpoint {
            id: checkpoint_id,
            task_id: task_id.to_owned(),
            repo_id: repo_id.to_owned(),
            state,
            reasons: vec![reason],
            attempt,
            base_commit: None,
            head_commit: head.to_owned(),
            range_id: None,
            policy_revision: policy.revision,
            blocked_code: blocked_code.clone(),
            blocked_detail_json: blocked_detail_json.clone(),
            version: 1,
            created_at: now.clone(),
            updated_at: now.clone(),
        });
        let supersede = retry
            .as_ref()
            .map(|(id, version, _, _, _)| vec![(id.clone(), *version)])
            .unwrap_or_default();
        let mut checkpoint = if existing_checkpoint.is_some() {
            checkpoint
        } else {
            self.db
                .upsert_queued_review_checkpoint(UpsertQueuedReviewCheckpoint {
                    checkpoint,
                    request,
                    supersede,
                    events,
                })
                .await?
        };
        if existing_checkpoint.is_some() && blocked_code.is_some() {
            let transition_event = review_event(
                "review.checkpoint_state_changed",
                "review_checkpoint",
                &checkpoint.id,
                task_id,
                json!({
                    "task_id": task_id,
                    "repo_id": repo_id,
                    "checkpoint_id": checkpoint.id.clone(),
                    "from_state": "queued",
                    "state": "blocked",
                    "version": checkpoint.version + 1,
                    "blocked_code": blocked_code.clone(),
                }),
                &now,
                None,
                Some(format!(
                    "review-checkpoint-transition:{}:{}",
                    checkpoint.id,
                    checkpoint.version + 1
                )),
            );
            checkpoint = self
                .db
                .transition_review_checkpoint(TransitionReviewCheckpoint {
                    checkpoint_id: checkpoint.id.clone(),
                    expected_version: checkpoint.version,
                    state: ReviewCheckpointState::Blocked,
                    add_reasons: Vec::new(),
                    blocked_code: blocked_code.clone(),
                    blocked_detail_json: blocked_detail_json.clone(),
                    cancel_review_work_units: false,
                    updated_at: now,
                    events: vec![transition_event],
                })
                .await?;
            self.emit_coverage_changed(task_id, repo_id).await?;
            return Ok(Some(checkpoint));
        }
        if checkpoint.state != ReviewCheckpointState::Queued {
            self.emit_coverage_changed(task_id, repo_id).await?;
            return Ok(Some(checkpoint));
        }
        if self.has_active_checkpoint(task_id, repo_id).await? {
            if existing_checkpoint.is_some() {
                return Ok(None);
            }
            return Err(coordination(
                CHECKPOINT_ACTIVE,
                "another review checkpoint is already active for this repository",
            ));
        }

        // Re-read after inserting the queued checkpoint: its coverage trigger
        // has advanced the revision used by activation's CAS.
        let rows_after_queue = self.db.load_review_coverage_rows(task_id, repo_id).await?;
        let base_commit = checkpoint
            .base_commit
            .clone()
            .or_else(|| {
                coverage
                    .frontier_commit
                    .as_deref()
                    .or(coverage.base_commit.as_deref())
                    .map(str::to_owned)
            })
            .ok_or_else(|| coordination(BASE_UNKNOWN, "the Task review base is unknown"))?;
        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
        let source = resolve_repo_source(&repo, &self.workspace_root).await?;
        let range = if let Some(existing) = rows_after_queue
            .ranges
            .iter()
            .find(|range| range.base_commit == base_commit && range.head_commit == head)
        {
            existing.clone()
        } else {
            let Some(integration) = rows_after_queue
                .integrations
                .iter()
                .filter(|integration| {
                    integration.outcome == WorkUnitRepositoryIntegrationOutcome::Success
                        && integration.target_after_sha.as_deref() == Some(head)
                })
                .max_by(|left, right| {
                    (&left.started_at, &left.id).cmp(&(&right.started_at, &right.id))
                })
            else {
                return Err(coordination(
                    HEAD_NOT_INTEGRATED,
                    "the integrated head changed while the checkpoint was being prepared",
                ));
            };
            let repository_set = TaskRepositorySetRepo::get_current(&*self.db, task_id)
                .await?
                .ok_or_else(|| {
                    coordination(BASE_UNKNOWN, "the Task repository set is unavailable")
                })?;
            let base_tree = git::resolve_tree(Path::new(&source), &base_commit).await?;
            let head_tree = git::resolve_tree(Path::new(&source), head).await?;
            let range_id = db::new_uuid_v4();
            let range_digest = canonical_digest(&CanonicalRangeDigest {
                task_id,
                repo_id,
                repository_set_revision: repository_set.revision,
                base_commit: &base_commit,
                head_commit: head,
                base_tree: &base_tree,
                head_tree: &head_tree,
                integration_workspace_id: &integration.target_workspace_id,
                head_integration_id: &integration.id,
            })?;
            ReviewRange {
                id: range_id,
                task_id: task_id.to_owned(),
                repo_id: repo_id.to_owned(),
                repository_set_revision: repository_set.revision,
                base_commit: base_commit.clone(),
                head_commit: head.to_owned(),
                base_tree,
                head_tree,
                range_digest,
                integration_workspace_id: integration.target_workspace_id.clone(),
                head_integration_id: integration.id.clone(),
                created_at: now.clone(),
            }
        };
        let facts = contributors.unwrap_or_else(|| super::attribution::RangeContributors {
            contributors: Vec::new(),
            unrecorded_commits: Vec::new(),
            upstream_commits: Vec::new(),
        });
        let range_contributors = facts
            .contributors
            .into_iter()
            .map(|mut contributor| {
                contributor.range_id = range.id.clone();
                contributor
            })
            .collect::<Vec<_>>();
        let open_findings =
            open_findings_for_head(&self.db, &self.workspace_root, task_id, repo_id, head).await?;
        let input_findings = open_findings
            .iter()
            .map(|finding| ReviewInputFinding {
                finding_id: finding.id.clone(),
                severity: finding.severity,
                digest: finding.digest.clone(),
            })
            .collect::<Vec<_>>();
        let input_findings_digest = canonical_digest(&input_findings)?;
        let reviewer_wu_id = format!("review:{}:1", checkpoint.id);
        let work_unit = CreateWorkUnit {
            id: reviewer_wu_id.clone(),
            task_id: task_id.to_owned(),
            parent_work_unit_id: None,
            title: format!("Review {}..{}", short_sha(&base_commit), short_sha(head)),
            scope: "Review this exact integrated repository range for findings and resolutions."
                .to_owned(),
            execution_instructions: None,
            role: role.unwrap_or_else(|| policy.policy.reviewer_role.clone()),
            assigned_actor,
            requires_integration: false,
            provenance: None,
            artifact_input_ids: Vec::new(),
            repository_ids: vec![repo_id.to_owned()],
            review_approval_gate_id: None,
            review_approval_line_id: None,
            created_by: ActorRef::Human(policy.created_by_user_id.clone()),
            created_at: now.clone(),
        };
        let work_unit_review_range = WorkUnitReviewRange {
            review_work_unit_id: reviewer_wu_id.clone(),
            task_id: task_id.to_owned(),
            checkpoint_id: checkpoint.id.clone(),
            range_id: range.id.clone(),
            repo_id: repo_id.to_owned(),
            input_findings,
            input_findings_digest,
            created_at: now.clone(),
        };
        let mut activation_events = vec![review_event(
            "review.checkpoint_state_changed",
            "review_checkpoint",
            &checkpoint.id,
            task_id,
            json!({
                "task_id": task_id,
                "repo_id": repo_id,
                "checkpoint_id": checkpoint.id,
                "from_state": "queued",
                "state": "pending",
                "version": checkpoint.version + 1,
                "blocked_code": Value::Null,
            }),
            &now,
            source_actor
                .as_ref()
                .map(|(kind, id)| (kind.as_str(), id.as_str())),
            Some(format!(
                "review-checkpoint-pending:{}:{}",
                checkpoint.id,
                checkpoint.version + 1
            )),
        )];
        activation_events.push(review_event(
            "work_unit.created",
            "work_unit",
            &reviewer_wu_id,
            task_id,
            json!({"version": 1, "status": "open"}),
            &now,
            Some(("human", policy.created_by_user_id.as_str())),
            Some(format!("work-unit-created:{reviewer_wu_id}")),
        ));
        checkpoint = self
            .db
            .activate_review_checkpoint(ActivateReviewCheckpoint {
                checkpoint_id: checkpoint.id.clone(),
                expected_version: checkpoint.version,
                expected_coverage_revision: rows_after_queue.state_revision,
                range,
                contributors: range_contributors,
                work_unit,
                review_range: work_unit_review_range,
                updated_at: now,
                events: activation_events,
            })
            .await?;
        self.emit_coverage_changed(task_id, repo_id).await?;
        Ok(Some(checkpoint))
    }

    async fn reviewer_assignment(
        &self,
        task_id: &str,
        _repo_id: &str,
        policy: &ReviewPolicyDocument,
        contributors: &[ReviewRangeContributor],
    ) -> Result<std::result::Result<Option<ActorRef>, ()>> {
        let Some(role) =
            TaskRoleRepo::get_by_task_and_role(&*self.db, task_id, &policy.reviewer_role).await?
        else {
            return Ok(Err(()));
        };
        let Some(configuration) =
            TaskRoleCapabilityConfigurationRepo::get_by_task_role_id(&*self.db, &role.id).await?
        else {
            return Ok(Err(()));
        };
        let capabilities = serde_json::from_str::<Value>(&configuration.configuration_json)
            .ok()
            .and_then(|value| value.get("capabilities").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        if !capabilities
            .iter()
            .any(|capability| capability.as_str() == Some("formal_review"))
        {
            return Ok(Err(()));
        }
        if policy.trigger_mode == ReviewTriggerMode::Manual {
            return Ok(Ok(None));
        }
        let memberships = RoleMembershipRepo::list_by_role(&*self.db, &role.id, false).await?;
        let contributor_humans = contributors
            .iter()
            .filter(|contributor| contributor.actor.kind() == ActorKind::Human)
            .map(|contributor| contributor.actor.id().to_owned())
            .collect::<BTreeSet<_>>();
        let mut eligible = memberships
            .into_iter()
            .filter(|membership| membership.status == RoleMembershipStatus::Active)
            .filter(|membership| {
                membership.actor_kind != ActorKind::Human
                    || !contributor_humans.contains(&membership.actor_id)
            })
            .map(|membership| membership.actor_ref())
            .filter(|actor| {
                policy.require_human == (actor.kind() == ActorKind::Human) || !policy.require_human
            })
            .collect::<Vec<_>>();
        eligible.sort_by(|left, right| {
            (left.kind().to_string(), left.id()).cmp(&(right.kind().to_string(), right.id()))
        });
        eligible.dedup();
        if eligible.len() == 1 {
            Ok(Ok(eligible.into_iter().next()))
        } else {
            Ok(Err(()))
        }
    }

    async fn authorize_request(
        &self,
        source: &ReviewRequestSource,
        task_id: &str,
        project_id: &str,
        policy: &ReviewPolicyDocument,
    ) -> Result<(ReviewRequestSourceKind, String, Option<ActorRef>)> {
        match source {
            ReviewRequestSource::Human { user_id } => {
                let is_admin = user_can_administer_project(
                    &self.db,
                    &ProjectRepo::get_by_id(&*self.db, project_id)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))?,
                    user_id,
                )
                .await?;
                let is_member = RoleMembershipRepo::list_by_task(&*self.db, task_id, false)
                    .await?
                    .into_iter()
                    .any(|(_, membership)| {
                        membership.status == RoleMembershipStatus::Active
                            && membership.actor_kind == ActorKind::Human
                            && membership.actor_id == *user_id
                    });
                if !is_admin && !is_member {
                    return Err(coordination(
                        REVIEW_NOT_PERMITTED,
                        "Human is not an active Task member",
                    ));
                }
                Ok((
                    ReviewRequestSourceKind::Human,
                    user_id.clone(),
                    Some(ActorRef::Human(user_id.clone())),
                ))
            }
            ReviewRequestSource::Coordinator {
                execution_id,
                request_id,
            } => {
                if !policy.coordinator_may_request {
                    return Err(coordination(
                        REVIEW_NOT_PERMITTED,
                        "the current review policy denies coordinator requests",
                    ));
                }
                let authorization = TaskCoordinationRepo::get_task_coordination(&*self.db, task_id)
                    .await?
                    .filter(|coordination| {
                        coordination.mode != TaskCoordinationMode::Manual
                            && coordination.revoked_at.is_none()
                            && !coordination.team_stale
                    })
                    .ok_or_else(|| {
                        coordination(
                            REVIEW_NOT_PERMITTED,
                            "Task coordination is not currently authorized",
                        )
                    })?;
                let request =
                    TaskCoordinationRepo::get_task_coordination_request(&*self.db, request_id)
                        .await?
                        .filter(|request| {
                            request.task_id == task_id
                                && request.authorization_version
                                    == authorization.authorization_version
                                && request.execution_id.as_deref() == Some(execution_id.as_str())
                                && !request.state.is_terminal()
                        })
                        .ok_or_else(|| {
                            coordination(REVIEW_NOT_PERMITTED, "coordinator request is stale")
                        })?;
                let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
                    .await?
                    .filter(|execution| {
                        execution.task_id == task_id
                            && execution.status == ExecutionStatus::Completed
                            && execution.actor_kind
                                == authorization.coordinator_actor.as_ref().map(ActorRef::kind)
                            && execution.actor_id.as_deref()
                                == authorization.coordinator_actor.as_ref().map(ActorRef::id)
                    })
                    .ok_or_else(|| {
                        coordination(
                            REVIEW_NOT_PERMITTED,
                            "execution is not the current coordinator",
                        )
                    })?;
                let _ = (request, execution);
                Ok((
                    ReviewRequestSourceKind::Coordinator,
                    request_id.clone(),
                    None,
                ))
            }
        }
    }

    async fn request_replay(
        &self,
        task_id: &str,
        idempotency_key: &str,
        source_kind: ReviewRequestSourceKind,
        source_ref: &str,
        reason: &str,
    ) -> Result<Option<ReviewCheckpoint>> {
        let row = sqlx::query(
            "SELECT r.checkpoint_id, r.source_kind, r.source_ref, r.reason
             FROM review_checkpoint_request r WHERE r.task_id = ? AND r.idempotency_key = ?",
        )
        .bind(task_id)
        .bind(idempotency_key)
        .fetch_optional(self.db.pool())
        .await?;
        let Some(row) = row else { return Ok(None) };
        let exact = row.try_get::<String, _>("source_kind")? == source_kind.to_string()
            && row.try_get::<String, _>("source_ref")? == source_ref
            && row.try_get::<String, _>("reason")? == reason;
        if !exact {
            return Err(ServiceError::Conflict(
                "review request idempotency key conflicts with an existing request".to_owned(),
            ));
        }
        let id: String = row.try_get("checkpoint_id")?;
        Ok(Some(
            self.db
                .get_review_checkpoint(&id)
                .await?
                .ok_or(db::DbError::NotFound)?,
        ))
    }

    async fn require_project_admin(&self, project_id: &str, user_id: &str) -> Result<()> {
        let project = ProjectRepo::get_by_id(&*self.db, project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))?;
        if user_can_administer_project(&self.db, &project, user_id).await? {
            Ok(())
        } else {
            Err(coordination(
                REVIEW_NOT_PERMITTED,
                "only a Project owner or admin may retry a checkpoint",
            ))
        }
    }

    async fn has_active_checkpoint(&self, task_id: &str, repo_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM review_checkpoint
             WHERE task_id = ? AND repo_id = ? AND state IN ('pending','in_review'))",
        )
        .bind(task_id)
        .bind(repo_id)
        .fetch_one(self.db.pool())
        .await?)
    }

    async fn checkpoint_count(&self, task_id: &str, repo_id: &str) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM review_checkpoint WHERE task_id = ? AND repo_id = ?",
        )
        .bind(task_id)
        .bind(repo_id)
        .fetch_one(self.db.pool())
        .await?)
    }

    async fn next_attempt(
        &self,
        task_id: &str,
        repo_id: &str,
        base: Option<&str>,
        head: &str,
    ) -> Result<i64> {
        let max: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(c.attempt), 0) FROM review_checkpoint c
             LEFT JOIN review_range r ON r.id = c.range_id
             WHERE c.task_id = ? AND c.repo_id = ? AND c.head_commit = ?
               AND COALESCE(c.base_commit, r.base_commit) IS ?",
        )
        .bind(task_id)
        .bind(repo_id)
        .bind(head)
        .bind(base)
        .fetch_one(self.db.pool())
        .await?;
        Ok(max.saturating_add(1).max(1))
    }

    async fn has_unfinished_repository_work(&self, task_id: &str, repo_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM work_unit w
                JOIN work_unit_repository wr ON wr.work_unit_id = w.id AND wr.task_id = w.task_id
                WHERE w.task_id = ? AND wr.repo_id = ? AND w.requires_integration = 1
                  AND w.status = 'open')
             OR EXISTS(
                SELECT 1 FROM work_unit_repository_integration i
                WHERE i.task_id = ? AND i.repo_id = ? AND i.outcome = 'running')",
        )
        .bind(task_id)
        .bind(repo_id)
        .bind(task_id)
        .bind(repo_id)
        .fetch_one(self.db.pool())
        .await?)
    }

    async fn supersede_stale_unstarted_pending(
        &self,
        task_id: &str,
        repo_id: &str,
        current_head: &str,
    ) -> Result<()> {
        let row = sqlx::query(
            "SELECT c.id, c.version, c.head_commit FROM review_checkpoint c
             JOIN work_unit_review_range r ON r.checkpoint_id = c.id
             WHERE c.task_id = ? AND c.repo_id = ? AND c.state = 'pending'
               AND c.head_commit != ?
               AND NOT EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = r.review_work_unit_id)
             LIMIT 1",
        )
        .bind(task_id)
        .bind(repo_id)
        .bind(current_head)
        .fetch_optional(self.db.pool())
        .await?;
        let Some(row) = row else { return Ok(()) };
        let id: String = row.try_get("id")?;
        let version: i64 = row.try_get("version")?;
        let now = db::now_rfc3339();
        let event = review_event(
            "review.checkpoint_state_changed",
            "review_checkpoint",
            &id,
            task_id,
            json!({
                "task_id": task_id,
                "repo_id": repo_id,
                "checkpoint_id": id,
                "from_state": "pending",
                "state": "superseded",
                "version": version + 1,
                "blocked_code": Value::Null,
            }),
            &now,
            None,
            Some(format!("review-checkpoint-transition:{id}:{}", version + 1)),
        );
        self.db
            .transition_review_checkpoint(TransitionReviewCheckpoint {
                checkpoint_id: id,
                expected_version: version,
                state: ReviewCheckpointState::Superseded,
                add_reasons: Vec::new(),
                blocked_code: None,
                blocked_detail_json: None,
                cancel_review_work_units: true,
                updated_at: now,
                events: vec![event],
            })
            .await?;
        Ok(())
    }

    async fn emit_coverage_changed(&self, task_id: &str, repo_id: &str) -> Result<()> {
        let coverage =
            ReviewCoverageService::new(Arc::clone(&self.db), self.workspace_root.clone())
                .compute(task_id, repo_id, None)
                .await?;
        let event = review_event(
            "review.coverage_changed",
            "task",
            task_id,
            task_id,
            json!({
                "task_id": task_id,
                "repo_id": repo_id,
                "revision": coverage.revision,
                "status": coverage.status.as_str(),
            }),
            &db::now_rfc3339(),
            None,
            Some(format!(
                "review-coverage:{task_id}:{repo_id}:{}",
                coverage.revision
            )),
        );
        let committed = DomainEventRepo::append_event(&*self.db, event).await?;
        if let Some(event_bus) = self.event_bus.as_deref() {
            DomainEventService::publish_committed_hint(event_bus, &committed);
        }
        Ok(())
    }
}

async fn open_findings_for_head(
    db: &Arc<SqliteDb>,
    workspace_root: &Path,
    task_id: &str,
    repo_id: &str,
    head: &str,
) -> Result<Vec<ReviewFinding>> {
    let rows = db.load_review_coverage_rows(task_id, repo_id).await?;
    let coverage = ReviewCoverageService::new(Arc::clone(db), workspace_root.to_path_buf())
        .compute(task_id, repo_id, Some(head))
        .await?;
    let open = coverage
        .open_finding_ids
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut findings = rows
        .findings
        .into_iter()
        .filter(|finding| open.contains(&finding.id))
        .collect::<Vec<_>>();
    findings.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(findings)
}

fn latest_success(
    integrations: &[WorkUnitRepositoryIntegration],
) -> Option<&WorkUnitRepositoryIntegration> {
    integrations
        .iter()
        .filter(|row| {
            row.outcome == WorkUnitRepositoryIntegrationOutcome::Success
                && row.target_after_sha.is_some()
        })
        .max_by(|left, right| (&left.started_at, &left.id).cmp(&(&right.started_at, &right.id)))
}

fn checkpoint_created_event(
    checkpoint_id: &str,
    task_id: &str,
    repo_id: &str,
    state: ReviewCheckpointState,
    reasons: &[ReviewCheckpointReason],
    attempt: i64,
    now: &str,
    actor: Option<(&str, &str)>,
) -> CreateDomainEvent {
    review_event(
        "review.checkpoint_created",
        "review_checkpoint",
        checkpoint_id,
        task_id,
        json!({
            "task_id": task_id,
            "repo_id": repo_id,
            "checkpoint_id": checkpoint_id,
            "state": state.to_string(),
            "reasons": reasons.iter().map(|reason| reason.to_string()).collect::<Vec<_>>(),
            "attempt": attempt,
        }),
        now,
        actor,
        Some(format!("review-checkpoint-created:{checkpoint_id}")),
    )
}

fn review_event(
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    task_id: &str,
    payload: Value,
    now: &str,
    actor: Option<(&str, &str)>,
    dedupe_key: Option<String>,
) -> CreateDomainEvent {
    CreateDomainEvent {
        id: db::new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: entity_type.to_owned(),
        entity_id: entity_id.to_owned(),
        actor_type: actor.map(|(kind, _)| kind).unwrap_or("system").to_owned(),
        actor_id: actor.map(|(_, id)| id.to_owned()),
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

fn coordination(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::Coordination {
        code,
        message: message.into(),
    }
}

fn error_code(error: &ServiceError) -> Option<&'static str> {
    match error {
        ServiceError::Coordination { code, .. } => Some(*code),
        _ => None,
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

fn size_threshold_reached(size: &git::NumstatSummary, threshold: &db::ReviewSizeThreshold) -> bool {
    size.changed_lines >= threshold.changed_lines || size.changed_files >= threshold.changed_files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_range_scheduler_events_do_not_embed_review_text_or_commits() {
        let event = checkpoint_created_event(
            "checkpoint-1",
            "task-1",
            "repo-1",
            ReviewCheckpointState::Pending,
            &[ReviewCheckpointReason::Baseline],
            1,
            "2026-11-27T00:00:00Z",
            None,
        );
        let payload: Value = serde_json::from_str(&event.payload_json).expect("JSON payload");
        assert_eq!(payload["task_id"], "task-1");
        assert_eq!(payload["repo_id"], "repo-1");
        assert!(payload.get("reason").is_none());
        assert!(payload.get("head_commit").is_none());
    }

    #[test]
    fn review_range_scheduler_size_trigger_uses_inclusive_line_and_file_limits() {
        let threshold = db::ReviewSizeThreshold {
            changed_lines: 800,
            changed_files: 25,
        };
        assert!(size_threshold_reached(
            &git::NumstatSummary {
                changed_lines: 800,
                changed_files: 1,
            },
            &threshold,
        ));
        assert!(size_threshold_reached(
            &git::NumstatSummary {
                changed_lines: 0,
                changed_files: 25,
            },
            &threshold,
        ));
        assert!(!size_threshold_reached(
            &git::NumstatSummary {
                changed_lines: 799,
                changed_files: 24,
            },
            &threshold,
        ));
    }
}
