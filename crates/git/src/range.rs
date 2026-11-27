//! Read-only helpers over exact commits, used by commit-range review.
//!
//! Revisions must not look like options, paths are literal, and output does not
//! depend on user diff, log, mailmap or blame settings.

use super::{run_git, run_git_bytes, GitError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;

fn ensure_revision(revision: &str) -> Result<()> {
    if revision.is_empty() || revision.starts_with('-') || revision.contains(['\0', '\n']) {
        return Err(GitError::CommandFailed {
            command: "git".to_owned(),
            stdout: String::new(),
            stderr: format!("invalid revision: {revision:?}"),
        });
    }
    Ok(())
}

fn unexpected_output(command: &str, output: &[u8]) -> GitError {
    GitError::CommandFailed {
        command: command.to_owned(),
        stdout: String::from_utf8_lossy(output).into_owned(),
        stderr: "Git returned unexpected output".to_owned(),
    }
}

/// Whether `revision` names a commit present in this repository.
pub async fn commit_exists(repo_path: &Path, revision: &str) -> Result<bool> {
    ensure_revision(revision)?;
    let commit = format!("{revision}^{{commit}}");
    let args = ["rev-parse", "--quiet", "--verify", commit.as_str()];
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

/// The tree object of an exact commit.
pub async fn resolve_tree(repo_path: &Path, commit: &str) -> Result<String> {
    ensure_revision(commit)?;
    let tree = format!("{commit}^{{tree}}");
    run_git(repo_path, &["rev-parse", "--verify", &tree]).await
}

/// Commits reachable from `head` but not from `exclude`, parents first.
pub async fn rev_list(repo_path: &Path, head: &str, exclude: &str) -> Result<Vec<String>> {
    walk(repo_path, head, exclude, &[]).await
}

/// The first-parent chain of `head` down to, and excluding, anything reachable
/// from `base`, oldest first.
pub async fn first_parent_commits(repo_path: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    walk(repo_path, head, base, &["--first-parent"]).await
}

async fn walk(repo_path: &Path, head: &str, exclude: &str, extra: &[&str]) -> Result<Vec<String>> {
    ensure_revision(head)?;
    ensure_revision(exclude)?;
    let excluded = format!("^{exclude}");
    let mut args = vec!["rev-list", "--topo-order", "--reverse"];
    args.extend_from_slice(extra);
    args.extend_from_slice(&[head, &excluded, "--"]);
    let output = run_git(repo_path, &args).await?;
    Ok(output.lines().map(str::to_owned).collect())
}

/// One path changed between two trees. A rename appears as a deletion plus an
/// addition. Binary files carry no line counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiffStat {
    pub path: String,
    pub added_lines: u64,
    pub deleted_lines: u64,
    pub binary: bool,
}

/// Per-file diffstat of the `base` to `head` tree diff, in Git path order.
pub async fn diffstat(repo_path: &Path, base: &str, head: &str) -> Result<Vec<FileDiffStat>> {
    ensure_revision(base)?;
    ensure_revision(head)?;
    let output = run_git_bytes(
        repo_path,
        &[
            "diff",
            "--numstat",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=none",
            "--diff-algorithm=myers",
            base,
            head,
            "--",
        ],
    )
    .await?;
    output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(parse_numstat_record)
        .collect()
}

fn parse_numstat_record(record: &[u8]) -> Result<FileDiffStat> {
    let malformed = || unexpected_output("git diff --numstat", record);
    let mut fields = record.splitn(3, |byte| *byte == b'\t');
    let (Some(added), Some(deleted), Some(path)) = (fields.next(), fields.next(), fields.next())
    else {
        return Err(malformed());
    };
    let binary = added == b"-" && deleted == b"-";
    let count = |field: &[u8]| -> Result<u64> {
        if binary {
            return Ok(0);
        }
        std::str::from_utf8(field)
            .ok()
            .and_then(|value| value.parse().ok())
            .ok_or_else(malformed)
    };
    Ok(FileDiffStat {
        path: String::from_utf8_lossy(path).into_owned(),
        added_lines: count(added)?,
        deleted_lines: count(deleted)?,
        binary,
    })
}

/// Totals of a range diff: a binary file counts as one file and zero lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumstatSummary {
    pub changed_files: u64,
    pub changed_lines: u64,
}

/// Changed files and changed (added plus deleted) lines of a range diff.
pub async fn numstat(repo_path: &Path, base: &str, head: &str) -> Result<NumstatSummary> {
    let files = diffstat(repo_path, base, head).await?;
    Ok(NumstatSummary {
        changed_files: files.len() as u64,
        changed_lines: files.iter().fold(0_u64, |total, file| {
            total
                .saturating_add(file.added_lines)
                .saturating_add(file.deleted_lines)
        }),
    })
}

/// Git author and committer of a commit. These are self-declared Git metadata:
/// informational only, never Actor attribution or authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitIdentity {
    pub commit: String,
    pub author_name: String,
    pub author_email: String,
    pub committer_name: String,
    pub committer_email: String,
}

/// Identities of every commit reachable from `head` but not from `base`,
/// parents first.
pub async fn commit_identities(
    repo_path: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<CommitIdentity>> {
    ensure_revision(base)?;
    ensure_revision(head)?;
    let excluded = format!("^{base}");
    let output = run_git_bytes(
        repo_path,
        &[
            "log",
            "--topo-order",
            "--reverse",
            "--no-show-signature",
            "--no-mailmap",
            "-z",
            "--format=%H%x01%an%x01%ae%x01%cn%x01%ce",
            head,
            &excluded,
            "--",
        ],
    )
    .await?;
    output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let fields = record
                .split(|byte| *byte == 1)
                .map(|field| String::from_utf8_lossy(field).into_owned())
                .collect::<Vec<_>>();
            let [commit, author_name, author_email, committer_name, committer_email] =
                <[String; 5]>::try_from(fields)
                    .map_err(|_| unexpected_output("git log", record))?;
            Ok(CommitIdentity {
                commit,
                author_name,
                author_email,
                committer_name,
                committer_email,
            })
        })
        .collect()
}

/// The commit that introduced one line, numbered as in the blamed commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub line: u32,
    pub commit: String,
}

/// Blame the inclusive, 1-based line interval `start..=end` of `path` as it
/// exists at `commit`.
pub async fn blame_lines(
    repo_path: &Path,
    commit: &str,
    path: &str,
    start: u32,
    end: u32,
) -> Result<Vec<BlameLine>> {
    ensure_revision(commit)?;
    let interval = format!("{start},{end}");
    if start == 0 || end < start {
        return Err(GitError::CommandFailed {
            command: format!("git blame -L {interval}"),
            stdout: String::new(),
            stderr: "invalid blame line interval".to_owned(),
        });
    }
    let output = run_git_bytes(
        repo_path,
        &[
            "--literal-pathspecs",
            "blame",
            "--porcelain",
            // An empty file name clears any configured ignore-revs list.
            "--ignore-revs-file=",
            "-L",
            &interval,
            commit,
            "--",
            path,
        ],
    )
    .await?;
    parse_blame_porcelain(&output)
}

/// Porcelain: a `<sha> <source line> <final line> [<count>]` header, optional
/// metadata, then the line content prefixed with a tab.
fn parse_blame_porcelain(output: &[u8]) -> Result<Vec<BlameLine>> {
    let mut lines = Vec::new();
    let mut expect_header = true;
    for record in output.split(|byte| *byte == b'\n') {
        if record.is_empty() {
            continue;
        }
        if expect_header {
            let malformed = || unexpected_output("git blame", record);
            let header = String::from_utf8_lossy(record);
            let mut fields = header.split(' ');
            let (Some(commit), Some(_), Some(line)) = (fields.next(), fields.next(), fields.next())
            else {
                return Err(malformed());
            };
            lines.push(BlameLine {
                line: line.parse().map_err(|_| malformed())?,
                commit: commit.to_owned(),
            });
            expect_header = false;
        } else if record.first() == Some(&b'\t') {
            expect_header = true;
        }
    }
    Ok(lines)
}

/// Whether `path` names a non-directory entry in the tree of `commit`.
async fn tree_has_file(repo_path: &Path, commit: &str, path: &str) -> Result<bool> {
    let output = run_git_bytes(
        repo_path,
        &[
            "--literal-pathspecs",
            "ls-tree",
            "-z",
            "--full-tree",
            commit,
            "--",
            path,
        ],
    )
    .await?;
    Ok(output.split(|byte| *byte == 0).any(|record| {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            return false;
        };
        let is_tree = record[..tab]
            .split(|byte| *byte == b' ')
            .nth(1)
            .is_some_and(|kind| kind == b"tree");
        !is_tree && &record[tab + 1..] == path.as_bytes()
    }))
}

/// Whether a repository-relative file path exists at `head`, or existed at
/// `base` and is absent from `head`. Unsafe or directory paths are `false`.
pub async fn path_exists_or_deleted(
    repo_path: &Path,
    base: &str,
    head: &str,
    path: &str,
) -> Result<bool> {
    ensure_revision(base)?;
    ensure_revision(head)?;
    if path.contains(['\0', '\\'])
        || path
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Ok(false);
    }
    Ok(tree_has_file(repo_path, head, path).await? || tree_has_file(repo_path, base, path).await?)
}

/// The best common ancestor of two commits. Unrelated histories are an error.
pub async fn merge_base(repo_path: &Path, left: &str, right: &str) -> Result<String> {
    ensure_revision(left)?;
    ensure_revision(right)?;
    run_git(repo_path, &["merge-base", left, right]).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        commit_all, get_current_sha, init, is_ancestor, run_git_as_forge, FORGE_SYSTEM_EMAIL,
    };
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tokio::fs;

    struct Range {
        _dir: TempDir,
        repo: PathBuf,
        base: String,
        edit: String,
        side: String,
        merged: String,
    }

    /// base -> edit (text and binary), then a side branch merged with a merge
    /// commit made by Forge.
    async fn setup() -> Range {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().to_path_buf();
        init(&repo).await.unwrap();
        fs::write(repo.join("a.txt"), "one\ntwo\nthree\n")
            .await
            .unwrap();
        let base = commit_all(&repo, "base").await.unwrap();

        fs::write(repo.join("a.txt"), "one\nTWO\nthree\n")
            .await
            .unwrap();
        fs::write(repo.join("blob.bin"), [0_u8, 159, 146, 150])
            .await
            .unwrap();
        let edit = commit_all(&repo, "edit").await.unwrap();

        run_git(&repo, &["checkout", "-q", "-b", "side"])
            .await
            .unwrap();
        fs::write(repo.join("side file.txt"), "s1\ns2\n")
            .await
            .unwrap();
        let side = commit_all(&repo, "side").await.unwrap();
        run_git(&repo, &["checkout", "-q", "-"]).await.unwrap();
        run_git_as_forge(&repo, &["merge", "--no-ff", "--no-edit", "side"])
            .await
            .unwrap();
        let merged = get_current_sha(&repo).await.unwrap();

        Range {
            _dir: dir,
            repo,
            base,
            edit,
            side,
            merged,
        }
    }

    #[tokio::test]
    async fn review_range_diffstat_counts_lines_files_and_binaries() {
        let r = setup().await;

        let files = diffstat(&r.repo, &r.base, &r.merged).await.unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| (
                    file.path.as_str(),
                    file.added_lines,
                    file.deleted_lines,
                    file.binary
                ))
                .collect::<Vec<_>>(),
            vec![
                ("a.txt", 1, 1, false),
                ("blob.bin", 0, 0, true),
                ("side file.txt", 2, 0, false),
            ]
        );
        assert_eq!(
            numstat(&r.repo, &r.base, &r.merged).await.unwrap(),
            NumstatSummary {
                changed_files: 3,
                changed_lines: 4,
            }
        );
        assert_eq!(
            numstat(&r.repo, &r.merged, &r.merged).await.unwrap(),
            NumstatSummary {
                changed_files: 0,
                changed_lines: 0,
            }
        );
    }

    #[tokio::test]
    async fn review_range_diffstat_ignores_rename_configuration() {
        let r = setup().await;
        run_git(&r.repo, &["config", "diff.renames", "true"])
            .await
            .unwrap();
        run_git(&r.repo, &["mv", "a.txt", "b.txt"]).await.unwrap();
        let renamed = commit_all(&r.repo, "rename").await.unwrap();

        assert_eq!(
            numstat(&r.repo, &r.merged, &renamed).await.unwrap(),
            NumstatSummary {
                changed_files: 2,
                changed_lines: 6,
            }
        );
    }

    #[tokio::test]
    async fn review_range_walks_all_parents_and_first_parent() {
        let r = setup().await;

        assert_eq!(
            rev_list(&r.repo, &r.merged, &r.base).await.unwrap(),
            vec![r.edit.clone(), r.side.clone(), r.merged.clone()]
        );
        assert_eq!(
            first_parent_commits(&r.repo, &r.base, &r.merged)
                .await
                .unwrap(),
            vec![r.edit.clone(), r.merged.clone()]
        );
        assert!(rev_list(&r.repo, &r.base, &r.merged)
            .await
            .unwrap()
            .is_empty());
        assert!(is_ancestor(&r.repo, &r.side, &r.merged).await.unwrap());
        assert!(!is_ancestor(&r.repo, &r.side, &r.edit).await.unwrap());
        assert_eq!(
            merge_base(&r.repo, &r.side, &r.merged).await.unwrap(),
            r.side
        );
        assert_eq!(merge_base(&r.repo, &r.base, &r.side).await.unwrap(), r.base);
    }

    #[tokio::test]
    async fn review_range_resolves_trees_and_commit_existence() {
        let r = setup().await;

        assert!(commit_exists(&r.repo, &r.merged).await.unwrap());
        assert!(!commit_exists(&r.repo, &"0".repeat(40)).await.unwrap());
        assert_ne!(
            resolve_tree(&r.repo, &r.base).await.unwrap(),
            resolve_tree(&r.repo, &r.merged).await.unwrap()
        );
        assert!(resolve_tree(&r.repo, "--all").await.is_err());
        assert!(rev_list(&r.repo, "--all", &r.base).await.is_err());
    }

    #[tokio::test]
    async fn review_range_reports_commit_identities() {
        let r = setup().await;

        let identities = commit_identities(&r.repo, &r.base, &r.merged)
            .await
            .unwrap();
        assert_eq!(
            identities
                .iter()
                .map(|identity| identity.commit.as_str())
                .collect::<Vec<_>>(),
            vec![r.edit.as_str(), r.side.as_str(), r.merged.as_str()]
        );
        assert_eq!(identities[0].author_email, "test@forge.dev");
        assert_eq!(identities[2].committer_email, FORGE_SYSTEM_EMAIL);
    }

    #[tokio::test]
    async fn review_range_blames_exact_lines() {
        let r = setup().await;

        let line = |line, commit: &str| BlameLine {
            line,
            commit: commit.to_owned(),
        };
        assert_eq!(
            blame_lines(&r.repo, &r.merged, "a.txt", 1, 3)
                .await
                .unwrap(),
            vec![line(1, &r.base), line(2, &r.edit), line(3, &r.base)]
        );
        assert!(blame_lines(&r.repo, &r.merged, "a.txt", 0, 1)
            .await
            .is_err());
        assert!(blame_lines(&r.repo, &r.merged, "a.txt", 9, 9)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn review_range_accepts_only_existing_or_deleted_files() {
        let r = setup().await;
        fs::create_dir(r.repo.join("dir")).await.unwrap();
        fs::write(r.repo.join("dir/kept.txt"), "kept")
            .await
            .unwrap();
        fs::remove_file(r.repo.join("a.txt")).await.unwrap();
        let head = commit_all(&r.repo, "delete").await.unwrap();

        for path in ["a.txt", "side file.txt", "dir/kept.txt"] {
            assert!(
                path_exists_or_deleted(&r.repo, &r.merged, &head, path)
                    .await
                    .unwrap(),
                "{path}"
            );
        }
        for path in [
            "dir",
            "dir/",
            "missing.txt",
            "*.txt",
            "../a.txt",
            "/a.txt",
            "dir\\kept.txt",
            "",
        ] {
            assert!(
                !path_exists_or_deleted(&r.repo, &r.merged, &head, path)
                    .await
                    .unwrap(),
                "{path}"
            );
        }
    }
}
