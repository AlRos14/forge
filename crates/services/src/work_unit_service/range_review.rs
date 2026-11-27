//! Admission facts of a commit-range reviewer WorkUnit
//! (`docs/plans/review-ranges.md`, C0.7 and C0.11 lane C).
//!
//! A range reviewer WorkUnit reviews one frozen range of one repository. Its
//! WorkspaceGroup is read-only (range head plus pinned context repositories),
//! it never receives a WorkspaceLease, and every use of the checkout verifies
//! that nothing was written.

use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use db::{
    new_uuid_v4, now_rfc3339, ActorKind, ActorRef, AgentRepo, CreateDomainEvent,
    ReviewCheckpointRepo, ReviewCheckpointState, ReviewRangeContributor, RoleMembershipRepo,
    RoleMembershipStatus, SqliteDb, TransitionReviewCheckpoint, WorkUnitReviewRange,
    WorkUnitWorkspaceRepo, WorkspaceLeaseRepo, WorkspaceRepo, WorkspaceScopeKind, WorkspaceStatus,
};
use serde::Serialize;

use crate::{Result, ServiceError};

/// One verified read-only repository of a range review checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RangeCheckout {
    pub repo_id: String,
    pub workspace_id: String,
    pub commit_sha: String,
    pub snapshot_digest: String,
}

/// The frozen range subject of a WorkUnit, when it is a range reviewer.
pub(crate) async fn frozen_range(
    db: &SqliteDb,
    work_unit_id: &str,
) -> Result<Option<WorkUnitReviewRange>> {
    Ok(ReviewCheckpointRepo::get_work_unit_review_range(db, work_unit_id).await?)
}

/// A range reviewer may be admitted only while its checkpoint is active.
pub(crate) async fn range_review_ready(
    db: &SqliteDb,
    frozen: &WorkUnitReviewRange,
) -> Result<bool> {
    Ok(
        ReviewCheckpointRepo::get_review_checkpoint(db, &frozen.checkpoint_id)
            .await?
            .is_some_and(|checkpoint| {
                checkpoint.task_id == frozen.task_id
                    && checkpoint.range_id.as_deref() == Some(frozen.range_id.as_str())
                    && matches!(
                        checkpoint.state,
                        ReviewCheckpointState::Pending | ReviewCheckpointState::InReview
                    )
            }),
    )
}

/// A Human contributor can never review the range. A contributing Agent is
/// admitted only with a session that lacks the producer context, which SQLite
/// and the report writer enforce on the exact Execution.
pub(crate) fn actor_may_review_range(
    contributors: &[ReviewRangeContributor],
    actor: &ActorRef,
) -> bool {
    match actor {
        ActorRef::Agent(_) => true,
        ActorRef::Human(_) => !contributed(contributors, actor),
    }
}

pub(crate) fn contributed(contributors: &[ReviewRangeContributor], actor: &ActorRef) -> bool {
    contributors
        .iter()
        .any(|contributor| &contributor.actor == actor)
}

/// Range independence for allocation. WorkUnits without a range subject are not
/// constrained here.
pub(crate) async fn range_allocation_is_independent(
    db: &SqliteDb,
    work_unit_id: &str,
    actor: &ActorRef,
) -> Result<bool> {
    let Some(frozen) = frozen_range(db, work_unit_id).await? else {
        return Ok(true);
    };
    let contributors =
        ReviewCheckpointRepo::list_review_range_contributors(db, &frozen.range_id).await?;
    Ok(actor_may_review_range(&contributors, actor))
}

/// Whether an automatic route may select this Actor: it must not have
/// contributed at all, because automatic admission opens no isolated session.
pub(crate) async fn actor_did_not_contribute(
    db: &SqliteDb,
    work_unit_id: &str,
    actor: &ActorRef,
) -> Result<bool> {
    let Some(frozen) = frozen_range(db, work_unit_id).await? else {
        return Ok(true);
    };
    let contributors =
        ReviewCheckpointRepo::list_review_range_contributors(db, &frozen.range_id).await?;
    Ok(!contributed(&contributors, actor))
}

/// Active members of the reviewer TaskRole that did not contribute to the
/// range, ordered and unique. Paused or missing Agents are excluded.
pub(crate) async fn eligible_range_reviewers(
    db: &SqliteDb,
    task_id: &str,
    reviewer_role: &str,
    contributors: &[ReviewRangeContributor],
) -> Result<Vec<ActorRef>> {
    if !crate::role_capabilities::task_role_name_has_capability(
        db,
        task_id,
        reviewer_role,
        api_types::TaskRoleCapability::FormalReview,
    )
    .await?
    {
        return Ok(Vec::new());
    }
    let mut eligible = BTreeMap::new();
    for (role, membership) in RoleMembershipRepo::list_by_task(db, task_id, false).await? {
        if role.role != reviewer_role || membership.status != RoleMembershipStatus::Active {
            continue;
        }
        let actor = match membership.actor_kind {
            ActorKind::Human => ActorRef::Human(membership.actor_id.clone()),
            ActorKind::Agent => {
                if AgentRepo::get_by_id(db, &membership.actor_id)
                    .await?
                    .is_none_or(|agent| agent.paused)
                {
                    continue;
                }
                ActorRef::Agent(membership.actor_id.clone())
            }
        };
        if !contributed(contributors, &actor) {
            eligible.insert((actor.kind().to_string(), actor.id().to_owned()), actor);
        }
    }
    Ok(eligible.into_values().collect())
}

/// `trigger_mode=auto` assigns only when exactly one member is eligible.
pub(crate) fn sole_reviewer(mut eligible: Vec<ActorRef>) -> Option<ActorRef> {
    if eligible.len() == 1 {
        eligible.pop()
    } else {
        None
    }
}

/// Pinned context repositories of the reviewer WorkUnit's repository-set
/// revision.
async fn pinned_context_repository_ids(db: &SqliteDb, work_unit_id: &str) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT member.repo_id
         FROM work_unit_repository_set_revision pin
         JOIN task_repository_set_revision_repo member
           ON member.task_id = pin.task_id AND member.revision = pin.revision
          AND member.role = 'context'
         WHERE pin.work_unit_id = ?
         ORDER BY member.repo_id",
    )
    .bind(work_unit_id)
    .fetch_all(db.pool())
    .await?)
}

/// The exact repository set of a formal-review WorkspaceGroup: the range
/// repository plus pinned context for a range review, otherwise the frozen
/// result subject repositories.
pub(crate) async fn review_group_repository_ids(
    db: &SqliteDb,
    work_unit_id: &str,
) -> Result<HashSet<String>> {
    if let Some(frozen) = frozen_range(db, work_unit_id).await? {
        let mut ids = pinned_context_repository_ids(db, work_unit_id)
            .await?
            .into_iter()
            .collect::<HashSet<_>>();
        ids.insert(frozen.repo_id);
        return Ok(ids);
    }
    Ok(
        db::WorkUnitRepo::list_review_subject_repositories(db, work_unit_id)
            .await?
            .into_iter()
            .map(|repository| repository.repo_id)
            .collect(),
    )
}

/// Whether every review repository is fully frozen to an exact commit.
pub(crate) async fn review_group_is_frozen(db: &SqliteDb, work_unit_id: &str) -> Result<bool> {
    if frozen_range(db, work_unit_id).await?.is_some() {
        return Ok(true);
    }
    Ok(
        db::WorkUnitRepo::list_review_subject_repositories(db, work_unit_id)
            .await?
            .iter()
            .all(|repository| {
                repository.result_execution_id.is_some()
                    && repository.result_commit_sha.is_some()
                    && repository.result_workspace_id.is_some()
            }),
    )
}

/// Verify the complete read-only checkout of a range reviewer: exact members,
/// detached at their pinned commit, clean, and without any active lease.
pub(crate) async fn verify_range_checkout(
    db: &SqliteDb,
    task_id: &str,
    work_unit_id: &str,
) -> Result<Vec<RangeCheckout>> {
    let frozen = frozen_range(db, work_unit_id)
        .await?
        .filter(|frozen| frozen.task_id == task_id)
        .ok_or_else(|| mismatch("WorkUnit has no frozen review range in this Task"))?;
    let range = ReviewCheckpointRepo::get_review_range(db, &frozen.range_id)
        .await?
        .filter(|range| range.task_id == task_id && range.repo_id == frozen.repo_id)
        .ok_or_else(|| mismatch("review range is unavailable"))?;
    let group = WorkspaceRepo::get_group_by_work_unit_id(db, work_unit_id)
        .await?
        .filter(|group| group.task_id == task_id && group.work_unit_id == work_unit_id)
        .ok_or_else(|| mismatch("range review has no read-only WorkspaceGroup"))?;
    let mut expected = review_group_repository_ids(db, work_unit_id).await?;
    let workspaces = WorkUnitWorkspaceRepo::list_by_work_unit_id(db, work_unit_id).await?;
    if workspaces.len() != expected.len() {
        return Err(mismatch(
            "range review WorkspaceGroup does not contain its exact repository set",
        ));
    }
    if !WorkspaceLeaseRepo::list_active_for_work_unit(db, work_unit_id)
        .await?
        .is_empty()
    {
        return Err(mismatch("range review must not hold a WorkspaceLease"));
    }
    let mut checkout = Vec::with_capacity(workspaces.len());
    for workspace in workspaces {
        if !expected.remove(&workspace.repo_id) {
            return Err(mismatch(
                "range review WorkspaceGroup contains an unexpected repository",
            ));
        }
        let scope = WorkUnitWorkspaceRepo::get_scope_by_id(db, &workspace.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("workspace scope", workspace.id.clone()))?;
        let commit = if workspace.repo_id == range.repo_id {
            range.head_commit.clone()
        } else {
            sqlx::query_scalar::<_, String>(
                "SELECT commit_sha FROM task_repository_context_snapshot
                 WHERE task_id = ? AND repo_id = ?",
            )
            .bind(task_id)
            .bind(&workspace.repo_id)
            .fetch_optional(db.pool())
            .await?
            .ok_or_else(|| mismatch("context repository has no immutable Task snapshot"))?
        };
        if workspace.task_id != task_id
            || workspace.status != WorkspaceStatus::Ready
            || scope.task_id != task_id
            || scope.kind != WorkspaceScopeKind::WorkUnit
            || scope.work_unit_id.as_deref() != Some(work_unit_id)
            || scope.workspace_group_id.as_deref() != Some(group.id.as_str())
            || workspace.before_sha.as_deref() != Some(commit.as_str())
            || workspace.branch != format!("detached:{commit}")
        {
            return Err(mismatch(format!(
                "repository {} is not an exact read-only member of the range review",
                workspace.repo_id
            )));
        }
        let path = Path::new(&workspace.worktree_path);
        let head = git::get_current_sha(path).await?;
        if head != commit || !git::is_worktree_clean(path).await? {
            return Err(mismatch(format!(
                "read-only review repository {} changed from pinned commit {commit}",
                workspace.repo_id
            )));
        }
        let snapshot_digest =
            crate::ValidationService::snapshot_digest(&workspace.worktree_path).await?;
        checkout.push(RangeCheckout {
            repo_id: workspace.repo_id,
            workspace_id: workspace.id,
            commit_sha: commit,
            snapshot_digest,
        });
    }
    checkout.sort_by(|left, right| left.repo_id.cmp(&right.repo_id));
    Ok(checkout)
}

/// Move the checkpoint to `in_review` once a reviewer attempt is admitted. The
/// admitted Execution is already authoritative, so a concurrent transition is
/// not an error.
pub(crate) async fn mark_range_review_started(db: &SqliteDb, work_unit_id: &str) -> Result<()> {
    let Some(frozen) = frozen_range(db, work_unit_id).await? else {
        return Ok(());
    };
    let Some(checkpoint) =
        ReviewCheckpointRepo::get_review_checkpoint(db, &frozen.checkpoint_id).await?
    else {
        return Ok(());
    };
    if checkpoint.state != ReviewCheckpointState::Pending {
        return Ok(());
    }
    let now = now_rfc3339();
    let event = CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: "review.checkpoint_state_changed".to_owned(),
        entity_type: "review_checkpoint".to_owned(),
        entity_id: checkpoint.id.clone(),
        actor_type: "system".to_owned(),
        actor_id: None,
        scope_type: "task".to_owned(),
        scope_id: checkpoint.task_id.clone(),
        correlation_id: checkpoint.id.clone(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: Some(format!("review-checkpoint-in-review:{}", checkpoint.id)),
        payload_json: serde_json::json!({
            "task_id": checkpoint.task_id,
            "repo_id": checkpoint.repo_id,
            "checkpoint_id": checkpoint.id,
            "from_state": checkpoint.state.to_string(),
            "state": ReviewCheckpointState::InReview.to_string(),
            "version": checkpoint.version + 1,
            "blocked_code": null,
        })
        .to_string(),
        created_at: now.clone(),
    };
    match ReviewCheckpointRepo::transition_review_checkpoint(
        db,
        TransitionReviewCheckpoint {
            checkpoint_id: checkpoint.id,
            expected_version: checkpoint.version,
            state: ReviewCheckpointState::InReview,
            add_reasons: Vec::new(),
            blocked_code: None,
            blocked_detail_json: None,
            cancel_review_work_units: false,
            updated_at: now,
            events: vec![event],
        },
    )
    .await
    {
        Ok(_) | Err(db::DbError::VersionConflict) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn mismatch(message: impl Into<String>) -> ServiceError {
    ServiceError::Coordination {
        code: "review.report.subject_mismatch",
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contributor(actor: ActorRef) -> ReviewRangeContributor {
        ReviewRangeContributor {
            id: "contributor".to_owned(),
            task_id: "task".to_owned(),
            range_id: "range".to_owned(),
            work_unit_id: Some("work".to_owned()),
            execution_id: Some("execution".to_owned()),
            attribution_id: None,
            actor,
            created_at: "2026-11-27T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn review_range_independence_excludes_human_contributors() {
        let contributors = vec![
            contributor(ActorRef::Human("producer".to_owned())),
            contributor(ActorRef::Agent("agent".to_owned())),
        ];
        assert!(!actor_may_review_range(
            &contributors,
            &ActorRef::Human("producer".to_owned())
        ));
        assert!(actor_may_review_range(
            &contributors,
            &ActorRef::Human("reviewer".to_owned())
        ));
        // Allowed for allocation; the exact Execution still needs an isolated session.
        assert!(actor_may_review_range(
            &contributors,
            &ActorRef::Agent("agent".to_owned())
        ));
        assert!(contributed(
            &contributors,
            &ActorRef::Agent("agent".to_owned())
        ));
    }

    #[test]
    fn review_range_auto_assignment_requires_exactly_one_reviewer() {
        let first = ActorRef::Agent("first".to_owned());
        assert_eq!(sole_reviewer(Vec::new()), None);
        assert_eq!(sole_reviewer(vec![first.clone()]), Some(first.clone()));
        assert_eq!(
            sole_reviewer(vec![first, ActorRef::Human("second".to_owned())]),
            None
        );
    }
}
