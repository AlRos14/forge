#![forbid(unsafe_code)]

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::{fs, process::Command};

pub mod repo_cache;

pub use repo_cache::RepoCacheLockManager;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("workspace already exists")]
    AlreadyExists,

    #[error("workspace is locked")]
    Locked,

    #[error("path escapes worktree root")]
    PathEscape,

    #[error("workspace not found")]
    NotFound,

    #[error("workspace identity is not a safe opaque path segment")]
    InvalidIdentity,

    #[error("git error: {0}")]
    Git(#[from] git::GitError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, WorkspaceError>;

fn git_command() -> Command {
    let mut command = Command::new("git");
    command
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    command
}

#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    root: PathBuf,
    repo_cache_locks: Option<Arc<RepoCacheLockManager>>,
}

impl WorkspaceManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            repo_cache_locks: None,
        }
    }

    pub fn with_repo_cache_locks(mut self, locks: Arc<RepoCacheLockManager>) -> Self {
        self.repo_cache_locks = Some(locks);
        self
    }

    pub async fn create_worktree(
        &self,
        repo_url: &str,
        task_id: &str,
        base_branch: &str,
    ) -> Result<PathBuf> {
        let repo_name = repo_name(repo_url);
        self.create_worktree_named(repo_url, task_id, &repo_name, base_branch)
            .await
    }

    pub async fn create_worktree_named(
        &self,
        repo_url: &str,
        task_id: &str,
        repo_name: &str,
        base_branch: &str,
    ) -> Result<PathBuf> {
        let task_root = self.root.join(task_id);
        let worktree_path = task_root.join(repo_name);

        if fs::try_exists(&worktree_path).await? {
            return Err(WorkspaceError::AlreadyExists);
        }

        fs::create_dir_all(&task_root).await?;

        let _repo_cache_guard = if let Some(locks) = &self.repo_cache_locks {
            Some(locks.acquire(repo_url).await)
        } else {
            None
        };

        let branch_name = task_branch_name(task_id);
        let mut args = vec![
            "worktree".to_string(),
            "add".to_string(),
            "-b".to_string(),
            branch_name,
            worktree_path.to_string_lossy().to_string(),
        ];

        if !base_branch.is_empty() {
            args.push(base_branch.to_string());
        }

        let output = git_command()
            .args(&args)
            .current_dir(repo_url)
            .output()
            .await?;

        if !output.status.success() {
            return Err(git::GitError::CommandFailed {
                command: format!("git {}", args.join(" ")),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            }
            .into());
        }

        Ok(worktree_path)
    }

    /// Create one isolated WorkUnit worktree from an exact recorded base ref.
    /// The Task, WorkUnit, Workspace, and repository IDs form its path and
    /// branch identity; user-controlled titles and scope never enter either.
    pub async fn create_work_unit_worktree(
        &self,
        repo_path: &str,
        task_id: &str,
        work_unit_id: &str,
        workspace_id: &str,
        repo_id: &str,
        base_ref: &str,
    ) -> Result<PathBuf> {
        let branch = work_unit_branch_name(task_id, work_unit_id, workspace_id)?;
        let path = self.work_unit_path(task_id, work_unit_id, workspace_id, repo_id)?;
        if fs::try_exists(&path).await? {
            return Err(WorkspaceError::AlreadyExists);
        }
        let parent = path.parent().ok_or(WorkspaceError::InvalidIdentity)?;
        fs::create_dir_all(parent).await?;
        let _repo_cache_guard = if let Some(locks) = &self.repo_cache_locks {
            Some(locks.acquire(repo_path).await)
        } else {
            None
        };
        let args = [
            "worktree",
            "add",
            "-b",
            &branch,
            &path.to_string_lossy(),
            base_ref,
        ];
        let output = git_command()
            .args(args)
            .current_dir(repo_path)
            .output()
            .await?;
        if !output.status.success() {
            return Err(git::GitError::CommandFailed {
                command: format!("git {}", args.join(" ")),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            }
            .into());
        }
        Ok(path)
    }

    /// Recover a missing WorkUnit worktree from its exact persisted branch.
    /// A branch belonging to another Workspace identity is rejected.
    pub async fn recover_work_unit_worktree(
        &self,
        repo_path: &str,
        task_id: &str,
        work_unit_id: &str,
        workspace_id: &str,
        repo_id: &str,
        existing_branch: &str,
    ) -> Result<PathBuf> {
        let expected_branch = work_unit_branch_name(task_id, work_unit_id, workspace_id)?;
        if existing_branch != expected_branch {
            return Err(WorkspaceError::InvalidIdentity);
        }
        let path = self.work_unit_path(task_id, work_unit_id, workspace_id, repo_id)?;
        if fs::try_exists(&path).await? {
            return Err(WorkspaceError::AlreadyExists);
        }
        let parent = path.parent().ok_or(WorkspaceError::InvalidIdentity)?;
        fs::create_dir_all(parent).await?;
        let _repo_cache_guard = if let Some(locks) = &self.repo_cache_locks {
            Some(locks.acquire(repo_path).await)
        } else {
            None
        };
        let _ = git_command()
            .args(["worktree", "prune"])
            .current_dir(repo_path)
            .output()
            .await;
        let args = ["worktree", "add", &path.to_string_lossy(), existing_branch];
        let output = git_command()
            .args(args)
            .current_dir(repo_path)
            .output()
            .await?;
        if !output.status.success() {
            return Err(git::GitError::CommandFailed {
                command: format!("git {}", args.join(" ")),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            }
            .into());
        }
        Ok(path)
    }

    /// Remove only the physical worktree identified by the exact WorkUnit and
    /// Workspace IDs. Its branch and all sibling worktrees are preserved.
    pub async fn cleanup_work_unit_worktree(
        &self,
        repo_path: &str,
        task_id: &str,
        work_unit_id: &str,
        workspace_id: &str,
        repo_id: &str,
    ) -> Result<()> {
        let path = self.work_unit_path(task_id, work_unit_id, workspace_id, repo_id)?;
        let _repo_cache_guard = if let Some(locks) = &self.repo_cache_locks {
            Some(locks.acquire(repo_path).await)
        } else {
            None
        };
        if fs::try_exists(&path).await? {
            let args = ["worktree", "remove", "--force", &path.to_string_lossy()];
            let output = git_command()
                .args(args)
                .current_dir(repo_path)
                .output()
                .await?;
            if !output.status.success() {
                return Err(git::GitError::CommandFailed {
                    command: format!("git {}", args.join(" ")),
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                }
                .into());
            }
        }
        let _ = git_command()
            .args(["worktree", "prune"])
            .current_dir(repo_path)
            .output()
            .await;
        Ok(())
    }

    fn work_unit_path(
        &self,
        task_id: &str,
        work_unit_id: &str,
        workspace_id: &str,
        repo_id: &str,
    ) -> Result<PathBuf> {
        for value in [task_id, work_unit_id, workspace_id, repo_id] {
            validate_identity_segment(value)?;
        }
        // Keep WorkUnit worktrees outside the legacy Task root. Legacy Task
        // cleanup removes `root/<task_id>` recursively, so nesting them below
        // that directory would let a Task-level cleanup erase sibling work.
        Ok(self
            .root
            .join("work_units")
            .join(task_id)
            .join(work_unit_id)
            .join(workspace_id)
            .join(repo_id))
    }

    /// Return the deterministic physical path for one exact WorkUnit
    /// Workspace identity after applying the same opaque-ID validation as
    /// create/recover/cleanup.
    pub fn work_unit_worktree_path(
        &self,
        task_id: &str,
        work_unit_id: &str,
        workspace_id: &str,
        repo_id: &str,
    ) -> Result<PathBuf> {
        self.work_unit_path(task_id, work_unit_id, workspace_id, repo_id)
    }

    pub async fn recover_worktree(
        &self,
        repo_url: &str,
        task_id: &str,
        existing_branch: &str,
    ) -> Result<PathBuf> {
        let repo_name = repo_name(repo_url);
        self.recover_worktree_named(repo_url, task_id, &repo_name, existing_branch)
            .await
    }

    pub async fn recover_worktree_named(
        &self,
        repo_url: &str,
        task_id: &str,
        repo_name: &str,
        existing_branch: &str,
    ) -> Result<PathBuf> {
        let task_root = self.root.join(task_id);
        let worktree_path = task_root.join(repo_name);

        if fs::try_exists(&worktree_path).await? {
            return Err(WorkspaceError::AlreadyExists);
        }

        fs::create_dir_all(&task_root).await?;

        let _repo_cache_guard = if let Some(locks) = &self.repo_cache_locks {
            Some(locks.acquire(repo_url).await)
        } else {
            None
        };

        // Prune stale worktree references before re-adding
        let _ = git_command()
            .args(["worktree", "prune"])
            .current_dir(repo_url)
            .output()
            .await;

        let args = [
            "worktree",
            "add",
            &worktree_path.to_string_lossy(),
            existing_branch,
        ];

        let output = git_command()
            .args(args)
            .current_dir(repo_url)
            .output()
            .await?;

        if !output.status.success() {
            return Err(git::GitError::CommandFailed {
                command: format!("git {}", args.join(" ")),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            }
            .into());
        }

        Ok(worktree_path)
    }

    pub async fn reset_worktree(&self, task_id: &str, repo_name: &str) -> Result<()> {
        let worktree_path = self.root.join(task_id).join(repo_name);

        if !fs::try_exists(&worktree_path).await? {
            return Err(WorkspaceError::NotFound);
        }

        let reset_args = ["reset", "--hard", "HEAD"];
        let output = git_command()
            .args(reset_args)
            .current_dir(&worktree_path)
            .output()
            .await?;

        if !output.status.success() {
            return Err(git::GitError::CommandFailed {
                command: format!("git {}", reset_args.join(" ")),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            }
            .into());
        }

        let clean_args = ["clean", "-fd"];
        let output = git_command()
            .args(clean_args)
            .current_dir(&worktree_path)
            .output()
            .await?;

        if !output.status.success() {
            return Err(git::GitError::CommandFailed {
                command: format!("git {}", clean_args.join(" ")),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            }
            .into());
        }

        Ok(())
    }

    pub async fn acquire_lock(&self, task_id: &str) -> Result<()> {
        let task_root = self.root.join(task_id);
        fs::create_dir_all(&task_root).await?;

        let lock_path = task_root.join(".forge.lock");
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(WorkspaceError::Locked)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn release_lock(&self, task_id: &str) -> Result<()> {
        let lock_path = self.root.join(task_id).join(".forge.lock");
        match fs::remove_file(lock_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(WorkspaceError::NotFound)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn cleanup_worktree(&self, task_id: &str) -> Result<()> {
        let task_root = self.root.join(task_id);
        match fs::remove_dir_all(task_root).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(WorkspaceError::NotFound)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub async fn detect_orphans(&self, active_task_ids: &[String]) -> Result<Vec<String>> {
        let active_task_ids = active_task_ids.iter().collect::<HashSet<_>>();
        let mut orphans = Vec::new();

        let mut entries = match fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(orphans),
            Err(error) => return Err(error.into()),
        };

        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }

            let task_id = entry.file_name().to_string_lossy().to_string();
            if task_id == "work_units" {
                continue;
            }
            if !active_task_ids.contains(&task_id) {
                orphans.push(task_id);
            }
        }

        orphans.sort();
        Ok(orphans)
    }

    pub fn validate_path(worktree_root: &Path, target_path: &Path) -> Result<()> {
        let worktree_root = worktree_root.canonicalize()?;
        let target_path = target_path.canonicalize()?;

        if target_path.starts_with(worktree_root) {
            Ok(())
        } else {
            Err(WorkspaceError::PathEscape)
        }
    }
}

pub fn task_branch_name(task_id: &str) -> String {
    format!("task/{}", &task_id[..task_id.len().min(8)])
}

pub fn work_unit_branch_name(
    task_id: &str,
    work_unit_id: &str,
    workspace_id: &str,
) -> Result<String> {
    for value in [task_id, work_unit_id, workspace_id] {
        validate_identity_segment(value)?;
    }
    Ok(format!(
        "forge/work-unit/{task_id}/{work_unit_id}/{workspace_id}"
    ))
}

fn validate_identity_segment(value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(WorkspaceError::InvalidIdentity);
    }
    Ok(())
}

fn repo_name(repo_url: &str) -> String {
    let trimmed = repo_url.trim_end_matches(['/', '\\']);
    let last_component = trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|component| !component.is_empty())
        .unwrap_or("repo");

    last_component
        .strip_suffix(".git")
        .unwrap_or(last_component)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs;

    async fn setup_repo() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let repo_path = dir.path().join("repo");

        fs::create_dir_all(&repo_path).await.unwrap();
        git::init(&repo_path).await.unwrap();
        fs::write(repo_path.join("README.md"), "# Test\n")
            .await
            .unwrap();
        git::commit_all(&repo_path, "initial commit").await.unwrap();

        (dir, repo_path)
    }

    #[tokio::test]
    async fn test_create_worktree() {
        let (_repo_dir, repo_path) = setup_repo().await;
        let workspace_dir = TempDir::new().unwrap();
        let manager = WorkspaceManager::new(workspace_dir.path().to_path_buf());

        let worktree_path = manager
            .create_worktree(repo_path.to_str().unwrap(), "task-1", "HEAD")
            .await
            .unwrap();

        assert_eq!(
            worktree_path,
            workspace_dir.path().join("task-1").join("repo")
        );
        assert!(fs::try_exists(worktree_path.join("README.md"))
            .await
            .unwrap());
        let branches = git::list_branches(&repo_path).await.unwrap();
        assert!(branches.branches.contains(&task_branch_name("task-1")));

        let sha = git::get_current_sha(&worktree_path).await.unwrap();
        assert_eq!(sha.len(), 40);
    }

    #[tokio::test]
    async fn test_lock_unlock() {
        let workspace_dir = TempDir::new().unwrap();
        let manager = WorkspaceManager::new(workspace_dir.path().to_path_buf());

        manager.acquire_lock("task-1").await.unwrap();
        assert!(matches!(
            manager.acquire_lock("task-1").await,
            Err(WorkspaceError::Locked)
        ));

        manager.release_lock("task-1").await.unwrap();
        manager.acquire_lock("task-1").await.unwrap();
    }

    #[tokio::test]
    async fn test_path_validation() {
        let workspace_dir = TempDir::new().unwrap();
        let worktree_root = workspace_dir.path().join("worktree");
        let inside = worktree_root.join("src").join("lib.rs");
        let outside = workspace_dir.path().join("outside.txt");

        fs::create_dir_all(inside.parent().unwrap()).await.unwrap();
        fs::write(&inside, "").await.unwrap();
        fs::write(&outside, "").await.unwrap();

        WorkspaceManager::validate_path(&worktree_root, &inside).unwrap();
        assert!(matches!(
            WorkspaceManager::validate_path(&worktree_root, &outside),
            Err(WorkspaceError::PathEscape)
        ));
    }

    #[tokio::test]
    async fn test_orphan_detection() {
        let workspace_dir = TempDir::new().unwrap();
        let manager = WorkspaceManager::new(workspace_dir.path().to_path_buf());

        fs::create_dir_all(workspace_dir.path().join("active"))
            .await
            .unwrap();
        fs::create_dir_all(workspace_dir.path().join("orphan-a"))
            .await
            .unwrap();
        fs::create_dir_all(workspace_dir.path().join("orphan-b"))
            .await
            .unwrap();
        fs::write(workspace_dir.path().join("not-a-task"), "")
            .await
            .unwrap();

        let active = vec!["active".to_string()];
        let orphans = manager.detect_orphans(&active).await.unwrap();

        assert_eq!(orphans, vec!["orphan-a", "orphan-b"]);
    }

    #[tokio::test]
    async fn test_cleanup() {
        let workspace_dir = TempDir::new().unwrap();
        let manager = WorkspaceManager::new(workspace_dir.path().to_path_buf());
        let task_root = workspace_dir.path().join("task-1");

        fs::create_dir_all(task_root.join("repo")).await.unwrap();
        fs::write(task_root.join("repo").join("README.md"), "")
            .await
            .unwrap();

        manager.cleanup_worktree("task-1").await.unwrap();
        assert!(!fs::try_exists(&task_root).await.unwrap());
    }

    #[tokio::test]
    async fn work_unit_worktrees_are_distinct_and_cleanup_is_workspace_scoped() {
        let (_repo_dir, repo_path) = setup_repo().await;
        let workspace_dir = TempDir::new().unwrap();
        let manager = WorkspaceManager::new(workspace_dir.path().to_path_buf());
        let repo = repo_path.to_str().unwrap();
        let integration = manager
            .create_worktree(repo, "task-a", "HEAD")
            .await
            .unwrap();
        let a = manager
            .create_work_unit_worktree(repo, "task-a", "unit-a", "workspace-a", "repo-a", "HEAD")
            .await
            .unwrap();
        let b = manager
            .create_work_unit_worktree(repo, "task-a", "unit-b", "workspace-b", "repo-a", "HEAD")
            .await
            .unwrap();
        let c = manager
            .create_work_unit_worktree(repo, "task-a", "unit-c", "workspace-c", "repo-a", "HEAD")
            .await
            .unwrap();

        assert_ne!(integration, a);
        assert_ne!(a, b);
        assert_ne!(b, c);
        let branches = git::list_branches(&repo_path).await.unwrap();
        let branch_a = work_unit_branch_name("task-a", "unit-a", "workspace-a").unwrap();
        let branch_b = work_unit_branch_name("task-a", "unit-b", "workspace-b").unwrap();
        let branch_c = work_unit_branch_name("task-a", "unit-c", "workspace-c").unwrap();
        assert!(branches.branches.contains(&branch_a));
        assert!(branches.branches.contains(&branch_b));
        assert!(branches.branches.contains(&branch_c));

        manager
            .cleanup_work_unit_worktree(repo, "task-a", "unit-a", "workspace-a", "repo-a")
            .await
            .unwrap();
        assert!(!fs::try_exists(&a).await.unwrap());
        assert!(fs::try_exists(&integration).await.unwrap());
        assert!(fs::try_exists(&b).await.unwrap());
        assert!(fs::try_exists(&c).await.unwrap());
        let branches = git::list_branches(&repo_path).await.unwrap();
        assert!(branches.branches.contains(&branch_a));
        assert!(branches.branches.contains(&branch_b));
        assert!(branches.branches.contains(&branch_c));

        let recovered = manager
            .recover_work_unit_worktree(
                repo,
                "task-a",
                "unit-a",
                "workspace-a",
                "repo-a",
                &branch_a,
            )
            .await
            .unwrap();
        assert_eq!(recovered, a);
        assert!(fs::try_exists(&b).await.unwrap());
        assert!(fs::try_exists(&c).await.unwrap());
        assert!(fs::try_exists(&integration).await.unwrap());

        // Legacy Task-wide cleanup owns only root/<task_id>; WorkUnit paths
        // live under root/work_units and survive intact.
        manager.cleanup_worktree("task-a").await.unwrap();
        assert!(!fs::try_exists(&integration).await.unwrap());
        assert!(fs::try_exists(&recovered).await.unwrap());
        assert!(fs::try_exists(&b).await.unwrap());
        assert!(fs::try_exists(&c).await.unwrap());
    }

    #[test]
    fn work_unit_identity_rejects_path_injection_and_uses_full_ids() {
        let branch_a = work_unit_branch_name("task-a", "unit-a", "workspace-a").unwrap();
        let branch_b = work_unit_branch_name("task-a", "unit-b", "workspace-b").unwrap();
        assert_ne!(branch_a, branch_b);
        assert!(matches!(
            work_unit_branch_name("../task", "unit-a", "workspace-a"),
            Err(WorkspaceError::InvalidIdentity)
        ));
    }
}
