#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use tempfile::TempDir;
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git command failed: {command}\nstdout: {stdout}\nstderr: {stderr}")]
    CommandFailed {
        command: String,
        stdout: String,
        stderr: String,
    },

    #[error("merge conflict in {path}: {stderr}")]
    MergeConflict { path: String, stderr: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, GitError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchList {
    pub branches: Vec<String>,
    pub default_branch: Option<String>,
    pub origin_url: Option<String>,
}

async fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await?;

    if !output.status.success() {
        return Err(GitError::CommandFailed {
            command: format!("git {}", args.join(" ")),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn run_git_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await?;

    if !output.status.success() {
        return Err(GitError::CommandFailed {
            command: format!("git {}", args.join(" ")),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }

    Ok(output.stdout)
}

pub async fn is_git_repo(path: &Path) -> bool {
    tokio::fs::symlink_metadata(path.join(".git")).await.is_ok()
}

pub async fn list_branches(path: &Path) -> Result<BranchList> {
    let output = run_git(
        path,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .await?;
    let branches = output
        .lines()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let default_branch = match run_git(path, &["symbolic-ref", "--short", "HEAD"]).await {
        Ok(branch) if !branch.is_empty() => Some(branch),
        Ok(_) => None,
        Err(GitError::CommandFailed { .. }) => None,
        Err(error) => return Err(error),
    };
    let origin_url = match run_git(path, &["remote", "get-url", "origin"]).await {
        Ok(url) if !url.is_empty() => Some(url),
        Ok(_) => None,
        Err(GitError::CommandFailed { .. }) => None,
        Err(error) => return Err(error),
    };

    Ok(BranchList {
        branches,
        default_branch,
        origin_url,
    })
}

/// Create a new worktree from an existing repo.
pub async fn create_worktree(
    repo_path: &Path,
    branch_name: &str,
    worktree_path: &Path,
) -> Result<()> {
    run_git(
        repo_path,
        &[
            "worktree",
            "add",
            "-b",
            branch_name,
            &worktree_path.to_string_lossy(),
        ],
    )
    .await?;
    Ok(())
}

/// Remove a worktree.
pub async fn remove_worktree(repo_path: &Path, worktree_path: &Path) -> Result<()> {
    run_git(
        repo_path,
        &[
            "worktree",
            "remove",
            "--force",
            &worktree_path.to_string_lossy(),
        ],
    )
    .await?;
    Ok(())
}

/// Get the current HEAD SHA.
pub async fn get_current_sha(worktree_path: &Path) -> Result<String> {
    run_git(worktree_path, &["rev-parse", "HEAD"]).await
}

pub async fn get_current_branch(worktree_path: &Path) -> Result<String> {
    run_git(worktree_path, &["symbolic-ref", "--short", "HEAD"]).await
}

/// Check if the worktree has no uncommitted changes.
pub async fn is_worktree_clean(worktree_path: &Path) -> Result<bool> {
    let output = run_git(worktree_path, &["status", "--porcelain"]).await?;
    Ok(output.is_empty())
}

/// List uncommitted worktree changes using git porcelain output.
pub async fn status_porcelain(worktree_path: &Path) -> Result<Vec<String>> {
    let output = run_git(worktree_path, &["status", "--porcelain"]).await?;
    Ok(output
        .lines()
        .map(|line| line.get(3..).unwrap_or(line).trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect())
}

/// Restore a managed worktree to an exact commit and remove untracked files.
///
/// This enforces read-only execution roles even when an underlying CLI creates
/// commits automatically.
pub async fn restore_worktree(worktree_path: &Path, commit_sha: &str) -> Result<()> {
    run_git(worktree_path, &["reset", "--hard", commit_sha]).await?;
    run_git(worktree_path, &["clean", "-fd"]).await?;
    Ok(())
}

/// A temporary, exact snapshot of the worktree state used by local Review.
/// The snapshot lives outside the worktree and never uses the user's stash.
pub struct WorktreeStateSnapshot {
    directory: TempDir,
    head_sha: String,
    index_permissions: std::fs::Permissions,
    tracked_file_permissions: Vec<(PathBuf, std::fs::Permissions)>,
    untracked_paths: Vec<PathBuf>,
    ignored_paths: Vec<PathBuf>,
}

impl WorktreeStateSnapshot {
    pub fn head_sha(&self) -> &str {
        &self.head_sha
    }

    /// Keep the isolated backup after a failed restore so an operator can
    /// inspect or recover the pre-review state.
    pub fn preserve_for_diagnostics(self) -> PathBuf {
        self.directory.keep()
    }

    /// Restore the original HEAD, tracked worktree bytes, exact Git index,
    /// and non-ignored untracked files. Ignored files are outside the
    /// workspace snapshot contract and are left untouched.
    pub async fn restore(&self, worktree_path: &Path) -> Result<()> {
        run_git(worktree_path, &["reset", "--hard", &self.head_sha]).await?;

        let pre_patch_untracked = list_untracked_paths(worktree_path, false).await?;
        for relative_path in pre_patch_untracked {
            if self.untracked_paths.contains(&relative_path)
                || is_within_any_path(&relative_path, &self.ignored_paths)
            {
                continue;
            }
            remove_worktree_entry(worktree_path, &relative_path).await?;
        }

        let tracked_patch = self.directory.path().join("tracked.patch");
        let tracked_patch_bytes = tokio::fs::read(&tracked_patch).await?;
        if !tracked_patch_bytes.is_empty() {
            run_git(
                worktree_path,
                &[
                    "apply",
                    "--binary",
                    "--whitespace=nowarn",
                    tracked_patch
                        .to_str()
                        .ok_or_else(|| GitError::CommandFailed {
                            command: "git apply".to_owned(),
                            stdout: String::new(),
                            stderr: "snapshot patch path is not valid UTF-8".to_owned(),
                        })?,
                ],
            )
            .await?;
        }

        // Re-evaluate ignored paths after restoring the original tracked
        // .gitignore files. Protect anything that was ignored before Review,
        // even if the reviewer changed an ignore rule while running.
        let current_untracked = list_untracked_paths(worktree_path, false).await?;
        for relative_path in current_untracked {
            if self.untracked_paths.contains(&relative_path)
                || is_within_any_path(&relative_path, &self.ignored_paths)
            {
                continue;
            }
            remove_worktree_entry(worktree_path, &relative_path).await?;
        }

        let backup_root = self.directory.path().join("untracked");
        for relative_path in &self.untracked_paths {
            remove_worktree_entry(worktree_path, relative_path).await?;
            copy_worktree_entry(&backup_root, worktree_path, relative_path).await?;
        }

        for (relative_path, permissions) in &self.tracked_file_permissions {
            ensure_safe_parent_directories(worktree_path, relative_path).await?;
            let path = worktree_path.join(relative_path);
            let metadata = tokio::fs::symlink_metadata(&path).await?;
            if !metadata.is_file() {
                return Err(GitError::CommandFailed {
                    command: "restore tracked file permissions".to_owned(),
                    stdout: path.display().to_string(),
                    stderr: "tracked file type changed during Review".to_owned(),
                });
            }
            tokio::fs::set_permissions(path, permissions.clone()).await?;
        }

        let index_backup = self.directory.path().join("index");
        let current_index = resolve_git_path(worktree_path, "index").await?;
        let index_parent = current_index
            .parent()
            .ok_or_else(|| GitError::CommandFailed {
                command: "git rev-parse --git-path index".to_owned(),
                stdout: current_index.display().to_string(),
                stderr: "Git index path has no parent directory".to_owned(),
            })?;
        let restore_index = index_parent.join(format!(
            ".forge-review-index-{}",
            self.directory
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("snapshot")
        ));
        tokio::fs::copy(&index_backup, &restore_index).await?;
        tokio::fs::set_permissions(&restore_index, self.index_permissions.clone()).await?;
        tokio::fs::rename(&restore_index, &current_index).await?;

        let restored_head = get_current_sha(worktree_path).await?;
        if restored_head != self.head_sha {
            return Err(GitError::CommandFailed {
                command: "git rev-parse HEAD".to_owned(),
                stdout: restored_head,
                stderr: "Review restore did not return to the original HEAD".to_owned(),
            });
        }
        Ok(())
    }
}

/// Capture the exact input state before a local Review starts. Staged state
/// is preserved by saving the real index file; tracked worktree content is
/// stored as a binary patch against HEAD; untracked files, symlinks, and
/// their relevant permissions are copied into an isolated temporary folder.
pub async fn capture_worktree_state(worktree_path: &Path) -> Result<WorktreeStateSnapshot> {
    let canonical_worktree = tokio::fs::canonicalize(worktree_path).await?;
    let temporary_root = tokio::fs::canonicalize(std::env::temp_dir()).await?;
    if temporary_root.starts_with(&canonical_worktree) {
        return Err(GitError::CommandFailed {
            command: "capture worktree state".to_owned(),
            stdout: temporary_root.display().to_string(),
            stderr: "system temporary directory is inside the Review worktree".to_owned(),
        });
    }
    let directory = tempfile::tempdir_in(temporary_root)?;
    let canonical_snapshot = tokio::fs::canonicalize(directory.path()).await?;
    if canonical_snapshot.starts_with(&canonical_worktree) {
        return Err(GitError::CommandFailed {
            command: "capture worktree state".to_owned(),
            stdout: canonical_snapshot.display().to_string(),
            stderr: "Review state snapshot must be outside the worktree".to_owned(),
        });
    }
    let head_sha = get_current_sha(worktree_path).await?;
    let tracked_patch = run_git_bytes(
        worktree_path,
        &[
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            "--",
        ],
    )
    .await?;
    tokio::fs::write(directory.path().join("tracked.patch"), tracked_patch).await?;

    let index_path = resolve_git_path(worktree_path, "index").await?;
    let index_metadata = tokio::fs::metadata(&index_path).await?;
    tokio::fs::copy(&index_path, directory.path().join("index")).await?;

    let mut tracked_file_permissions = Vec::new();
    for relative_path in list_tracked_paths(worktree_path).await? {
        let path = worktree_path.join(&relative_path);
        match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) if metadata.is_file() => {
                tracked_file_permissions.push((relative_path, metadata.permissions()));
            }
            Ok(metadata) if metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(GitError::CommandFailed {
                    command: "snapshot tracked workspace file".to_owned(),
                    stdout: path.display().to_string(),
                    stderr: "tracked workspace entry is not a regular file or symlink".to_owned(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    let untracked_paths = list_untracked_paths(worktree_path, false).await?;
    let ignored_paths = list_untracked_paths(worktree_path, true).await?;
    let backup_root = directory.path().join("untracked");
    for relative_path in &untracked_paths {
        copy_worktree_entry(worktree_path, &backup_root, relative_path).await?;
    }

    let original_index = tokio::fs::read(directory.path().join("index")).await?;
    if tokio::fs::read(&index_path).await? != original_index {
        return Err(GitError::CommandFailed {
            command: "snapshot Git index".to_owned(),
            stdout: index_path.display().to_string(),
            stderr: "Git index changed while the pre-review state was captured".to_owned(),
        });
    }
    for (relative_path, permissions) in &tracked_file_permissions {
        let path = worktree_path.join(relative_path);
        let current = tokio::fs::symlink_metadata(&path).await?.permissions();
        if !same_permissions(&current, permissions) {
            return Err(GitError::CommandFailed {
                command: "snapshot tracked file permissions".to_owned(),
                stdout: path.display().to_string(),
                stderr: "tracked file permissions changed while the pre-review state was captured"
                    .to_owned(),
            });
        }
    }

    Ok(WorktreeStateSnapshot {
        directory,
        head_sha,
        index_permissions: index_metadata.permissions(),
        tracked_file_permissions,
        untracked_paths,
        ignored_paths,
    })
}

async fn list_tracked_paths(worktree_path: &Path) -> Result<Vec<PathBuf>> {
    let output = run_git_bytes(worktree_path, &["ls-files", "--cached", "-z"]).await?;
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let relative_path = path_from_git_bytes(path)?;
            validate_relative_path(&relative_path)?;
            Ok(relative_path)
        })
        .collect()
}

#[cfg(unix)]
fn same_permissions(left: &std::fs::Permissions, right: &std::fs::Permissions) -> bool {
    use std::os::unix::fs::PermissionsExt;
    left.mode() == right.mode()
}

#[cfg(not(unix))]
fn same_permissions(left: &std::fs::Permissions, right: &std::fs::Permissions) -> bool {
    left.readonly() == right.readonly()
}

async fn resolve_git_path(worktree_path: &Path, path: &str) -> Result<PathBuf> {
    let resolved = run_git(worktree_path, &["rev-parse", "--git-path", path]).await?;
    let resolved = PathBuf::from(resolved);
    Ok(if resolved.is_absolute() {
        resolved
    } else {
        worktree_path.join(resolved)
    })
}

async fn list_untracked_paths(worktree_path: &Path, ignored: bool) -> Result<Vec<PathBuf>> {
    let args = if ignored {
        vec![
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ]
    } else {
        vec!["ls-files", "--others", "--exclude-standard", "-z"]
    };
    let output = run_git_bytes(worktree_path, &args).await?;
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let relative_path = path_from_git_bytes(path)?;
            validate_relative_path(&relative_path)?;
            Ok(relative_path)
        })
        .collect()
}

fn path_from_git_bytes(path: &[u8]) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(path)))
    }
    #[cfg(not(unix))]
    {
        let path = std::str::from_utf8(path).map_err(|_| GitError::CommandFailed {
            command: "git ls-files --others".to_owned(),
            stdout: String::new(),
            stderr: "worktree contains a non-UTF8 untracked path".to_owned(),
        })?;
        Ok(PathBuf::from(path))
    }
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(GitError::CommandFailed {
            command: "git ls-files --others".to_owned(),
            stdout: path.display().to_string(),
            stderr: "Git returned an unsafe worktree path".to_owned(),
        });
    }
    Ok(())
}

fn is_within_any_path(path: &Path, parents: &[PathBuf]) -> bool {
    parents
        .iter()
        .any(|parent| path == parent || path.starts_with(parent))
}

async fn copy_worktree_entry(
    source_root: &Path,
    target_root: &Path,
    relative: &Path,
) -> Result<()> {
    validate_relative_path(relative)?;
    let source = source_root.join(relative);
    let target = target_root.join(relative);
    let metadata = tokio::fs::symlink_metadata(&source).await?;
    tokio::fs::create_dir_all(target_root).await?;
    ensure_safe_parent_directories(target_root, relative).await?;
    if metadata.file_type().is_symlink() {
        let target_path = tokio::fs::read_link(&source).await?;
        create_symlink(&target_path, &target, &source).await?;
    } else if metadata.is_file() {
        tokio::fs::copy(&source, &target).await?;
        tokio::fs::set_permissions(&target, metadata.permissions()).await?;
    } else {
        return Err(GitError::CommandFailed {
            command: "snapshot untracked workspace entry".to_owned(),
            stdout: source.display().to_string(),
            stderr: "untracked workspace entry is not a regular file or symlink".to_owned(),
        });
    }
    Ok(())
}

async fn ensure_safe_parent_directories(root: &Path, relative: &Path) -> Result<()> {
    let components = relative.components().collect::<Vec<_>>();
    let mut parent = root.to_path_buf();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        let Component::Normal(name) = component else {
            return Err(GitError::CommandFailed {
                command: "restore worktree entry".to_owned(),
                stdout: relative.display().to_string(),
                stderr: "worktree path has an unsafe parent component".to_owned(),
            });
        };
        parent.push(name);
        match tokio::fs::symlink_metadata(&parent).await {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                tokio::fs::remove_file(&parent).await?;
                tokio::fs::create_dir(&parent).await?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::create_dir(&parent).await?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn create_symlink(target: &Path, destination: &Path, _source: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, destination)?;
    Ok(())
}

#[cfg(windows)]
async fn create_symlink(target: &Path, destination: &Path, source: &Path) -> Result<()> {
    let points_to_directory = tokio::fs::metadata(source)
        .await
        .map(|metadata| metadata.is_dir())
        .map_err(|error| GitError::CommandFailed {
            command: "snapshot untracked workspace symlink".to_owned(),
            stdout: source.display().to_string(),
            stderr: format!("could not determine the symlink target kind: {error}"),
        })?;
    if points_to_directory {
        std::os::windows::fs::symlink_dir(target, destination)?;
    } else {
        std::os::windows::fs::symlink_file(target, destination)?;
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
async fn create_symlink(_target: &Path, source: &Path, _original: &Path) -> Result<()> {
    Err(GitError::CommandFailed {
        command: "snapshot untracked workspace symlink".to_owned(),
        stdout: source.display().to_string(),
        stderr: "symlink restoration is unsupported on this platform".to_owned(),
    })
}

async fn remove_worktree_entry(worktree_path: &Path, relative: &Path) -> Result<()> {
    validate_relative_path(relative)?;
    let mut parent = worktree_path.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        let Component::Normal(name) = component else {
            return Err(GitError::CommandFailed {
                command: "restore worktree entry".to_owned(),
                stdout: relative.display().to_string(),
                stderr: "worktree path has an unsafe parent component".to_owned(),
            });
        };
        parent.push(name);
        match tokio::fs::symlink_metadata(&parent).await {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    let Some(Component::Normal(name)) = components.last() else {
        return Err(GitError::CommandFailed {
            command: "restore worktree entry".to_owned(),
            stdout: relative.display().to_string(),
            stderr: "worktree path has no safe final component".to_owned(),
        });
    };
    let path = parent.join(name);
    match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            tokio::fs::remove_dir(&path).await?;
        }
        Ok(_) => tokio::fs::remove_file(&path).await?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Check out a branch in a repo or worktree.
pub async fn branch_exists(repo_path: &Path, branch_name: &str) -> Result<bool> {
    let result = run_git(
        repo_path,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch_name}"),
        ],
    )
    .await;
    match result {
        Ok(_) => Ok(true),
        Err(GitError::CommandFailed { .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

pub async fn checkout_branch(repo_path: &Path, branch_name: &str) -> Result<()> {
    run_git(repo_path, &["checkout", branch_name]).await?;
    Ok(())
}

/// Attempt to merge a branch into the current worktree HEAD.
pub async fn merge(worktree_path: &Path, target_branch: &str) -> Result<()> {
    let result = run_git(worktree_path, &["merge", target_branch, "--no-edit"]).await;
    match result {
        Ok(_) => Ok(()),
        Err(GitError::CommandFailed { stdout, stderr, .. })
            if stdout.contains("CONFLICT") || stderr.contains("CONFLICT") =>
        {
            let details = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            Err(GitError::MergeConflict {
                path: worktree_path.to_string_lossy().to_string(),
                stderr: details,
            })
        }
        Err(e) => Err(e),
    }
}

/// Attempt to merge a branch into the currently checked-out branch.
pub async fn merge_branch_into(repo_path: &Path, branch_name: &str) -> Result<()> {
    merge(repo_path, branch_name).await
}

/// Abort an in-progress merge.
pub async fn abort_merge(worktree_path: &Path) -> Result<()> {
    run_git(worktree_path, &["merge", "--abort"]).await?;
    Ok(())
}

/// Return the exact SHA recorded in `MERGE_HEAD`, if Git has an interrupted
/// merge in this worktree.
pub async fn get_merge_head(worktree_path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["rev-parse", "--quiet", "--verify", "MERGE_HEAD"])
        .current_dir(worktree_path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await?;
    if output.status.success() {
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        return Ok((!sha.is_empty()).then_some(sha));
    }
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    Err(GitError::CommandFailed {
        command: "git rev-parse --quiet --verify MERGE_HEAD".to_owned(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Read the direct parent SHAs for an exact commit object.
pub async fn commit_parents(repo_path: &Path, commit_sha: &str) -> Result<Vec<String>> {
    let output = run_git(repo_path, &["rev-list", "--parents", "-n", "1", commit_sha]).await?;
    let mut fields = output.split_whitespace();
    let _commit = fields.next().ok_or_else(|| GitError::CommandFailed {
        command: format!("git rev-list --parents -n 1 {commit_sha}"),
        stdout: output.clone(),
        stderr: "Git returned no commit identity".to_owned(),
    })?;
    Ok(fields.map(str::to_owned).collect())
}

/// Check ancestry without treating Git's normal exit code 1 as an error.
pub async fn is_ancestor(
    repo_path: &Path,
    ancestor_sha: &str,
    descendant_sha: &str,
) -> Result<bool> {
    let args = ["merge-base", "--is-ancestor", ancestor_sha, descendant_sha];
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await?;
    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    Err(GitError::CommandFailed {
        command: format!("git {}", args.join(" ")),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Detect if there is an interrupted merge.
pub async fn detect_interrupted_merge(worktree_path: &Path) -> Result<bool> {
    Ok(get_merge_head(worktree_path).await?.is_some())
}

/// Get diff between a base SHA and HEAD.
pub async fn get_diff(worktree_path: &Path, base_sha: &str) -> Result<String> {
    run_git(worktree_path, &["diff", &format!("{base_sha}..HEAD")]).await
}

/// Stage all worktree changes.
pub async fn stage_all(path: &Path) -> Result<()> {
    run_git(path, &["add", "-A"]).await?;
    Ok(())
}

/// Commit currently staged changes.
pub async fn commit_with_message(path: &Path, message: &str) -> Result<String> {
    run_git(path, &["commit", "-m", message]).await?;
    get_current_sha(path).await
}

/// Count commits in the half-open range before_sha..after_sha.
pub async fn count_commits_between(path: &Path, before_sha: &str, after_sha: &str) -> Result<u64> {
    let range = format!("{before_sha}..{after_sha}");
    let output = run_git(path, &["rev-list", "--count", &range]).await?;
    output
        .parse::<u64>()
        .map_err(|error| GitError::CommandFailed {
            command: format!("git rev-list --count {range}"),
            stdout: output,
            stderr: error.to_string(),
        })
}

// ---------------------------------------------------------------------------
// Rebase
// ---------------------------------------------------------------------------

/// Rebase the current branch onto `onto_branch`.
pub async fn rebase(worktree_path: &Path, onto_branch: &str) -> Result<()> {
    let result = run_git(worktree_path, &["rebase", onto_branch]).await;
    match result {
        Ok(_) => Ok(()),
        Err(GitError::CommandFailed { stdout, stderr, .. })
            if stdout.contains("CONFLICT")
                || stderr.contains("CONFLICT")
                || stderr.contains("could not apply") =>
        {
            let details = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            Err(GitError::MergeConflict {
                path: worktree_path.to_string_lossy().to_string(),
                stderr: details,
            })
        }
        Err(e) => Err(e),
    }
}

/// Abort an in-progress rebase.
pub async fn abort_rebase(worktree_path: &Path) -> Result<()> {
    run_git(worktree_path, &["rebase", "--abort"]).await?;
    Ok(())
}

/// Continue a paused rebase (after conflict resolution).
pub async fn continue_rebase(worktree_path: &Path) -> Result<()> {
    run_git(worktree_path, &["rebase", "--continue"]).await?;
    Ok(())
}

/// Detect if a rebase is in progress via `git rev-parse`.
pub async fn detect_rebase_in_progress(worktree_path: &Path) -> Result<bool> {
    let result = Command::new("git")
        .args(["rev-parse", "--git-path", "rebase-merge"])
        .current_dir(worktree_path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await?;
    if result.status.success() {
        let git_path = String::from_utf8_lossy(&result.stdout).trim().to_string();
        let abs = if Path::new(&git_path).is_absolute() {
            PathBuf::from(&git_path)
        } else {
            worktree_path.join(&git_path)
        };
        if tokio::fs::symlink_metadata(&abs).await.is_ok() {
            return Ok(true);
        }
    }

    let result = Command::new("git")
        .args(["rev-parse", "--git-path", "rebase-apply"])
        .current_dir(worktree_path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await?;
    if result.status.success() {
        let git_path = String::from_utf8_lossy(&result.stdout).trim().to_string();
        let abs = if Path::new(&git_path).is_absolute() {
            PathBuf::from(&git_path)
        } else {
            worktree_path.join(&git_path)
        };
        if tokio::fs::symlink_metadata(&abs).await.is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}

// ---------------------------------------------------------------------------
// Conflict helpers
// ---------------------------------------------------------------------------

/// The kind of conflict operation currently in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictOperation {
    Merge,
    Rebase,
    None,
}

/// Detect what conflict operation is in progress (if any).
pub async fn detect_conflict_state(worktree_path: &Path) -> Result<ConflictOperation> {
    if detect_interrupted_merge(worktree_path).await? {
        return Ok(ConflictOperation::Merge);
    }
    if detect_rebase_in_progress(worktree_path).await? {
        return Ok(ConflictOperation::Rebase);
    }
    Ok(ConflictOperation::None)
}

/// List paths with unresolved conflicts.
pub async fn conflict_paths(worktree_path: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=U"])
        .current_dir(worktree_path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await?;

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Abort whatever conflict operation is in progress.
pub async fn abort_conflict(worktree_path: &Path) -> Result<()> {
    match detect_conflict_state(worktree_path).await? {
        ConflictOperation::Merge => abort_merge(worktree_path).await,
        ConflictOperation::Rebase => abort_rebase(worktree_path).await,
        ConflictOperation::None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

/// Fetch from a remote (defaults to "origin").
pub async fn fetch(repo_path: &Path, remote: Option<&str>) -> Result<()> {
    let remote = remote.unwrap_or("origin");
    run_git(repo_path, &["fetch", remote]).await?;
    Ok(())
}

/// Fetch a specific branch from a remote.
pub async fn fetch_branch(repo_path: &Path, remote: &str, branch: &str) -> Result<()> {
    run_git(repo_path, &["fetch", remote, branch]).await?;
    Ok(())
}

/// Pull from the configured upstream using `--ff-only` to avoid creating
/// surprise merge commits. Returns the trimmed stdout from git.
pub async fn pull_ff_only(repo_path: &Path) -> Result<String> {
    run_git(repo_path, &["pull", "--ff-only"]).await
}

/// Push the current branch to its upstream (no force).
pub async fn push(repo_path: &Path) -> Result<String> {
    run_git(repo_path, &["push"]).await
}

// ---------------------------------------------------------------------------
// Testing
// ---------------------------------------------------------------------------

/// Initialize a new git repo (for testing).
pub async fn init(path: &Path) -> Result<()> {
    run_git(path, &["init"]).await?;
    // Set required config for commits
    run_git(path, &["config", "user.email", "test@forge.dev"]).await?;
    run_git(path, &["config", "user.name", "Forge Test"]).await?;
    Ok(())
}

/// Stage all files and commit.
pub async fn commit_all(path: &Path, message: &str) -> Result<String> {
    stage_all(path).await?;
    run_git(path, &["commit", "-m", message, "--allow-empty"]).await?;
    get_current_sha(path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs;

    async fn setup_repo() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let repo_path = dir.path().to_path_buf();
        init(&repo_path).await.unwrap();

        // Create initial commit
        fs::write(repo_path.join("README.md"), "# Test")
            .await
            .unwrap();
        commit_all(&repo_path, "initial commit").await.unwrap();

        (dir, repo_path)
    }

    #[tokio::test]
    async fn is_git_repo_true() {
        let dir = TempDir::new().unwrap();
        init(dir.path()).await.unwrap();

        assert!(is_git_repo(dir.path()).await);
    }

    #[tokio::test]
    async fn is_git_repo_false() {
        let dir = TempDir::new().unwrap();

        assert!(!is_git_repo(dir.path()).await);
    }

    #[tokio::test]
    async fn is_git_repo_worktree() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".git"), "gitdir: /some/path")
            .await
            .unwrap();

        assert!(is_git_repo(dir.path()).await);
    }

    #[tokio::test]
    async fn is_git_repo_subfolder_of_repo_is_false() {
        let dir = TempDir::new().unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .current_dir(dir.path())
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .status()
            .unwrap();
        assert!(status.success());

        let subdir = dir.path().join("subdir");
        fs::create_dir(&subdir).await.unwrap();

        assert!(!is_git_repo(&subdir).await);
    }

    #[tokio::test]
    async fn list_branches_basic() {
        let dir = TempDir::new().unwrap();
        let repo_path = dir.path().to_path_buf();
        init(&repo_path).await.unwrap();
        fs::write(repo_path.join("README.md"), "# Test")
            .await
            .unwrap();
        commit_all(&repo_path, "initial commit").await.unwrap();

        let initial_branch = run_git(&repo_path, &["symbolic-ref", "--short", "HEAD"])
            .await
            .unwrap();
        let extra_branch = "extra";
        run_git(&repo_path, &["branch", extra_branch])
            .await
            .unwrap();

        let branch_list = list_branches(&repo_path).await.unwrap();

        assert!(branch_list.branches.contains(&initial_branch));
        assert!(branch_list.branches.contains(&extra_branch.to_owned()));
        assert_eq!(branch_list.default_branch, Some(initial_branch));
    }

    #[tokio::test]
    async fn test_create_worktree_and_get_sha() {
        let (dir, repo_path) = setup_repo().await;
        let worktree_path = dir.path().join("worktree1");

        create_worktree(&repo_path, "forge/task-1", &worktree_path)
            .await
            .unwrap();

        let sha = get_current_sha(&worktree_path).await.unwrap();
        assert!(!sha.is_empty());
        assert_eq!(sha.len(), 40); // SHA-1 hex

        let clean = is_worktree_clean(&worktree_path).await.unwrap();
        assert!(clean);

        // Write a file in worktree
        fs::write(worktree_path.join("new_file.txt"), "hello")
            .await
            .unwrap();

        let dirty = is_worktree_clean(&worktree_path).await.unwrap();
        assert!(!dirty);

        // Commit and get new SHA
        let new_sha = commit_all(&worktree_path, "add file").await.unwrap();
        assert_ne!(sha, new_sha);

        // Get diff
        let diff = get_diff(&worktree_path, &sha).await.unwrap();
        assert!(diff.contains("new_file.txt"));

        // Cleanup
        remove_worktree(&repo_path, &worktree_path).await.unwrap();
    }

    #[tokio::test]
    async fn restore_worktree_discards_commits_and_untracked_files() {
        let (_dir, repo_path) = setup_repo().await;
        let original_sha = get_current_sha(&repo_path).await.unwrap();

        fs::write(repo_path.join("README.md"), "changed by reviewer")
            .await
            .unwrap();
        fs::write(repo_path.join("reviewer.tmp"), "untracked")
            .await
            .unwrap();
        commit_all(&repo_path, "reviewer mutation").await.unwrap();
        fs::write(repo_path.join("leftover.tmp"), "untracked")
            .await
            .unwrap();

        restore_worktree(&repo_path, &original_sha).await.unwrap();

        assert_eq!(get_current_sha(&repo_path).await.unwrap(), original_sha);
        assert_eq!(
            fs::read_to_string(repo_path.join("README.md"))
                .await
                .unwrap(),
            "# Test"
        );
        assert!(!repo_path.join("reviewer.tmp").exists());
        assert!(!repo_path.join("leftover.tmp").exists());
        assert!(is_worktree_clean(&repo_path).await.unwrap());
    }

    #[tokio::test]
    async fn capture_restore_worktree_state_preserves_exact_pre_review_state() {
        let (_dir, repo_path) = setup_repo().await;
        let original_head = get_current_sha(&repo_path).await.unwrap();
        let untracked_dir = repo_path.join("review-input");
        fs::create_dir_all(&untracked_dir).await.unwrap();
        fs::write(repo_path.join("README.md"), "staged subject\n")
            .await
            .unwrap();
        run_git(&repo_path, &["add", "README.md"]).await.unwrap();
        fs::write(repo_path.join("README.md"), "unstaged subject\n")
            .await
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                repo_path.join("README.md"),
                std::fs::Permissions::from_mode(0o600),
            )
            .await
            .unwrap();
        }
        fs::write(untracked_dir.join("notes.txt"), "pre-review untracked\n")
            .await
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                untracked_dir.join("notes.txt"),
                std::fs::Permissions::from_mode(0o751),
            )
            .await
            .unwrap();
            std::os::unix::fs::symlink("notes.txt", untracked_dir.join("notes-link")).unwrap();
        }
        let staged_before = run_git_bytes(&repo_path, &["diff", "--cached", "--binary"])
            .await
            .unwrap();
        let tracked_before = run_git_bytes(
            &repo_path,
            &[
                "diff",
                "--binary",
                "--no-ext-diff",
                "--no-textconv",
                "HEAD",
                "--",
            ],
        )
        .await
        .unwrap();
        let status_before = run_git(&repo_path, &["status", "--porcelain"])
            .await
            .unwrap();
        let snapshot = capture_worktree_state(&repo_path).await.unwrap();
        assert!(!snapshot
            .directory
            .path()
            .starts_with(std::fs::canonicalize(&repo_path).unwrap()));

        fs::write(repo_path.join("README.md"), "reviewer mutation\n")
            .await
            .unwrap();
        fs::write(
            untracked_dir.join("notes.txt"),
            "reviewer overwrote input\n",
        )
        .await
        .unwrap();
        fs::remove_file(untracked_dir.join("notes-link"))
            .await
            .unwrap();
        commit_all(&repo_path, "reviewer created commit")
            .await
            .unwrap();
        fs::write(
            repo_path.join("reviewer-created.tmp"),
            "reviewer temporary file",
        )
        .await
        .unwrap();

        snapshot.restore(&repo_path).await.unwrap();

        assert_eq!(get_current_sha(&repo_path).await.unwrap(), original_head);
        assert_eq!(
            fs::read_to_string(repo_path.join("README.md"))
                .await
                .unwrap(),
            "unstaged subject\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(repo_path.join("README.md"))
                    .await
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            run_git_bytes(&repo_path, &["diff", "--cached", "--binary"])
                .await
                .unwrap(),
            staged_before
        );
        assert_eq!(
            run_git_bytes(
                &repo_path,
                &["diff", "--binary", "--no-ext-diff", "HEAD", "--"],
            )
            .await
            .unwrap(),
            tracked_before
        );
        assert_eq!(
            run_git(&repo_path, &["status", "--porcelain"])
                .await
                .unwrap(),
            status_before
        );
        assert_eq!(
            fs::read_to_string(untracked_dir.join("notes.txt"))
                .await
                .unwrap(),
            "pre-review untracked\n"
        );
        assert!(!repo_path.join("reviewer-created.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(untracked_dir.join("notes.txt"))
                    .await
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o751
            );
            assert!(fs::symlink_metadata(untracked_dir.join("notes-link"))
                .await
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(
                fs::read_link(untracked_dir.join("notes-link"))
                    .await
                    .unwrap(),
                PathBuf::from("notes.txt")
            );
        }
    }

    #[tokio::test]
    async fn test_merge_conflict() {
        let (dir, repo_path) = setup_repo().await;

        // Create two branches with conflicting changes
        let wt1 = dir.path().join("wt1");
        create_worktree(&repo_path, "branch-a", &wt1).await.unwrap();
        fs::write(wt1.join("conflict.txt"), "branch a content")
            .await
            .unwrap();
        commit_all(&wt1, "branch a change").await.unwrap();

        let wt2 = dir.path().join("wt2");
        create_worktree(&repo_path, "branch-b", &wt2).await.unwrap();
        fs::write(wt2.join("conflict.txt"), "branch b content")
            .await
            .unwrap();
        commit_all(&wt2, "branch b change").await.unwrap();
        assert_eq!(get_merge_head(&wt2).await.unwrap(), None);

        // Merge branch-a into branch-b should conflict
        let result = merge(&wt2, "branch-a").await;
        assert!(result.is_err());

        // Detect interrupted merge
        let has_merge = detect_interrupted_merge(&wt2).await.unwrap();
        assert!(has_merge);
        assert!(get_merge_head(&wt2).await.unwrap().is_some());

        // Abort merge
        abort_merge(&wt2).await.unwrap();

        let has_merge_after = detect_interrupted_merge(&wt2).await.unwrap();
        assert!(!has_merge_after);
        assert_eq!(get_merge_head(&wt2).await.unwrap(), None);

        // Cleanup
        remove_worktree(&repo_path, &wt1).await.unwrap();
        remove_worktree(&repo_path, &wt2).await.unwrap();
    }

    #[tokio::test]
    async fn test_rebase_clean() {
        let (dir, repo_path) = setup_repo().await;

        let wt = dir.path().join("wt_rebase");
        create_worktree(&repo_path, "feature", &wt).await.unwrap();
        fs::write(wt.join("feature.txt"), "feature work")
            .await
            .unwrap();
        commit_all(&wt, "feature commit").await.unwrap();

        // Add a commit on main so there's something to rebase onto
        fs::write(repo_path.join("main_file.txt"), "main work")
            .await
            .unwrap();
        commit_all(&repo_path, "main commit").await.unwrap();

        // Rebase feature onto main (via the default branch ref)
        let default_branch = run_git(&repo_path, &["symbolic-ref", "--short", "HEAD"])
            .await
            .unwrap();
        rebase(&wt, &default_branch).await.unwrap();

        assert!(!detect_rebase_in_progress(&wt).await.unwrap());
        assert!(wt.join("feature.txt").exists());

        remove_worktree(&repo_path, &wt).await.unwrap();
    }

    #[tokio::test]
    async fn test_rebase_conflict_and_abort() {
        let (dir, repo_path) = setup_repo().await;

        let wt = dir.path().join("wt_rebase_conflict");
        create_worktree(&repo_path, "feat-conflict", &wt)
            .await
            .unwrap();
        fs::write(wt.join("README.md"), "feature change")
            .await
            .unwrap();
        commit_all(&wt, "feature change").await.unwrap();

        // Conflicting commit on main
        fs::write(repo_path.join("README.md"), "main change")
            .await
            .unwrap();
        commit_all(&repo_path, "main change").await.unwrap();

        let default_branch = run_git(&repo_path, &["symbolic-ref", "--short", "HEAD"])
            .await
            .unwrap();
        let result = rebase(&wt, &default_branch).await;
        assert!(result.is_err());

        assert!(detect_rebase_in_progress(&wt).await.unwrap());
        assert_eq!(
            detect_conflict_state(&wt).await.unwrap(),
            ConflictOperation::Rebase
        );

        let paths = conflict_paths(&wt).await.unwrap();
        assert!(paths.contains(&"README.md".to_owned()));

        abort_rebase(&wt).await.unwrap();
        assert!(!detect_rebase_in_progress(&wt).await.unwrap());

        remove_worktree(&repo_path, &wt).await.unwrap();
    }

    #[tokio::test]
    async fn test_detect_conflict_state_merge() {
        let (dir, repo_path) = setup_repo().await;

        let wt1 = dir.path().join("wt_cs1");
        create_worktree(&repo_path, "cs-a", &wt1).await.unwrap();
        fs::write(wt1.join("conflict.txt"), "a").await.unwrap();
        commit_all(&wt1, "a").await.unwrap();

        let wt2 = dir.path().join("wt_cs2");
        create_worktree(&repo_path, "cs-b", &wt2).await.unwrap();
        fs::write(wt2.join("conflict.txt"), "b").await.unwrap();
        commit_all(&wt2, "b").await.unwrap();

        let _ = merge(&wt2, "cs-a").await;
        assert_eq!(
            detect_conflict_state(&wt2).await.unwrap(),
            ConflictOperation::Merge
        );

        abort_conflict(&wt2).await.unwrap();
        assert_eq!(
            detect_conflict_state(&wt2).await.unwrap(),
            ConflictOperation::None
        );

        remove_worktree(&repo_path, &wt1).await.unwrap();
        remove_worktree(&repo_path, &wt2).await.unwrap();
    }
}
