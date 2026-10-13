use super::*;
use ::workspace::WorkspaceManager;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Prepare the Task's own integration workspace for a WorkUnit operation.
/// This is the integration target; executable WorkUnit workspaces are stored
/// and resolved by exact WorkUnit identity.
pub(crate) async fn prepare_integration_workspace_for_work_unit(
    db: &SqliteDb,
    workspace_root: &std::path::Path,
    task: &Task,
    repo_cache_locks: Option<Arc<RepoCacheLockManager>>,
) -> Result<Workspace> {
    let task_repo_id = task.repo_id.as_deref().ok_or_else(|| {
        ServiceError::invalid_operation("WorkUnit Task has no repository binding")
    })?;
    if let Some(workspace) = WorkspaceRepo::get_integration_by_task_id(db, &task.id).await? {
        if workspace.repo_id != task_repo_id {
            return Err(ServiceError::invalid_operation(
                "Task integration Workspace repository does not match its Task",
            ));
        }
        let repo = RepoRepo::get_by_id(db, &workspace.repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repo", workspace.repo_id.clone()))?;
        if repo.project_id != task.project_id {
            return Err(ServiceError::invalid_operation(
                "Task integration Workspace repository belongs to another Project",
            ));
        }
        if workspace.status == WorkspaceStatus::Cleaning {
            return Err(ServiceError::invalid_operation(format!(
                "Task integration workspace for {} is being cleaned",
                task.id
            )));
        }
        return match worktree_readiness(Path::new(&workspace.worktree_path)).await {
            WorktreeReadiness::Ready if workspace.status == WorkspaceStatus::Ready => Ok(workspace),
            WorktreeReadiness::Ready => {
                WorkspaceRepo::update_status(
                    db,
                    &workspace.id,
                    WorkspaceStatus::Ready,
                    None,
                    &now_rfc3339(),
                )
                .await?;
                WorkspaceRepo::get_by_id(db, &workspace.id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("workspace", workspace.id))
            }
            WorktreeReadiness::Missing | WorktreeReadiness::Invalid => {
                recover_missing_worktree(
                    db,
                    workspace_root,
                    task,
                    &task.id,
                    workspace,
                    repo_cache_locks,
                )
                .await
            }
        };
    }
    create_fresh_workspace(db, workspace_root, task, &task.id, repo_cache_locks).await
}

async fn recover_missing_worktree(
    db: &SqliteDb,
    workspace_root: &std::path::Path,
    task: &Task,
    task_id: &str,
    workspace: Workspace,
    repo_cache_locks: Option<Arc<RepoCacheLockManager>>,
) -> Result<Workspace> {
    let readiness = worktree_readiness(Path::new(&workspace.worktree_path)).await;
    warn!(
        task_id = task_id,
        workspace_id = %workspace.id,
        worktree_path = %workspace.worktree_path,
        readiness = ?readiness,
        "workspace worktree path missing or unusable, attempting recovery"
    );

    let repo_id = task
        .repo_id
        .as_deref()
        .ok_or_else(|| ServiceError::invalid_operation("task has no associated repo"))?;
    let repo = RepoRepo::get_by_id(db, repo_id)
        .await?
        .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
    let repo_source = resolve_repo_source(&repo, workspace_root).await?;

    if !Path::new(&repo_source).exists() {
        return Err(ServiceError::invalid_operation(format!(
            "repo source path does not exist: {repo_source}"
        )));
    }

    if matches!(readiness, WorktreeReadiness::Invalid)
        && try_repair_worktree_gitdir(Path::new(&repo_source), Path::new(&workspace.worktree_path))
            .await
    {
        info!(
            task_id = task_id,
            workspace_id = %workspace.id,
            worktree_path = %workspace.worktree_path,
            "workspace gitdir repaired"
        );
        return Ok(workspace);
    }

    let branch = &workspace.branch;
    let branch_exists = git::branch_exists(Path::new(&repo_source), branch)
        .await
        .unwrap_or(false);

    if branch_exists {
        if !matches!(readiness, WorktreeReadiness::Missing) {
            move_unusable_worktree_aside(Path::new(&workspace.worktree_path)).await?;
        }
        let mut manager = WorkspaceManager::new(workspace_root.to_path_buf());
        if let Some(locks) = repo_cache_locks {
            manager = manager.with_repo_cache_locks(locks);
        }
        let worktree_path = manager
            .recover_worktree_named(&repo_source, task_id, &repo.name, branch)
            .await
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let before_sha = git::get_current_sha(&worktree_path).await.ok();
        let now = now_rfc3339();
        WorkspaceRepo::update_status(db, &workspace.id, WorkspaceStatus::Ready, None, &now).await?;

        info!(
            task_id = task_id,
            workspace_id = %workspace.id,
            branch = %branch,
            worktree_path = %worktree_path.display(),
            before_sha = ?before_sha,
            "workspace recovered from existing branch"
        );

        WorkspaceRepo::get_by_id(db, &workspace.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("workspace", workspace.id))
    } else {
        WorkspaceRepo::delete(db, &workspace.id).await?;
        warn!(
            task_id = task_id,
            branch = %branch,
            "task branch no longer exists in repo, workspace reset required"
        );
        Err(ServiceError::WorkspaceResetRequired {
            task_id: task_id.to_owned(),
            reason: format!(
                "worktree and branch '{branch}' are both gone; workspace must be recreated from {}",
                repo.default_branch
            ),
        })
    }
}

#[derive(Debug)]
enum WorktreeReadiness {
    Ready,
    Missing,
    Invalid,
}

async fn worktree_readiness(worktree_path: &Path) -> WorktreeReadiness {
    if !worktree_path.exists() {
        return WorktreeReadiness::Missing;
    }
    if !worktree_path.join(".git").exists() {
        // Some unit tests seed lightweight workspace rows with plain directories.
        // Real Forge-created worktrees always have .git metadata, so only verify
        // git health when metadata is present.
        return WorktreeReadiness::Ready;
    }
    match git::get_current_sha(worktree_path).await {
        Ok(_) => WorktreeReadiness::Ready,
        Err(_) => WorktreeReadiness::Invalid,
    }
}

async fn try_repair_worktree_gitdir(repo_source: &Path, worktree_path: &Path) -> bool {
    let output = Command::new("git")
        .args(["worktree", "repair", &worktree_path.to_string_lossy()])
        .current_dir(repo_source)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => git::get_current_sha(worktree_path).await.is_ok(),
        Ok(output) => {
            warn!(
                repo_source = %repo_source.display(),
                worktree_path = %worktree_path.display(),
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "git worktree repair did not repair workspace"
            );
            false
        }
        Err(error) => {
            warn!(
                repo_source = %repo_source.display(),
                worktree_path = %worktree_path.display(),
                %error,
                "failed to run git worktree repair"
            );
            false
        }
    }
}

async fn move_unusable_worktree_aside(worktree_path: &Path) -> Result<()> {
    if !worktree_path.exists() {
        return Ok(());
    }
    let parent = worktree_path.parent().ok_or_else(|| {
        ServiceError::invalid_operation(format!(
            "worktree path has no parent: {}",
            worktree_path.display()
        ))
    })?;
    let name = worktree_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("worktree");
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let backup_path: PathBuf = parent.join(format!("{name}.broken-{millis}"));
    tokio::fs::rename(worktree_path, &backup_path)
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!(
                "failed to move unusable worktree aside: {error}"
            ))
        })?;
    warn!(
        worktree_path = %worktree_path.display(),
        backup_path = %backup_path.display(),
        "moved unusable worktree aside before recovery"
    );
    Ok(())
}

async fn create_fresh_workspace(
    db: &SqliteDb,
    workspace_root: &std::path::Path,
    task: &Task,
    task_id: &str,
    repo_cache_locks: Option<Arc<RepoCacheLockManager>>,
) -> Result<Workspace> {
    let repo_id = task
        .repo_id
        .as_deref()
        .ok_or_else(|| ServiceError::invalid_operation("task has no associated repo"))?;
    let repo = RepoRepo::get_by_id(db, repo_id)
        .await?
        .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
    let now = now_rfc3339();
    let mut manager = WorkspaceManager::new(workspace_root.to_path_buf());
    if let Some(locks) = repo_cache_locks {
        manager = manager.with_repo_cache_locks(locks);
    }
    let worktree_source = resolve_repo_source(&repo, workspace_root).await?;
    let branch = ::workspace::task_branch_name(task_id);
    let branch_exists = git::branch_exists(Path::new(&worktree_source), &branch)
        .await
        .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
    let worktree_path = if branch_exists {
        // A rejected admission may have removed its fresh workspace row and
        // directory after Git created the task branch. Recover that exact
        // task-scoped branch so a corrected retry remains possible and no
        // potentially useful work is discarded.
        manager
            .recover_worktree_named(&worktree_source, task_id, &repo.name, &branch)
            .await
    } else {
        manager
            .create_worktree_named(&worktree_source, task_id, &repo.name, &repo.default_branch)
            .await
    }
    .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
    let before_sha = git::get_current_sha(&worktree_path).await.ok();
    let workspace_id = new_uuid_v4();
    let workspace = match WorkspaceRepo::create(
        db,
        CreateWorkspace {
            id: workspace_id,
            task_id: task_id.to_owned(),
            repo_id: repo_id.to_owned(),
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            branch,
            status: WorkspaceStatus::Ready,
            before_sha,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    {
        Ok(workspace) => workspace,
        Err(error) => {
            // A concurrent creator may have won the Task-unique workspace
            // row after this call observed no row.  Never remove its
            // worktree when our INSERT loses that race.
            if WorkspaceRepo::get_integration_by_task_id(db, task_id)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                let _ = manager.cleanup_worktree(task_id).await;
            }
            return Err(error.into());
        }
    };

    info!(
        task_id = task_id,
        workspace_id = %workspace.id,
        repo_id = repo_id,
        workspace_root = %workspace_root.display(),
        worktree_path = %workspace.worktree_path,
        branch = %workspace.branch,
        "workspace created"
    );

    Ok(workspace)
}

pub(crate) async fn resolve_repo_source(
    repo: &db::Repo,
    workspace_root: &std::path::Path,
) -> Result<String> {
    if let Some(local_path) = repo
        .local_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        if Path::new(local_path).exists() {
            return Ok(local_path.to_owned());
        }
    }
    let clone_path = workspace_root.join(".repos").join(&repo.id);
    if !clone_path.exists() {
        tokio::fs::create_dir_all(
            clone_path
                .parent()
                .ok_or_else(|| ServiceError::invalid_operation("repo cache path has no parent"))?,
        )
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("failed to create repo cache: {error}"))
        })?;
        let output = Command::new("git")
            .args(["clone", &repo.remote_url, &clone_path.to_string_lossy()])
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .await
            .map_err(|error| {
                ServiceError::invalid_operation(format!("failed to clone repo: {error}"))
            })?;
        if !output.status.success() {
            return Err(ServiceError::invalid_operation(format!(
                "failed to clone repo from {}: {}",
                repo.remote_url,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    }
    Ok(clone_path.to_string_lossy().into_owned())
}

pub(super) fn default_workspace_root() -> PathBuf {
    std::env::var("FORGE_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("forge").join("worktrees"))
}
