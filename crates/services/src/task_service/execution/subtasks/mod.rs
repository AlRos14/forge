use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use db::{now_rfc3339, Task, TaskRepo, WorkspaceRepo};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};

use crate::{
    workflow::{default_states, engine::WorkflowEngine, inherited_subtask_workflow},
    ServiceError,
};

pub const ORDERED_TURN_BEFORE_SHA: &str = "ordered_turn_before_sha";
pub const ORDERED_TURN_NO_PROGRESS_COUNT: &str = "ordered_turn_no_progress_count";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SubtaskCommitResult {
    ForgeCommit {
        sha: String,
    },
    AgentCommitRange {
        from_sha: String,
        to_sha: String,
        count: u64,
    },
    NoDiff,
}

impl SubtaskCommitResult {
    pub fn result_type(&self) -> &'static str {
        match self {
            Self::ForgeCommit { .. } => "forge_commit",
            Self::AgentCommitRange { .. } => "agent_commit_range",
            Self::NoDiff => "no_diff",
        }
    }
}

impl fmt::Display for SubtaskCommitResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForgeCommit { sha } => write!(f, "forge_commit {sha}"),
            Self::AgentCommitRange {
                from_sha,
                to_sha,
                count,
            } => write!(f, "agent_commit_range {from_sha}..{to_sha} ({count})"),
            Self::NoDiff => f.write_str("no_diff"),
        }
    }
}

pub async fn record_subtask_commit_result(
    repo_path: &Path,
    before_sha: &str,
    subtask_id: &str,
    subtask_title: &str,
) -> Result<SubtaskCommitResult, git::GitError> {
    let current_sha = git::get_current_sha(repo_path).await?;
    let clean = git::is_worktree_clean(repo_path).await?;

    if current_sha == before_sha && clean {
        return Ok(SubtaskCommitResult::NoDiff);
    }

    if !clean {
        git::stage_all(repo_path).await?;
        let short_id = subtask_id.chars().take(8).collect::<String>();
        let message = format!("subtask({short_id}): {subtask_title}");
        git::commit_with_message(repo_path, &message).await?;
        let sha = git::get_current_sha(repo_path).await?;
        return Ok(SubtaskCommitResult::ForgeCommit { sha });
    }

    let count = git::count_commits_between(repo_path, before_sha, &current_sha).await?;
    Ok(SubtaskCommitResult::AgentCommitRange {
        from_sha: before_sha.to_owned(),
        to_sha: current_sha,
        count,
    })
}

pub enum NextTurn {
    Prompt { user_prompt: String },
    AllDone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreditResult {
    /// Parent has no subtasks, no in-progress subtask, or `before_sha` was missing —
    /// caller should fall through to its normal failure handling.
    Skipped,
    /// In-progress subtask exists but the worktree shows no progress.
    NotCommitted,
    /// In-progress subtask had a commit; subtask transitioned to `done`.
    /// `all_done` is true iff every ordered subtask is now terminal.
    Committed { all_done: bool },
}

/// Inspect the parent's worktree and credit any commit that the in-progress subtask
/// produced before its agent thread died. Used by failure paths that would otherwise
/// throw away a real commit because the executor exited non-zero.
pub async fn credit_in_progress_subtask_commit(
    db: &db::SqliteDb,
    event_bus: &Arc<EventBus>,
    workspace_root: &Path,
    parent_id: &str,
) -> Result<CreditResult, ServiceError> {
    let workspace = match WorkspaceRepo::get_by_task_id(db, parent_id).await? {
        Some(workspace) => workspace,
        None => return Ok(CreditResult::Skipped),
    };
    let wt_path = worktree_path(workspace_root, &workspace.worktree_path);

    let ordered_subtasks = list_active_subtasks(db, parent_id).await?;
    if ordered_subtasks.is_empty() {
        return Ok(CreditResult::Skipped);
    }
    let Some(subtask) = ordered_subtasks
        .iter()
        .find(|s| s.status == default_states::IN_PROGRESS)
        .cloned()
    else {
        return Ok(CreditResult::Skipped);
    };

    let metadata = subtask.metadata().map_err(|error| {
        ServiceError::invalid_operation(format!("invalid subtask metadata: {error}"))
    })?;
    let before_sha = metadata
        .extra
        .get(ORDERED_TURN_BEFORE_SHA)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    if before_sha.is_empty() {
        return Ok(CreditResult::Skipped);
    }

    let commit_result =
        record_subtask_commit_result(&wt_path, &before_sha, &subtask.id, &subtask.title).await?;

    event_bus.publish(ForgeEvent {
        event_type: "task.subtask_commit_recorded".to_owned(),
        entity_id: parent_id.to_owned(),
        timestamp: event_timestamp(),
        context: EventContext::TaskSubtaskCommitRecorded {
            task_id: parent_id.to_owned(),
            subtask_id: subtask.id.clone(),
            result_type: commit_result.result_type().to_owned(),
            commit_sha: commit_sha(&commit_result),
        },
    });

    if matches!(commit_result, SubtaskCommitResult::NoDiff) {
        return Ok(CreditResult::NotCommitted);
    }

    let workflow = inherited_subtask_workflow();
    subtask_engine(db, event_bus, workspace_root)
        .transition(
            &subtask.id,
            default_states::DONE,
            subtask.version,
            &workflow,
            &api_types::Actor::system(api_types::SystemComponent::Workflow),
            "credit subtask commit despite executor failure",
            false,
        )
        .await?;

    let refreshed = list_active_subtasks(db, parent_id).await?;
    let all_done = refreshed.iter().all(|s| {
        matches!(
            s.status.as_str(),
            default_states::DONE | default_states::CANCELLED
        )
    });
    Ok(CreditResult::Committed { all_done })
}

#[allow(dead_code)]
pub struct TurnHandle {
    pub subtask: Task,
    pub before_sha: String,
    pub total: usize,
    pub completed: Vec<CompletedSubtaskInfo>,
}

pub struct CompletedSubtaskInfo {
    pub title: String,
    pub was_skipped: bool,
}

fn worktree_path(workspace_root: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

pub fn compose_subtask_prompt(
    root: &Task,
    subtask: &Task,
    index: usize,
    total: usize,
    completed: &[CompletedSubtaskInfo],
    is_first: bool,
) -> String {
    let subtask_description = subtask.description.clone().unwrap_or_default();
    let position = format!("Subtask {} of {}", index + 1, total);
    let mut prompt = String::new();

    if is_first {
        prompt.push_str(&format!("Root task: {}\n", root.title));
        if let Some(description) = root.description.as_deref() {
            prompt.push_str(&format!("\n{description}\n"));
        }
        prompt.push_str(&format!(
            "\nThis task is broken into {total} ordered subtasks. \
             Work on the current subtask below, commit your changes, then stop. \
             Forge will dispatch the next subtask as a follow-up.\n"
        ));
    } else {
        prompt.push_str("Continuing the root task work.\n");
    }

    if !completed.is_empty() {
        prompt.push_str("\nCompleted subtasks:\n");
        for (i, info) in completed.iter().enumerate() {
            let marker = if info.was_skipped { "SKIPPED" } else { "DONE" };
            prompt.push_str(&format!("  {}. [{}] {}\n", i + 1, marker, info.title));
        }
    }

    prompt.push_str(&format!(
        "\n--- Current: {position} ---\n{}\n{subtask_description}\n\n\
         Work on this subtask only. Commit your changes, then stop.\n",
        subtask.title
    ));

    prompt
}

pub fn commit_sha(result: &SubtaskCommitResult) -> Option<String> {
    match result {
        SubtaskCommitResult::ForgeCommit { sha } => Some(sha.clone()),
        SubtaskCommitResult::AgentCommitRange { to_sha, .. } => Some(to_sha.clone()),
        SubtaskCommitResult::NoDiff => None,
    }
}

pub fn build_first_turn_prompt_from_context(root: &Task, sub_tasks: &[Task]) -> Option<String> {
    if root.parent_task_id.is_some() {
        return None;
    }
    if sub_tasks.is_empty() {
        return None;
    }

    let total = sub_tasks.len();
    let mut completed: Vec<CompletedSubtaskInfo> = Vec::new();
    let mut first_incomplete = None;

    for subtask in sub_tasks {
        if matches!(
            subtask.status.as_str(),
            default_states::DONE | default_states::CANCELLED
        ) {
            completed.push(CompletedSubtaskInfo {
                title: subtask.title.clone(),
                was_skipped: true,
            });
        } else if first_incomplete.is_none() {
            first_incomplete = Some(subtask);
        }
    }

    let subtask = first_incomplete?;
    let index = sub_tasks
        .iter()
        .position(|s| s.id == subtask.id)
        .unwrap_or(0);
    let is_first = completed.iter().all(|info| info.was_skipped);

    Some(compose_subtask_prompt(
        root, subtask, index, total, &completed, is_first,
    ))
}

async fn list_active_subtasks(
    db: &db::SqliteDb,
    parent_id: &str,
) -> Result<Vec<Task>, ServiceError> {
    let all = TaskRepo::list_subtasks_ordered(db, parent_id).await?;
    Ok(all)
}

fn subtask_engine(
    db: &db::SqliteDb,
    event_bus: &Arc<EventBus>,
    workspace_root: &Path,
) -> WorkflowEngine {
    WorkflowEngine {
        db: Arc::new(db.clone()),
        event_bus: Arc::clone(event_bus),
        review_runner: None,
        merge_service: None,
        cleanup_scheduler: None,
        task_executor: None,
        daemon_connections: None,
        workspace_exec_locks: None,
        terminal_activity: None,
        workspace_root: workspace_root.to_path_buf(),
        repo_cache_locks: None,
    }
}

/// Capture parent's HEAD SHA on `subtask`'s metadata, reset its no-progress counter,
/// and transition `todo → in_progress` if needed. Returns the refreshed subtask plus
/// the captured `before_sha`.
async fn start_turn_for(
    db: &db::SqliteDb,
    event_bus: &Arc<EventBus>,
    workspace_root: &Path,
    wt_path: &Path,
    subtask: Task,
) -> Result<(Task, String), ServiceError> {
    let before_sha = git::get_current_sha(wt_path).await?;
    let mut metadata = subtask.metadata().map_err(|error| {
        ServiceError::invalid_operation(format!("invalid subtask metadata: {error}"))
    })?;
    metadata.extra.insert(
        ORDERED_TURN_BEFORE_SHA.to_owned(),
        serde_json::json!(before_sha),
    );
    metadata.extra.insert(
        ORDERED_TURN_NO_PROGRESS_COUNT.to_owned(),
        serde_json::json!(0),
    );
    TaskRepo::set_metadata_json(db, &subtask.id, metadata.to_json(), &now_rfc3339()).await?;

    let mut subtask = subtask;
    if subtask.status == default_states::TODO {
        let workflow = inherited_subtask_workflow();
        let result = subtask_engine(db, event_bus, workspace_root)
            .transition(
                &subtask.id,
                default_states::IN_PROGRESS,
                subtask.version,
                &workflow,
                &api_types::Actor::system(api_types::SystemComponent::Workflow),
                "ordered turn started",
                false,
            )
            .await?;
        subtask = result.task;
    }
    Ok((subtask, before_sha))
}

pub async fn begin_next_turn(
    db: &db::SqliteDb,
    event_bus: &Arc<EventBus>,
    workspace_root: &Path,
    parent_id: &str,
) -> Result<Option<TurnHandle>, ServiceError> {
    let subtasks = TaskRepo::list_subtasks_ordered(db, parent_id).await?;
    if subtasks.is_empty() {
        return Ok(None);
    }

    let workspace = WorkspaceRepo::get_by_task_id(db, parent_id)
        .await?
        .ok_or_else(|| ServiceError::invalid_operation("parent workspace missing"))?;
    let wt_path = worktree_path(workspace_root, &workspace.worktree_path);

    let total = subtasks.len();
    let mut completed: Vec<CompletedSubtaskInfo> = Vec::new();
    let mut next_subtask = None;

    for subtask in &subtasks {
        if matches!(
            subtask.status.as_str(),
            default_states::DONE | default_states::CANCELLED
        ) {
            completed.push(CompletedSubtaskInfo {
                title: subtask.title.clone(),
                was_skipped: true,
            });
        } else if next_subtask.is_none() {
            next_subtask = Some(subtask.clone());
        }
    }

    let Some(subtask) = next_subtask else {
        return Ok(None);
    };
    let (subtask, before_sha) =
        start_turn_for(db, event_bus, workspace_root, &wt_path, subtask).await?;

    event_bus.publish(ForgeEvent {
        event_type: "task.subtask_sequence_started".to_owned(),
        entity_id: parent_id.to_owned(),
        timestamp: event_timestamp(),
        context: EventContext::TaskSubtaskSequenceStarted {
            task_id: parent_id.to_owned(),
        },
    });

    Ok(Some(TurnHandle {
        subtask,
        before_sha,
        total,
        completed,
    }))
}

pub async fn finish_current_turn_and_begin_next(
    db: &db::SqliteDb,
    event_bus: &Arc<EventBus>,
    workspace_root: &Path,
    parent_id: &str,
) -> Result<NextTurn, ServiceError> {
    let subtasks = TaskRepo::list_subtasks_ordered(db, parent_id).await?;
    if subtasks.is_empty() {
        return Err(ServiceError::invalid_operation("parent has no subtasks"));
    }

    let parent = TaskRepo::get_by_id(db, parent_id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", parent_id.to_owned()))?;
    let workspace = WorkspaceRepo::get_by_task_id(db, parent_id)
        .await?
        .ok_or_else(|| ServiceError::invalid_operation("parent workspace missing"))?;
    let wt_path = worktree_path(workspace_root, &workspace.worktree_path);

    let ordered_subtasks = list_active_subtasks(db, parent_id).await?;
    let in_progress = ordered_subtasks
        .iter()
        .find(|s| s.status == default_states::IN_PROGRESS);

    if let Some(subtask) = in_progress {
        let metadata = subtask.metadata().map_err(|error| {
            ServiceError::invalid_operation(format!("invalid subtask metadata: {error}"))
        })?;
        let before_sha = metadata
            .extra
            .get(ORDERED_TURN_BEFORE_SHA)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned();

        let commit_result =
            record_subtask_commit_result(&wt_path, &before_sha, &subtask.id, &subtask.title)
                .await?;

        // Record what we saw so the UI/log can show the result, then advance the
        // sequence regardless of whether this turn produced a new commit. The
        // agent may have committed during a *previous* turn (e.g. when the
        // workflow engine dispatched the agent before begin_next_turn captured a
        // baseline SHA), or it may have decided no work was needed for this
        // subtask. Either way, the reviewer gate is the real backstop for
        // "did the work actually land" — we shouldn't penalize the sequence with
        // an indefinite "you didn't commit" loop.
        event_bus.publish(ForgeEvent {
            event_type: "task.subtask_commit_recorded".to_owned(),
            entity_id: parent_id.to_owned(),
            timestamp: event_timestamp(),
            context: EventContext::TaskSubtaskCommitRecorded {
                task_id: parent_id.to_owned(),
                subtask_id: subtask.id.clone(),
                result_type: commit_result.result_type().to_owned(),
                commit_sha: commit_sha(&commit_result),
            },
        });

        let workflow = inherited_subtask_workflow();
        subtask_engine(db, event_bus, workspace_root)
            .transition(
                &subtask.id,
                default_states::DONE,
                subtask.version,
                &workflow,
                &api_types::Actor::system(api_types::SystemComponent::Workflow),
                "subtask turn completed",
                false,
            )
            .await?;
    } else if let Some(subtask) = ordered_subtasks
        .iter()
        .find(|s| s.status == default_states::TODO)
        .cloned()
    {
        // The daemon's task_dispatcher transitions the parent to in_progress via
        // the workflow engine, which dispatches the agent before begin_next_turn
        // ever fires for the first subtask. By the time the cascade reaches us,
        // the agent has already committed but no subtask is in_progress. Credit
        // that commit to the first todo subtask so the sequence advances cleanly.
        let baseline_sha = WorkspaceRepo::get_by_task_id(db, parent_id)
            .await?
            .and_then(|workspace| workspace.before_sha)
            .unwrap_or_default();
        let commit_result = if baseline_sha.is_empty() {
            SubtaskCommitResult::NoDiff
        } else {
            record_subtask_commit_result(&wt_path, &baseline_sha, &subtask.id, &subtask.title)
                .await?
        };

        event_bus.publish(ForgeEvent {
            event_type: "task.subtask_commit_recorded".to_owned(),
            entity_id: parent_id.to_owned(),
            timestamp: event_timestamp(),
            context: EventContext::TaskSubtaskCommitRecorded {
                task_id: parent_id.to_owned(),
                subtask_id: subtask.id.clone(),
                result_type: commit_result.result_type().to_owned(),
                commit_sha: commit_sha(&commit_result),
            },
        });

        if !matches!(commit_result, SubtaskCommitResult::NoDiff) {
            // Mark this todo subtask as done in two transitions (todo→in_progress→done)
            // so the inherited subtask workflow stays valid.
            let workflow = inherited_subtask_workflow();
            let engine = subtask_engine(db, event_bus, workspace_root);
            let intermediate = engine
                .transition(
                    &subtask.id,
                    default_states::IN_PROGRESS,
                    subtask.version,
                    &workflow,
                    &api_types::Actor::system(api_types::SystemComponent::Workflow),
                    "credit pre-existing commit",
                    false,
                )
                .await?;
            engine
                .transition(
                    &intermediate.task.id,
                    default_states::DONE,
                    intermediate.task.version,
                    &workflow,
                    &api_types::Actor::system(api_types::SystemComponent::Workflow),
                    "subtask turn completed",
                    false,
                )
                .await?;
        }
    }

    // Re-list after the transition so completed[] reflects the just-finished subtask.
    let ordered_subtasks = list_active_subtasks(db, parent_id).await?;
    let total = ordered_subtasks.len();
    let mut completed: Vec<CompletedSubtaskInfo> = Vec::new();
    let mut next_subtask = None;

    for subtask in &ordered_subtasks {
        if matches!(
            subtask.status.as_str(),
            default_states::DONE | default_states::CANCELLED
        ) {
            completed.push(CompletedSubtaskInfo {
                title: subtask.title.clone(),
                was_skipped: false,
            });
        } else if next_subtask.is_none() {
            next_subtask = Some(subtask.clone());
        }
    }

    let Some(subtask) = next_subtask else {
        return Ok(NextTurn::AllDone);
    };
    let (subtask, _before_sha) =
        start_turn_for(db, event_bus, workspace_root, &wt_path, subtask).await?;

    let index = ordered_subtasks
        .iter()
        .position(|s| s.id == subtask.id)
        .unwrap_or(0);
    let is_first = completed.iter().all(|info| info.was_skipped);
    let user_prompt = compose_subtask_prompt(&parent, &subtask, index, total, &completed, is_first);

    Ok(NextTurn::Prompt { user_prompt })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    async fn setup_repo() -> (TempDir, PathBuf, String) {
        let dir = TempDir::new().expect("temp dir creates");
        let repo_path = dir.path().join("repo");
        std::fs::create_dir_all(&repo_path).expect("repo dir creates");
        git::init(&repo_path).await.expect("repo initializes");
        std::fs::write(repo_path.join("README.md"), "# Test\n").expect("readme writes");
        let before_sha = git::commit_all(&repo_path, "initial")
            .await
            .expect("initial commit creates");

        (dir, repo_path, before_sha)
    }

    fn make_task(id: &str, title: &str, description: Option<&str>, status: &str) -> Task {
        Task {
            id: id.to_owned(),
            project_id: "project".to_owned(),
            repo_id: Some("repo".to_owned()),
            parent_task_id: None,
            assignee_type: None,
            assignee_id: None,
            title: title.to_owned(),
            description: description.map(str::to_owned),
            task_type: "task".to_string(),
            status: status.to_owned(),
            is_automation: false,
            priority: 0,
            board_position: 1.0,
            subtask_order: None,
            task_state_config: None,
            merge_config: None,
            metadata_json: None,
            plan: None,
            error_annotation: None,
            blocked_json: None,
            failed_json: None,
            entry_barrier_json: None,
            review_passed_at: None,
            archived_at: None,
            deleted_at: None,
            version: 0,
            created_at: "now".to_owned(),
            updated_at: "now".to_owned(),
        }
    }

    #[tokio::test]
    async fn records_no_diff_when_head_and_worktree_are_unchanged() {
        let (_dir, repo_path, before_sha) = setup_repo().await;

        let result =
            record_subtask_commit_result(&repo_path, &before_sha, "subtask-1", "No changes")
                .await
                .expect("result records");

        assert!(matches!(result, SubtaskCommitResult::NoDiff));
        assert_eq!(result.result_type(), "no_diff");
    }

    #[tokio::test]
    async fn records_forge_commit_for_dirty_worktree() {
        let (_dir, repo_path, before_sha) = setup_repo().await;
        std::fs::write(repo_path.join("feature.txt"), "feature\n").expect("feature writes");

        let result =
            record_subtask_commit_result(&repo_path, &before_sha, "subtask-123456", "Add feature")
                .await
                .expect("result records");

        match result {
            SubtaskCommitResult::ForgeCommit { sha } => {
                assert_ne!(sha, before_sha);
                assert_eq!(
                    sha,
                    git::get_current_sha(&repo_path).await.expect("head reads")
                );
            }
            other => panic!("expected forge commit, got {other:?}"),
        }
        assert!(git::is_worktree_clean(&repo_path)
            .await
            .expect("worktree clean check succeeds"));
    }

    #[tokio::test]
    async fn records_agent_commit_range_for_clean_changed_head() {
        let (_dir, repo_path, before_sha) = setup_repo().await;
        std::fs::write(repo_path.join("agent.txt"), "agent\n").expect("agent file writes");
        let to_sha = git::commit_all(&repo_path, "agent commit")
            .await
            .expect("agent commit creates");

        let result =
            record_subtask_commit_result(&repo_path, &before_sha, "subtask-1", "Agent commit")
                .await
                .expect("result records");

        match result {
            SubtaskCommitResult::AgentCommitRange {
                from_sha,
                to_sha: actual_to_sha,
                count,
            } => {
                assert_eq!(from_sha, before_sha);
                assert_eq!(actual_to_sha, to_sha);
                assert_eq!(count, 1);
            }
            other => panic!("expected agent commit range, got {other:?}"),
        }
    }

    #[test]
    fn compose_subtask_prompt_includes_sequence_context() {
        let root = make_task(
            "root",
            "Root title",
            Some("Root description"),
            "in_progress",
        );
        let subtask1 = make_task(
            "subtask1",
            "Subtask 1 title",
            Some("Subtask 1 description"),
            "todo",
        );
        let subtask2 = make_task(
            "subtask2",
            "Subtask 2 title",
            Some("Subtask 2 description"),
            "todo",
        );

        let first = compose_subtask_prompt(&root, &subtask1, 0, 2, &[], true);
        assert!(first.contains("Root title"));
        assert!(first.contains("Root description"));
        assert!(first.contains("Subtask 1 of 2"));
        assert!(first.contains("Subtask 1 title"));
        assert!(first.contains("broken into 2 ordered subtasks"));
        assert!(!first.contains("Completed subtasks:"));

        let completed = vec![CompletedSubtaskInfo {
            title: "Subtask 1 title".to_owned(),
            was_skipped: false,
        }];
        let second = compose_subtask_prompt(&root, &subtask2, 1, 2, &completed, false);
        assert!(!second.contains("Root title"));
        assert!(second.contains("Continuing the root task work"));
        assert!(second.contains("Subtask 2 of 2"));
        assert!(second.contains("Subtask 2 title"));
        assert!(second.contains("Completed subtasks:"));
        assert!(second.contains("[DONE] Subtask 1 title"));
    }
}
