use executors::ExecutorError;
use std::path::Path;
use tokio::process::Command;

const COMMIT_PREFIX: &str = "agent: ";
const SUBJECT_TEXT_LIMIT: usize = 72;

pub async fn commit_worktree_changes(
    worktree: &Path,
    message_subject: &str,
) -> Result<Option<String>, ExecutorError> {
    let status = run_git(worktree, &["status", "--porcelain"]).await?;
    if status.trim().is_empty() {
        return Ok(None);
    }

    run_git(worktree, &["add", "-A"]).await?;
    run_git(
        worktree,
        &[
            "-c",
            "user.email=agent@forge.local",
            "-c",
            "user.name=Forge Agent",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            message_subject,
        ],
    )
    .await?;

    let sha = run_git(worktree, &["rev-parse", "HEAD"]).await?;
    Ok(Some(sha.trim().to_owned()))
}

pub fn build_commit_subject(task_description: Option<&str>, fallback_title: &str) -> String {
    let subject_text = task_description
        .and_then(first_non_empty_line)
        .or_else(|| first_non_empty_line(fallback_title))
        .unwrap_or("task");
    format!(
        "{COMMIT_PREFIX}{}",
        truncate_chars(subject_text, SUBJECT_TEXT_LIMIT)
    )
}

async fn run_git(worktree: &Path, args: &[&str]) -> Result<String, ExecutorError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await?;
    if !output.status.success() {
        return Err(ExecutorError::Other(format!(
            "git -C {} {} failed with status {}\nstdout: {}\nstderr: {}",
            worktree.display(),
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn first_non_empty_line(value: &str) -> Option<&str> {
    value.lines().map(str::trim).find(|line| !line.is_empty())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn commits_dirty_worktree_and_returns_new_sha() {
        let tempdir = tempfile::tempdir().expect("tempdir creates");
        init_repo(tempdir.path()).await;
        let before = git_output(tempdir.path(), &["rev-parse", "HEAD"]).await;

        fs::write(tempdir.path().join("changed.txt"), "changed\n").expect("file writes");

        let subject = build_commit_subject(Some("Implement the thing\n\nDetails"), "Fallback");
        let sha = commit_worktree_changes(tempdir.path(), &subject)
            .await
            .expect("commit succeeds")
            .expect("dirty worktree creates commit");
        let log_subject = git_output(tempdir.path(), &["log", "-1", "--pretty=%s"]).await;

        assert_ne!(sha, before);
        assert_eq!(log_subject, "agent: Implement the thing");
    }

    #[tokio::test]
    async fn clean_worktree_returns_none() {
        let tempdir = tempfile::tempdir().expect("tempdir creates");
        init_repo(tempdir.path()).await;

        let sha = commit_worktree_changes(tempdir.path(), "agent: noop")
            .await
            .expect("clean check succeeds");

        assert_eq!(sha, None);
    }

    #[test]
    fn build_commit_subject_uses_first_non_empty_line_and_truncates() {
        let long = format!("{}\nsecond line", "a".repeat(80));

        let subject = build_commit_subject(Some(&long), "Fallback");

        assert_eq!(subject, format!("agent: {}", "a".repeat(72)));
    }

    #[test]
    fn build_commit_subject_falls_back_for_empty_description() {
        let subject = build_commit_subject(None, "Fallback title");

        assert_eq!(subject, "agent: Fallback title");
    }

    #[test]
    fn build_commit_subject_falls_back_for_whitespace_description() {
        let subject = build_commit_subject(Some(" \n\t\n"), "Fallback title");

        assert_eq!(subject, "agent: Fallback title");
    }

    async fn init_repo(path: &Path) {
        git_output(path, &["init", "-b", "main"]).await;
        fs::write(path.join("README.md"), "initial\n").expect("readme writes");
        git_output(path, &["add", "README.md"]).await;
        git_output(
            path,
            &[
                "-c",
                "user.email=agent@forge.local",
                "-c",
                "user.name=Forge Agent",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "initial",
            ],
        )
        .await;
    }

    async fn git_output(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .await
            .expect("git command runs");
        assert!(
            output.status.success(),
            "git -C {} {} failed\nstdout: {}\nstderr: {}",
            path.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
}
