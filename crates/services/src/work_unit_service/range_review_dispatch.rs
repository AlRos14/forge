//! Reviewer assignment and read-only workspace preparation for commit-range
//! reviewer WorkUnits.

use std::path::Path;

use db::{
    new_uuid_v4, now_rfc3339, ActorRef, CreateDomainEvent, RepoRepo, ReviewCheckpointRepo,
    ReviewCheckpointState, ReviewPolicyRepo, ReviewTriggerMode, Task, TaskRepo, TaskRepositoryRole,
    TransitionReviewCheckpoint, WorkUnit, WorkUnitRepo, WorkUnitReviewRange,
};

use super::{invalid, not_found, range_review, WorkUnitService, WorkspaceGroupMember};
use crate::{
    collaboration_service::CollaborationActorSource, task_service::workspace::resolve_repo_source,
    Result,
};

impl WorkUnitService {
    /// The range repository detached at the range head, plus the pinned context
    /// repositories. Every member is read-only.
    pub(super) async fn range_review_group_members(
        &self,
        task: &Task,
        frozen: &WorkUnitReviewRange,
    ) -> Result<Vec<WorkspaceGroupMember>> {
        let range = ReviewCheckpointRepo::get_review_range(&*self.db, &frozen.range_id)
            .await?
            .filter(|range| range.task_id == task.id && range.repo_id == frozen.repo_id)
            .ok_or_else(|| range_review::mismatch("review range is unavailable"))?;
        let writable = TaskRepo::list_repositories(&*self.db, &task.id)
            .await?
            .into_iter()
            .find(|repository| {
                repository.repo_id == range.repo_id
                    && matches!(
                        repository.role,
                        TaskRepositoryRole::Primary | TaskRepositoryRole::Affected
                    )
            })
            .ok_or_else(|| invalid("range repository is not writable in the Task scope"))?;
        let repo = RepoRepo::get_by_id(&*self.db, &range.repo_id)
            .await?
            .ok_or_else(|| not_found("repo", range.repo_id.clone()))?;
        let repo_path = resolve_repo_source(&repo, &self.workspace_root).await?;
        git::commit_parents(Path::new(&repo_path), &range.head_commit)
            .await
            .map_err(|_| invalid("range head commit is unavailable in its repository"))?;
        let mut members = vec![WorkspaceGroupMember {
            repo,
            role: writable.role,
            base_sha: range.head_commit.clone(),
            read_only: true,
        }];
        let mut context_ids =
            range_review::review_group_repository_ids(&self.db, &frozen.review_work_unit_id)
                .await?
                .into_iter()
                .filter(|repo_id| repo_id != &range.repo_id)
                .collect::<Vec<_>>();
        context_ids.sort();
        for repo_id in context_ids {
            let repo = RepoRepo::get_by_id(&*self.db, &repo_id)
                .await?
                .ok_or_else(|| not_found("repo", repo_id.clone()))?;
            let repo_path = resolve_repo_source(&repo, &self.workspace_root).await?;
            let observed_sha = git::get_current_sha(Path::new(&repo_path)).await?;
            let pin = TaskRepo::pin_context_repository_snapshot(
                &*self.db,
                db::CreateTaskRepositoryContextSnapshot {
                    task_id: task.id.clone(),
                    repo_id: repo.id.clone(),
                    commit_sha: observed_sha,
                    captured_at: now_rfc3339(),
                },
            )
            .await?;
            git::commit_parents(Path::new(&repo_path), &pin.commit_sha)
                .await
                .map_err(|_| invalid("pinned context repository commit is unavailable"))?;
            members.push(WorkspaceGroupMember {
                repo,
                role: TaskRepositoryRole::Context,
                base_sha: pin.commit_sha,
                read_only: true,
            });
        }
        Ok(members)
    }

    /// Assign `auto` range reviewers and prepare the read-only group of every
    /// assigned Agent reviewer. A Human reviewer's group is prepared by the
    /// review submission.
    pub(crate) async fn prepare_range_review_workspaces(
        &self,
        task_id: &str,
        preparation_user: Option<&str>,
    ) -> Result<Vec<String>> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT unit.id FROM work_unit unit
             JOIN work_unit_review_range range_unit ON range_unit.review_work_unit_id = unit.id
             JOIN review_checkpoint checkpoint ON checkpoint.id = range_unit.checkpoint_id
             WHERE unit.task_id = ? AND unit.status = 'open'
               AND checkpoint.state IN ('pending', 'in_review')
               AND NOT EXISTS (SELECT 1 FROM execution
                               WHERE work_unit_id = unit.id AND status = 'running')
               AND NOT EXISTS (SELECT 1 FROM task_dispatch
                               WHERE task_id = unit.task_id AND state IN ('paused', 'cancelled'))
               AND NOT EXISTS (SELECT 1 FROM task_lifecycle
                               WHERE task_id = unit.task_id
                                 AND state IN ('cancelled', 'done', 'merging'))
             ORDER BY unit.id",
        )
        .bind(task_id)
        .fetch_all(self.db.pool())
        .await?;
        let mut blockers = Vec::new();
        for id in ids {
            if let Err(error) = self.prepare_range_review(&id, preparation_user).await {
                blockers.push(format!("range_review_preparation_failed: {id}: {error}"));
            }
        }
        Ok(blockers)
    }

    async fn prepare_range_review(&self, id: &str, preparation_user: Option<&str>) -> Result<()> {
        let Some(unit) = WorkUnitRepo::get_by_id(&*self.db, id).await? else {
            return Ok(());
        };
        let user_id = match (preparation_user, &unit.created_by) {
            (Some(user_id), _) => user_id.to_owned(),
            (None, ActorRef::Human(user_id)) => user_id.clone(),
            (None, ActorRef::Agent(_)) => {
                return Err(invalid(
                    "range review preparation requires a Human authority",
                ));
            }
        };
        let source = CollaborationActorSource::Human(user_id);
        let unit = if unit.assigned_actor.is_some() {
            unit
        } else {
            match self.assign_range_reviewer(source.clone(), unit).await? {
                Some(unit) => unit,
                None => return Ok(()),
            }
        };
        if !matches!(unit.assigned_actor, Some(ActorRef::Agent(_)))
            || !self.readiness(source.clone(), &unit.id).await?.runnable
        {
            return Ok(());
        }
        self.prepare_workspace_group(source, &unit.id).await?;
        Ok(())
    }

    /// `manual` leaves the WorkUnit for a Human to allocate. `auto` assigns the
    /// only eligible independent member, otherwise blocks the checkpoint.
    async fn assign_range_reviewer(
        &self,
        source: CollaborationActorSource,
        unit: WorkUnit,
    ) -> Result<Option<WorkUnit>> {
        let Some(frozen) = range_review::frozen_range(&self.db, &unit.id).await? else {
            return Ok(None);
        };
        let automatic = ReviewPolicyRepo::get_current_task_review_policy(&*self.db, &unit.task_id)
            .await?
            .is_some_and(|policy| policy.policy.trigger_mode == ReviewTriggerMode::Auto);
        if !automatic {
            return Ok(None);
        }
        let contributors =
            ReviewCheckpointRepo::list_review_range_contributors(&*self.db, &frozen.range_id)
                .await?;
        let eligible = range_review::eligible_range_reviewers(
            &self.db,
            &unit.task_id,
            &unit.role,
            &contributors,
        )
        .await?;
        if let Some(reviewer) = range_review::sole_reviewer(eligible) {
            return Ok(Some(
                self.allocate(
                    source,
                    &unit.id,
                    unit.version,
                    unit.role.clone(),
                    Some(reviewer),
                )
                .await?,
            ));
        }
        self.block_unassignable_checkpoint(&frozen).await?;
        Ok(None)
    }

    async fn block_unassignable_checkpoint(&self, frozen: &WorkUnitReviewRange) -> Result<()> {
        let Some(checkpoint) =
            ReviewCheckpointRepo::get_review_checkpoint(&*self.db, &frozen.checkpoint_id).await?
        else {
            return Ok(());
        };
        // A checkpoint already under review is never cancelled here.
        if checkpoint.state != ReviewCheckpointState::Pending {
            return Ok(());
        }
        let code = "review.checkpoint.reviewer_unavailable";
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
            dedupe_key: Some(format!("review-checkpoint-unassignable:{}", checkpoint.id)),
            payload_json: serde_json::json!({
                "task_id": checkpoint.task_id,
                "repo_id": checkpoint.repo_id,
                "checkpoint_id": checkpoint.id,
                "from_state": checkpoint.state.to_string(),
                "state": ReviewCheckpointState::Blocked.to_string(),
                "version": checkpoint.version + 1,
                "blocked_code": code,
            })
            .to_string(),
            created_at: now.clone(),
        };
        ReviewCheckpointRepo::transition_review_checkpoint(
            &*self.db,
            TransitionReviewCheckpoint {
                checkpoint_id: checkpoint.id,
                expected_version: checkpoint.version,
                state: ReviewCheckpointState::Blocked,
                add_reasons: Vec::new(),
                blocked_code: Some(code.to_owned()),
                blocked_detail_json: None,
                cancel_review_work_units: true,
                updated_at: now,
                events: vec![event],
            },
        )
        .await?;
        Ok(())
    }
}
