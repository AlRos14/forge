use std::{
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use api_types::ProjectPlanDocument;
use db::RepoRepo;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

use crate::{
    error::McpToolError,
    params::parse_params,
    protocol::McpContext,
    state::AppState,
    tools::handlers::{require_project_access, require_project_write_access},
};

const MAX_TREE_BYTES: usize = 4 * 1024 * 1024;
const MAX_TREE_ENTRIES: usize = 10_000;
const MAX_LIST_LIMIT: usize = 200;
const MAX_DOCUMENT_BYTES: usize = 128 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectParams {
    project_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposePlanParams {
    project_id: String,
    expected_version: i64,
    document: ProjectPlanDocument,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyPlanParams {
    project_id: String,
    proposal_id: String,
    expected_version: i64,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListRepoTreeParams {
    project_id: String,
    repo_id: String,
    revision: String,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRepoFileParams {
    project_id: String,
    repo_id: String,
    revision: String,
    path: String,
}

#[derive(Debug)]
struct TreeEntry {
    path: String,
    object_id: String,
    size: u64,
}

pub(super) async fn get_project_plan(
    state: &AppState,
    arguments: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: ProjectParams = parse_params(arguments)?;
    let user_id = authenticated_user(context)?;
    require_project_access(state, &params.project_id, user_id).await?;
    let graph =
        services::project_plan::ProjectPlanService::new(state.db.clone(), state.event_bus.clone())
            .graph(&params.project_id)
            .await?;
    serde_json::to_value(graph)
        .map_err(|_| McpToolError::new(-32603, "Project plan could not be encoded"))
}

pub(super) async fn propose_project_plan(
    state: &AppState,
    arguments: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: ProposePlanParams = parse_params(arguments)?;
    let user_id = authenticated_user(context)?;
    require_project_write_access(state, &params.project_id, user_id).await?;
    let document = service_document(params.document)?;
    let service =
        services::project_plan::ProjectPlanService::new(state.db.clone(), state.event_bus.clone());
    let proposal = if let Some(run_id) = context.copilot_run_id.as_deref() {
        service
            .propose_from_copilot(
                &params.project_id,
                user_id,
                run_id,
                params.expected_version,
                document.clone(),
                &params.idempotency_key,
            )
            .await?
    } else {
        service
            .propose(
                &params.project_id,
                user_id,
                params.expected_version,
                document,
                &params.idempotency_key,
            )
            .await?
    };
    serde_json::to_value(proposal)
        .map_err(|_| McpToolError::new(-32603, "Project plan proposal could not be encoded"))
}

pub(super) async fn apply_project_plan(
    state: &AppState,
    arguments: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: ApplyPlanParams = parse_params(arguments)?;
    let user_id = authenticated_user(context)?;
    require_project_write_access(state, &params.project_id, user_id).await?;
    let service =
        services::project_plan::ProjectPlanService::new(state.db.clone(), state.event_bus.clone());
    let graph = if let Some(run_id) = context.copilot_run_id.as_deref() {
        service
            .apply_from_copilot(
                &params.project_id,
                user_id,
                run_id,
                &params.proposal_id,
                params.expected_version,
                &params.idempotency_key,
            )
            .await?
    } else {
        service
            .apply(
                &params.project_id,
                user_id,
                &params.proposal_id,
                params.expected_version,
                &params.idempotency_key,
            )
            .await?
    };
    serde_json::to_value(graph)
        .map_err(|_| McpToolError::new(-32603, "Project plan graph could not be encoded"))
}

pub(super) async fn list_project_repo_tree(
    state: &AppState,
    arguments: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: ListRepoTreeParams = parse_params(arguments)?;
    let user_id = authenticated_user(context)?;
    require_project_access(state, &params.project_id, user_id).await?;
    let (repo_path, revision) =
        repo_snapshot(state, &params.project_id, &params.repo_id, &params.revision).await?;
    let entries = read_commit_tree(&repo_path, &revision).await?;
    let limit = params.limit.unwrap_or(100).clamp(1, MAX_LIST_LIMIT);
    let items = entries
        .into_iter()
        .filter(|entry| !sensitive_path(&entry.path))
        .take(limit)
        .map(|entry| json!({"path":entry.path,"reference":format!("repo:{}:{}",params.repo_id,entry.path),"size_bytes":entry.size,"revision":revision}))
        .collect::<Vec<_>>();
    Ok(
        json!({"project_id":params.project_id,"repo_id":params.repo_id,"revision":revision,"items":items}),
    )
}

pub(super) async fn read_project_repo_file(
    state: &AppState,
    arguments: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: ReadRepoFileParams = parse_params(arguments)?;
    let user_id = authenticated_user(context)?;
    require_project_access(state, &params.project_id, user_id).await?;
    if !valid_repo_path(&params.path) || sensitive_path(&params.path) {
        return Err(invalid_source());
    }
    let (repo_path, revision) =
        repo_snapshot(state, &params.project_id, &params.repo_id, &params.revision).await?;
    let entries = read_commit_tree(&repo_path, &revision).await?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.path == params.path)
        .ok_or_else(invalid_source)?;
    if entry.size > MAX_DOCUMENT_BYTES as u64 {
        return Err(McpToolError::new(
            -32602,
            "Repository document exceeds the read limit",
        ));
    }
    let bytes = run_git_bounded(
        &repo_path,
        &["cat-file", "blob", &entry.object_id],
        MAX_DOCUMENT_BYTES,
    )
    .await?;
    if bytes.len() as u64 != entry.size {
        return Err(invalid_source());
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| McpToolError::new(-32602, "Repository document is not UTF-8 text"))?;
    if looks_like_credential(&content) {
        return Err(McpToolError::new(
            -32603,
            "Sensitive-looking repository content is unavailable",
        ));
    }
    Ok(json!({
        "project_id":params.project_id,
        "repo_id":params.repo_id,
        "revision":revision,
        "path":entry.path,
        "reference":format!("repo:{}:{}",params.repo_id,entry.path),
        "blob_id":entry.object_id,
        "content":content,
    }))
}

async fn repo_snapshot(
    state: &AppState,
    project_id: &str,
    repo_id: &str,
    revision: &str,
) -> Result<(PathBuf, String), McpToolError> {
    if !valid_object_id(revision) {
        return Err(McpToolError::new(
            -32602,
            "revision must be a full immutable Git object ID",
        ));
    }
    let repo = RepoRepo::get_by_id(&*state.db, repo_id)
        .await?
        .filter(|repo| repo.project_id == project_id)
        .ok_or_else(|| McpToolError::not_found("project repository", repo_id.to_owned()))?;
    let path = repo
        .local_path
        .ok_or_else(|| McpToolError::new(-32602, "Repository source is unavailable locally"))?;
    let repo_path = PathBuf::from(path);
    if !repository_path_is_safe(&repo_path) {
        return Err(McpToolError::new(
            -32602,
            "Repository source failed its path guard",
        ));
    }
    let metadata = std::fs::symlink_metadata(&repo_path)
        .map_err(|_| McpToolError::new(-32602, "Repository source is unavailable locally"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(McpToolError::new(
            -32602,
            "Repository source failed its path guard",
        ));
    }
    let repo_path = repo_path
        .canonicalize()
        .map_err(|_| McpToolError::new(-32602, "Repository source is unavailable locally"))?;
    require_repository_object_store(&repo_path)?;
    let top_level = run_git_bounded(&repo_path, &["rev-parse", "--show-toplevel"], 4096).await?;
    let top_level = String::from_utf8(top_level)
        .map_err(|_| McpToolError::new(-32602, "Repository source is unavailable locally"))?;
    let top_level = PathBuf::from(top_level.trim_end_matches(|ch| ch == '\r' || ch == '\n'));
    if top_level != repo_path {
        return Err(McpToolError::new(
            -32602,
            "Repository source failed its root guard",
        ));
    }
    // The caller supplies a full object ID; this check makes the command use
    // only an existing commit object and never a ref, pathspec, or revision expression.
    run_git_bounded(
        &repo_path,
        &["cat-file", "-e", &format!("{revision}^{{commit}}")],
        0,
    )
    .await?;
    Ok((repo_path, revision.to_ascii_lowercase()))
}

async fn read_commit_tree(
    repo_path: &Path,
    revision: &str,
) -> Result<Vec<TreeEntry>, McpToolError> {
    let bytes = run_git_bounded(
        repo_path,
        &["ls-tree", "-r", "-z", "-l", "--full-tree", revision],
        MAX_TREE_BYTES,
    )
    .await?;
    let mut entries = Vec::new();
    for record in bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let record = std::str::from_utf8(record).map_err(|_| invalid_source())?;
        let (header, path) = record.split_once('\t').ok_or_else(invalid_source)?;
        let fields = header.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 4 || fields[1] != "blob" || !matches!(fields[0], "100644" | "100755") {
            // Git symlinks and gitlinks/submodules are intentionally omitted.
            continue;
        }
        if !valid_object_id(fields[2]) || !valid_repo_path(path) || fields[3] == "-" {
            continue;
        }
        let size = fields[3].parse::<u64>().map_err(|_| invalid_source())?;
        entries.push(TreeEntry {
            path: path.to_owned(),
            object_id: fields[2].to_owned(),
            size,
        });
        if entries.len() > MAX_TREE_ENTRIES {
            return Err(McpToolError::new(
                -32602,
                "Repository tree exceeds the listing limit",
            ));
        }
    }
    Ok(entries)
}

async fn run_git_bounded(
    repo_path: &Path,
    args: &[&str],
    max_bytes: usize,
) -> Result<Vec<u8>, McpToolError> {
    let mut child = Command::new("git")
        .arg("--no-pager")
        .args(args)
        .current_dir(repo_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| invalid_source())?;
    let mut stdout = child.stdout.take().ok_or_else(invalid_source)?;
    let mut bytes = Vec::new();
    let read = timeout(
        GIT_TIMEOUT,
        stdout
            .take((max_bytes.saturating_add(1)) as u64)
            .read_to_end(&mut bytes),
    )
    .await
    .map_err(|_| McpToolError::new(-32602, "Repository object read exceeded its time limit"))?
    .map_err(|_| invalid_source())?;
    if read > max_bytes {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(McpToolError::new(
            -32602,
            "Repository object exceeds the read limit",
        ));
    }
    let status = timeout(GIT_TIMEOUT, child.wait())
        .await
        .map_err(|_| McpToolError::new(-32602, "Repository object read exceeded its time limit"))?
        .map_err(|_| invalid_source())?;
    if !status.success() {
        return Err(invalid_source());
    }
    Ok(bytes)
}

fn authenticated_user(context: &McpContext) -> Result<&str, McpToolError> {
    context
        .user_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| McpToolError::new(-32003, "authenticated server identity is required"))
}

fn service_document(
    document: ProjectPlanDocument,
) -> Result<services::project_plan::ProjectPlanDocument, McpToolError> {
    let value = serde_json::to_value(document)
        .map_err(|_| McpToolError::new(-32602, "Project plan document is invalid"))?;
    serde_json::from_value(value)
        .map_err(|_| McpToolError::new(-32602, "Project plan document is invalid"))
}

fn valid_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_repo_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && !Path::new(path).is_absolute()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn repository_path_is_safe(path: &Path) -> bool {
    if !path.is_absolute()
        || !path.components().all(|component| {
            matches!(
                component,
                Component::RootDir | Component::Prefix(_) | Component::Normal(_)
            )
        })
    {
        return false;
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if !metadata.file_type().is_symlink() => {}
            _ => return false,
        }
    }
    true
}

fn sensitive_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let parts = lower.split('/').collect::<Vec<_>>();
    parts.iter().any(|part| {
        matches!(
            *part,
            ".aws"
                | ".ssh"
                | "credentials"
                | "credential"
                | "secrets"
                | "secret"
                | "private"
                | "certs"
                | "certificates"
                | "keys"
        )
    }) || parts.iter().any(|part| {
        *part == ".env"
            || part.starts_with(".env.")
            || matches!(
                *part,
                ".npmrc" | ".pypirc" | ".netrc" | "id_rsa" | "id_ed25519"
            )
    }) || Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "pem" | "key" | "p12" | "pfx"
            )
        })
}

fn require_repository_object_store(repo_path: &Path) -> Result<(), McpToolError> {
    let git_dir = repo_path.join(".git");
    let metadata = std::fs::symlink_metadata(&git_dir)
        .map_err(|_| McpToolError::new(-32602, "Repository source is not a Git working tree"))?;
    // A .git file can redirect Git to a different Project's object database.
    // Only read ordinary repositories whose object store belongs to this root.
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(McpToolError::new(
            -32602,
            "Repository source failed its Git identity guard",
        ));
    }
    let objects = git_dir.join("objects");
    let objects_metadata = std::fs::symlink_metadata(&objects).map_err(|_| {
        McpToolError::new(
            -32602,
            "Repository source failed its Git object-store guard",
        )
    })?;
    if objects_metadata.file_type().is_symlink() || !objects_metadata.is_dir() {
        return Err(McpToolError::new(
            -32602,
            "Repository source failed its Git object-store guard",
        ));
    }
    for child in [objects.join("info"), objects.join("pack")] {
        if let Ok(metadata) = std::fs::symlink_metadata(&child) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(McpToolError::new(
                    -32602,
                    "Repository source failed its Git object-store guard",
                ));
            }
        }
    }
    if std::fs::symlink_metadata(git_dir.join("commondir")).is_ok()
        || std::fs::symlink_metadata(git_dir.join("gitdir")).is_ok()
    {
        return Err(McpToolError::new(
            -32602,
            "Repository source cannot use an indirect Git directory",
        ));
    }
    if std::fs::symlink_metadata(objects.join("info/alternates")).is_ok() {
        return Err(McpToolError::new(
            -32602,
            "Repository source cannot use an external Git object store",
        ));
    }
    Ok(())
}

fn looks_like_credential(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    if lower.contains("-----begin ") && lower.contains("private key-----") {
        return true;
    }
    for (marker, minimum_token_len) in [
        ("github_pat_", 20),
        ("ghp_", 20),
        ("gho_", 20),
        ("ghu_", 20),
        ("ghs_", 20),
        ("ghr_", 20),
        ("glpat-", 20),
        ("xoxb-", 20),
        ("xoxp-", 20),
        ("xoxa-", 20),
        ("xoxr-", 20),
        // OpenAI API keys, including project and service-account forms.
        ("sk-proj-", 32),
        ("sk-svcacct-", 32),
        ("sk-", 32),
    ] {
        if token_after_marker_is_secret(&lower, marker, minimum_token_len) {
            return true;
        }
    }
    if aws_access_key_is_present(&lower) {
        return true;
    }
    content.lines().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        if !name.trim().eq_ignore_ascii_case("authorization") {
            return false;
        }
        let mut fields = value.split_ascii_whitespace();
        let Some(scheme) = fields.next() else {
            return false;
        };
        let Some(token) = fields.next() else {
            return false;
        };
        if !matches!(scheme.to_ascii_lowercase().as_str(), "bearer" | "basic") {
            return false;
        }
        is_secret_token(token, 20)
    })
}

fn token_after_marker_is_secret(content: &str, marker: &str, minimum_token_len: usize) -> bool {
    let Some(index) = content.find(marker) else {
        return false;
    };
    is_secret_token(&content[index + marker.len()..], minimum_token_len)
}

fn is_secret_token(value: &str, minimum_token_len: usize) -> bool {
    let lower = value.to_ascii_lowercase();
    let token = lower
        .trim_start_matches(|ch: char| matches!(ch, ' ' | '\t' | '"' | '\'' | '='))
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')))
        .next()
        .unwrap_or_default();
    token.len() >= minimum_token_len
        && ![
            "example",
            "sample",
            "placeholder",
            "redacted",
            "yourtoken",
            "fake",
        ]
        .iter()
        .any(|placeholder| token.contains(placeholder))
        && !token
            .bytes()
            .all(|byte| byte == token.as_bytes().first().copied().unwrap_or_default())
}

fn aws_access_key_is_present(content: &str) -> bool {
    content.match_indices("akia").any(|(index, _)| {
        let token = content[index..]
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .next()
            .unwrap_or_default();
        token.len() == 20
            && token.bytes().all(|byte| byte.is_ascii_alphanumeric())
            && !["example", "sample", "placeholder", "redacted"]
                .iter()
                .any(|placeholder| token.contains(placeholder))
    })
}

fn invalid_source() -> McpToolError {
    McpToolError::new(
        -32602,
        "Repository document is unavailable at the requested revision",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command as StdCommand};

    struct GitFixture(PathBuf);

    impl GitFixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("forge-project-source-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).expect("temporary repository directory creates");
            git(&path, &["init", "-q"]);
            git(&path, &["config", "user.name", "Forge test"]);
            git(&path, &["config", "user.email", "forge-test@example.com"]);
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn commit(&self) -> String {
            git(&self.0, &["add", "--all"]);
            git(&self.0, &["commit", "-qm", "fixture"]);
            git(&self.0, &["rev-parse", "HEAD"])
        }
    }

    impl Drop for GitFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git(path: &Path, args: &[&str]) -> String {
        let output = StdCommand::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .expect("git starts");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git output is UTF-8")
            .trim()
            .to_owned()
    }

    #[tokio::test]
    async fn reads_pinned_git_blobs_of_arbitrary_text_formats_not_worktree_files() {
        let fixture = GitFixture::new();
        fs::create_dir_all(fixture.path().join("docs")).expect("docs directory creates");
        fs::write(
            fixture.path().join("docs/plan.yaml"),
            "stages:\n  - review\n",
        )
        .expect("YAML source writes");
        fs::write(
            fixture.path().join("docs/graph.dot"),
            "digraph { review -> approve }\n",
        )
        .expect("DOT source writes");
        fs::write(
            fixture.path().join("docs/notes"),
            "Plain source without an extension\n",
        )
        .expect("extensionless source writes");
        let revision = fixture.commit();

        fs::write(
            fixture.path().join("docs/plan.yaml"),
            "uncommitted replacement\n",
        )
        .expect("worktree changes after commit");
        let entries = read_commit_tree(fixture.path(), &revision)
            .await
            .expect("exact commit tree reads");
        for expected in ["docs/plan.yaml", "docs/graph.dot", "docs/notes"] {
            assert!(entries.iter().any(|entry| entry.path == expected));
        }
        let entry = entries
            .iter()
            .find(|entry| entry.path == "docs/plan.yaml")
            .expect("YAML blob exists at pinned commit");
        let content = run_git_bounded(
            fixture.path(),
            &["cat-file", "blob", &entry.object_id],
            MAX_DOCUMENT_BYTES,
        )
        .await
        .expect("pinned object reads");
        assert_eq!(String::from_utf8(content).unwrap(), "stages:\n  - review\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn source_tree_skips_symlinks_and_rejects_traversal_and_oversized_blobs() {
        use std::os::unix::fs::symlink;

        let fixture = GitFixture::new();
        fs::create_dir_all(fixture.path().join("docs")).expect("docs directory creates");
        fs::write(fixture.path().join("docs/allowed.md"), "safe document\n")
            .expect("regular document writes");
        fs::write(
            fixture.path().join("docs/large.json"),
            vec![b'x'; MAX_DOCUMENT_BYTES + 1],
        )
        .expect("large document writes");
        let outside = fixture.path().join("outside.md");
        fs::write(&outside, "must not follow links\n").expect("outside target writes");
        symlink(&outside, fixture.path().join("docs/linked.md")).expect("tree symlink creates");
        let revision = fixture.commit();

        let entries = read_commit_tree(fixture.path(), &revision)
            .await
            .expect("tree reads");
        assert!(entries.iter().any(|entry| entry.path == "docs/allowed.md"));
        assert!(entries.iter().any(|entry| entry.path == "docs/large.json"));
        assert!(!entries.iter().any(|entry| entry.path == "docs/linked.md"));
        assert!(!valid_repo_path("../outside.md"));
        assert!(!valid_repo_path("/outside.md"));
        assert!(sensitive_path(".env.local"));
        assert!(sensitive_path("docs/credentials/token.txt"));

        let large = entries
            .iter()
            .find(|entry| entry.path == "docs/large.json")
            .expect("large text blob exists");
        assert!(large.size > MAX_DOCUMENT_BYTES as u64);
        assert!(run_git_bounded(
            fixture.path(),
            &["cat-file", "blob", &large.object_id],
            MAX_DOCUMENT_BYTES,
        )
        .await
        .is_err());
    }

    #[test]
    fn repository_identity_guard_rejects_gitdir_files_and_external_object_stores() {
        let fixture = GitFixture::new();
        let git_dir = fixture.path().join(".git");
        fs::remove_dir_all(&git_dir).expect("fixture git directory removes");
        fs::write(&git_dir, "gitdir: /other/project/.git/worktrees/repo\n")
            .expect("indirect git directory marker writes");
        assert!(require_repository_object_store(fixture.path()).is_err());

        fs::remove_file(&git_dir).expect("gitdir marker removes");
        git(fixture.path(), &["init", "-q"]);
        fs::create_dir_all(git_dir.join("objects/info")).expect("object info directory creates");
        fs::write(
            git_dir.join("objects/info/alternates"),
            "/other/project/.git/objects\n",
        )
        .expect("external object database marker writes");
        assert!(require_repository_object_store(fixture.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn repository_path_guard_rejects_symlinked_roots_and_parent_traversal() {
        use std::os::unix::fs::symlink;

        let fixture = GitFixture::new();
        let alias = fixture.path().with_extension("alias");
        symlink(fixture.path(), &alias).expect("repository path alias creates");
        assert!(!repository_path_is_safe(&alias));
        assert!(!repository_path_is_safe(&fixture.path().join("..")));
        fs::remove_file(alias).expect("repository path alias removes");
    }

    #[test]
    fn credential_filter_accepts_documentation_and_rejects_credential_shapes() {
        assert!(!looks_like_credential(
            "Authorization: Bearer <token>\nExample AWS key: AKIAexample\n"
        ));
        assert!(!looks_like_credential(
            "Example AWS key: AKIAIOSFODNN7EXAMPLE\n"
        ));
        assert!(!looks_like_credential(
            "ghp_XXXXXXXXXXXXXXXXXXXXXXXXXXXX is a placeholder\n"
        ));
        assert!(!looks_like_credential(
            "Example API key: sk-proj-example-placeholder\n"
        ));
        assert!(looks_like_credential(
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789\n"
        ));
        assert!(looks_like_credential(
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789\n"
        ));
        assert!(looks_like_credential(
            "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQR\n"
        ));
        assert!(looks_like_credential(
            "gho_abcdefghijklmnopqrstuvwxyz0123456789\n"
        ));
        assert!(looks_like_credential("-----BEGIN PRIVATE KEY-----\n"));
    }
}
