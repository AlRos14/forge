use super::*;
use crate::coordinator_outputs::external_feedback::build_pr_feedback_summary;
use crate::credential_service::CredentialService;
use crate::pr_github_client::{GitHubPrProvider, PrProviderFactory};
use crate::pr_rework::tests::{superseded, world, World};
use crate::pr_service::{PrProvider, PrProviderError};
use async_trait::async_trait;
use base64::Engine as _;
use db::{PrReworkSupersession, RepoRepo, TaskRepo};
use std::process::Command as StdCommand;
use std::sync::Mutex;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: &str = "ghp_test_feedback_token";

// ---------------------------------------------------------------------
// Mock GitHub server and git helpers
// ---------------------------------------------------------------------

struct MockState {
    head: String,
    head_branch: String,
    pr_state: String,
    merged: bool,
    comments: Vec<(u64, String)>,
    requests: Vec<String>,
    post_status: u16,
}

struct Mock {
    base_url: String,
    state: Arc<Mutex<MockState>>,
}

impl Mock {
    fn requests(&self) -> Vec<String> {
        self.state.lock().expect("state").requests.clone()
    }

    fn set_head(&self, head: &str) {
        self.state.lock().expect("state").head = head.to_owned();
    }

    fn posts(&self) -> usize {
        self.requests()
            .iter()
            .filter(|line| line.starts_with("POST "))
            .count()
    }

    fn pull_creations(&self) -> usize {
        self.requests()
            .iter()
            .filter(|line| {
                line.starts_with("POST ")
                    && line.split_whitespace().nth(1).is_some_and(|target| {
                        target
                            .split('?')
                            .next()
                            .unwrap_or_default()
                            .ends_with("/pulls")
                    })
            })
            .count()
    }
}

fn pull_json(number: u64, state: &str, merged: bool, sha: &str, head_branch: &str) -> String {
    format!(
        r#"{{"number":{number},"html_url":"https://github.com/acme/widgets/pull/{number}","state":"{state}","merged":{merged},"draft":false,"head":{{"ref":"{head_branch}","sha":"{sha}"}},"base":{{"ref":"main","sha":"b"}},"merge_commit_sha":"mc1","mergeable":true,"mergeable_state":"clean","updated_at":"2026-01-01T00:00:00Z"}}"#
    )
}

fn comment_json(id: u64, body: &str) -> serde_json::Value {
    json!({
        "id": id,
        "body": body,
        "user": {"login": "forge"},
        "author_association": "OWNER",
        "html_url": null,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
    })
}

fn respond(state: &Mutex<MockState>, request_line: &str, body: &str) -> (u16, String) {
    let mut state = state.lock().expect("state");
    state.requests.push(request_line.to_owned());
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let path = target.split('?').next().unwrap_or_default();
    if method == "GET" && path.ends_with("/pulls") {
        return (
            200,
            format!(
                "[{}]",
                pull_json(
                    77,
                    &state.pr_state,
                    state.merged,
                    &state.head,
                    &state.head_branch,
                )
            ),
        );
    }
    if method == "GET" && path.ends_with("/pulls/77") {
        return (
            200,
            pull_json(
                77,
                &state.pr_state,
                state.merged,
                &state.head,
                &state.head_branch,
            ),
        );
    }
    if method == "GET" && path.ends_with("/issues/77/comments") {
        let items: Vec<serde_json::Value> = state
            .comments
            .iter()
            .map(|(id, body)| comment_json(*id, body))
            .collect();
        return (200, serde_json::Value::Array(items).to_string());
    }
    if method == "POST" && path.ends_with("/issues/77/comments") {
        if state.post_status != 201 {
            return (state.post_status, "{}".to_owned());
        }
        let posted: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let text = posted["body"].as_str().unwrap_or_default().to_owned();
        let id = state.comments.len() as u64 + 1;
        state.comments.push((id, text.clone()));
        return (201, comment_json(id, &text).to_string());
    }
    (404, "{}".to_owned())
}

async fn mock(head: &str, head_branch: &str) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base_url = format!("http://{}", listener.local_addr().expect("addr"));
    let state = Arc::new(Mutex::new(MockState {
        head: head.to_owned(),
        head_branch: head_branch.to_owned(),
        pr_state: "open".to_owned(),
        merged: false,
        comments: Vec::new(),
        requests: Vec::new(),
        post_status: 201,
    }));
    let shared = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut data = Vec::new();
            let mut buffer = [0_u8; 4096];
            let (head_end, content_length) = loop {
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                if read == 0 {
                    break (data.len(), 0);
                }
                data.extend_from_slice(&buffer[..read]);
                if let Some(position) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&data[..position]).to_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (position + 4, length);
                }
            };
            while data.len() < head_end + content_length {
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                if read == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..read]);
            }
            let head = String::from_utf8_lossy(&data[..head_end.min(data.len())]).to_string();
            let request_line = head.lines().next().unwrap_or_default().to_owned();
            let body = String::from_utf8_lossy(&data[head_end.min(data.len())..]).to_string();
            let (status, payload) = respond(&shared, &request_line, &body);
            let text = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = socket.write_all(text.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    Mock { base_url, state }
}

struct MockFactory {
    base_url: String,
}

#[async_trait]
impl PrProviderFactory for MockFactory {
    async fn build(
        &self,
        _provider_type: &str,
        _base_url: Option<&str>,
        token: Secret,
    ) -> std::result::Result<Arc<dyn PrProvider>, PrProviderError> {
        let client = GitHubClient::new_unvalidated(&self.base_url, token)
            .map_err(|error| error.into_pr_error(false))?;
        Ok(Arc::new(GitHubPrProvider::new(client)))
    }

    async fn build_client(
        &self,
        _provider_type: &str,
        _base_url: Option<&str>,
        token: Secret,
    ) -> std::result::Result<GitHubClient, PrProviderError> {
        GitHubClient::new_unvalidated(&self.base_url, token)
            .map_err(|error| error.into_pr_error(false))
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = StdCommand::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct Repos {
    _temp: TempDir,
    bare: PathBuf,
    work: PathBuf,
    old_sha: String,
    new_sha: String,
    branch: String,
}

/// A bare remote holding `branch` at `old_sha` and a clone whose local `HEAD`
/// is a descendant `new_sha`.
fn repos() -> Repos {
    repos_for_branch("forge/task")
}

fn repos_for_branch(branch: &str) -> Repos {
    let temp = TempDir::new().expect("temp");
    let bare = temp.path().join("remote.git");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&bare).expect("bare dir");
    std::fs::create_dir_all(&work).expect("work dir");
    git(&bare, &["init", "--bare", "-q"]);
    git(&work, &["init", "-q"]);
    git(&work, &["config", "user.email", "forge@example.invalid"]);
    git(&work, &["config", "user.name", "Forge Test"]);
    std::fs::write(work.join("a.txt"), "one\n").expect("write");
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "one"]);
    let old_sha = git(&work, &["rev-parse", "HEAD"]);
    let refspec = format!("HEAD:refs/heads/{branch}");
    git(
        &work,
        &["push", "-q", bare.to_str().expect("utf8"), &refspec],
    );
    std::fs::write(work.join("a.txt"), "one\ntwo\n").expect("write");
    git(&work, &["commit", "-q", "-am", "two"]);
    let new_sha = git(&work, &["rev-parse", "HEAD"]);
    Repos {
        _temp: temp,
        bare,
        work,
        old_sha,
        new_sha,
        branch: branch.to_owned(),
    }
}

impl Repos {
    fn remote_head(&self) -> String {
        git(
            &self.bare,
            &["rev-parse", &format!("refs/heads/{}", self.branch)],
        )
    }

    fn bare_url(&self) -> String {
        self.bare.to_str().expect("utf8").to_owned()
    }
}

// ---------------------------------------------------------------------
// Rework world with a re-admission (raw SQL, the only way to hold the
// pre-conditions without running the whole Gate pipeline again)
// ---------------------------------------------------------------------

async fn drop_triggers(db: &SqliteDb, tables: &[&str]) {
    for table in tables {
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?",
        )
        .bind(*table)
        .fetch_all(db.pool())
        .await
        .expect("trigger names");
        for name in names {
            sqlx::query(&format!("DROP TRIGGER IF EXISTS \"{name}\""))
                .execute(db.pool())
                .await
                .expect("drop trigger");
        }
    }
}

async fn exec(db: &SqliteDb, sql: &str, args: &[&str]) {
    let mut query = sqlx::query(sql);
    for arg in args {
        query = query.bind(*arg);
    }
    query.execute(db.pool()).await.expect(sql);
}

struct Rig {
    w: World,
    mock: Mock,
    repos: Repos,
    service: PrExternalActionService,
    owner: String,
    merge2: String,
    supersession: PrReworkSupersession,
}

fn policy_json(push: &str, summary: &str) -> String {
    json!({
        "schema_version": 1,
        "ingest": true,
        "trusted_author_associations": ["OWNER"],
        "trusted_logins": [],
        "max_items_per_poll": 50,
        "push_corrections": push,
        "post_summary": summary,
    })
    .to_string()
}

async fn rig(push: &str, summary: &str) -> Rig {
    let w = world(true).await;
    let supersession = superseded(&w).await;
    let branch = format!("task/{}", &w.task_id[..8]);
    let repos = repos_for_branch(&branch);
    let mock = mock(&repos.old_sha, &branch).await;
    let db = &w.db;
    drop_triggers(
        db,
        &[
            "remote_pr_admission",
            "pr_rework_supersession",
            "pr_metadata",
            "task_lifecycle",
            "task_integration_operation",
            "artifact",
        ],
    )
    .await;
    let policy = policy_json(push, summary);
    let digest = db::pr_feedback_policy_digest(&policy);
    // The fixture admission used placeholder identities; give the paused PR
    // real ones (a parseable repository, real commits, a numeric PR id).
    exec(
        db,
        "UPDATE remote_pr_admission SET admitted_source_sha = ?,
            remote_repo_identity = 'https://github.com/acme/widgets.git',
            feedback_policy_json = ?, feedback_policy_digest = ?
         WHERE task_merge_operation_id = ?",
        &[&repos.old_sha, &policy, &digest, &w.merge_id],
    )
    .await;
    exec(
        db,
        "UPDATE pr_rework_supersession SET superseded_head_sha = ?, provider_pr_id = '77'
         WHERE id = ?",
        &[&repos.old_sha, &supersession.id],
    )
    .await;
    let (merge2, publish2) = (new_uuid_v4(), new_uuid_v4());
    let gate: String = sqlx::query_scalar(
        "SELECT gate_evaluation_id FROM task_integration_operation WHERE id = ?",
    )
    .bind(&w.merge_id)
    .fetch_one(db.pool())
    .await
    .expect("gate");
    let now = now_rfc3339();
    exec(
        db,
        "INSERT INTO task_integration_operation (id, task_id, kind, owner_id, status, version,
            created_at, updated_at, gate_evaluation_id, remote_waiting)
         VALUES (?, ?, 'task_merge', 'owner', 'running', 1, ?, ?, ?, 1)",
        &[&merge2, &w.task_id, &now, &now, &gate],
    )
    .await;
    exec(
        db,
        "INSERT INTO task_integration_operation (id, task_id, kind, owner_id, status, version,
            created_at, updated_at, gate_evaluation_id, remote_waiting, parent_operation_id)
         VALUES (?, ?, 'publish_pr', 'owner', 'running', 1, ?, ?, ?, 0, ?)",
        &[&publish2, &w.task_id, &now, &now, &gate, &merge2],
    )
    .await;
    exec(
        db,
        "UPDATE pr_metadata SET provider_pr_id = '77', task_merge_operation_id = ?,
            publish_operation_id = ? WHERE task_id = ?",
        &[&merge2, &publish2, &w.task_id],
    )
    .await;
    exec(
        db,
        "UPDATE task_lifecycle SET state = 'merging', reason_kind = 'task_merge',
            reason_ref = ?, version = version + 1 WHERE task_id = ?",
        &[&merge2, &w.task_id],
    )
    .await;
    exec(
        db,
        "INSERT INTO remote_pr_admission (
            task_merge_operation_id, publish_operation_id, metadata_id, task_id,
            provider_config_id, provider_type, provider_config_revision,
            provider_config_digest, provider_base_url, token_secret_ref,
            remote_repo_identity, source_branch, target_branch, admitted_source_sha,
            state, created_at, updated_at, credential_handle_id, credential_owner_user_id,
            feedback_policy_json, feedback_policy_digest, readmits_supersession_id)
         SELECT ?, ?, metadata_id, task_id, provider_config_id, provider_type,
            provider_config_revision, provider_config_digest, provider_base_url,
            token_secret_ref, remote_repo_identity, source_branch, target_branch, ?,
            'admitted', ?, ?, credential_handle_id, credential_owner_user_id,
            feedback_policy_json, feedback_policy_digest, ?
         FROM remote_pr_admission WHERE task_merge_operation_id = ?",
        &[
            &merge2,
            &publish2,
            &repos.new_sha,
            &now,
            &now,
            &supersession.id,
            &w.merge_id,
        ],
    )
    .await;
    let owner: String = sqlx::query_scalar(
        "SELECT credential_owner_user_id FROM remote_pr_admission
         WHERE task_merge_operation_id = ?",
    )
    .bind(&merge2)
    .fetch_one(db.pool())
    .await
    .expect("owner");
    let credentials = Arc::new(CredentialService::new(
        Arc::clone(&w.db),
        b"pr-rework-test-key",
    ));
    let pr = PrService::new(Arc::clone(&w.db), Arc::clone(&w.bus))
        .with_credentials(credentials)
        .with_provider_factory(Arc::new(MockFactory {
            base_url: mock.base_url.clone(),
        }));
    let service = PrExternalActionService::new(Arc::clone(&w.db), Arc::clone(&w.bus), pr)
        .with_push_source_for_test(repos.work.clone(), repos.bare_url());
    let supersession = PrReworkRepo::get_supersession(&*w.db, &supersession.id)
        .await
        .expect("supersession")
        .expect("exists");
    Rig {
        w,
        mock,
        repos,
        service,
        owner,
        merge2,
        supersession,
    }
}

impl Rig {
    async fn action(&self, id: &str) -> PrExternalAction {
        PrExternalActionRepo::get_pr_external_action(&*self.w.db, id)
            .await
            .expect("action")
            .expect("exists")
    }

    async fn admission_state(&self) -> String {
        TaskIntegrationOperationRepo::get_remote_pr_admission(&*self.w.db, &self.merge2)
            .await
            .expect("admission")
            .expect("exists")
            .state
    }

    async fn supersession_state(&self) -> PrReworkSupersession {
        PrReworkRepo::get_supersession(&*self.w.db, &self.supersession.id)
            .await
            .expect("supersession")
            .expect("exists")
    }
}

// ---------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------

#[test]
fn pr_external_action_summary_marker_is_verified() {
    let summary = build_pr_feedback_summary(&[], "artifact-1");
    assert_eq!(
        verify_summary("artifact-1", &summary.text).expect("exact summary"),
        summary.digest
    );
    assert!(verify_summary("artifact-2", &summary.text).is_err());
    let tampered = summary.text.replacen("triaged", "TRIAGED", 1);
    assert!(verify_summary("artifact-1", &tampered).is_err());
    assert!(verify_summary("artifact-1", "no marker at all").is_err());
    assert!(verify_summary("artifact-1", &"x".repeat(MAX_SUMMARY_BYTES + 1)).is_err());
}

#[test]
fn pr_external_action_redaction_and_classification_are_conservative() {
    assert_eq!(
        redact("token sekret-value-123 leaked", &["sekret-value-123"]),
        "token [redacted] leaked"
    );
    assert!(redact(&"y".repeat(5000), &[]).chars().count() <= MAX_STORED_ERROR_CHARS);
    assert!(valid_branch("forge/task-1"));
    assert!(!valid_branch("-oops"));
    assert!(!valid_branch("a..b"));
    assert!(!valid_branch("a b"));
    assert!(valid_sha("abcdef1"));
    assert!(!valid_sha("not-a-sha"));
    assert!(definitive_push_failure("remote: Permission to x denied"));
    assert!(definitive_push_failure(
        "! [remote rejected] x (protected branch hook declined)"
    ));
    assert!(!definitive_push_failure("! [rejected] x -> x (stale info)"));
    assert!(!definitive_push_failure(
        "fatal: unable to access: Could not resolve host"
    ));
}

#[tokio::test]
async fn pr_external_action_git_push_is_a_compare_and_swap_fast_forward() {
    let repos = repos();
    // A wrong expectation is rejected by the remote and changes nothing.
    let stale = git_push_cas(
        &repos.work,
        &repos.bare_url(),
        &repos.branch,
        &repos.new_sha,
        &repos.new_sha,
        None,
    )
    .await
    .expect_err("stale expectation");
    assert!(!stale.definitive);
    assert_eq!(repos.remote_head(), repos.old_sha);
    // The exact expectation moves the branch.
    git_push_cas(
        &repos.work,
        &repos.bare_url(),
        &repos.branch,
        &repos.old_sha,
        &repos.new_sha,
        None,
    )
    .await
    .expect("fast-forward push");
    assert_eq!(repos.remote_head(), repos.new_sha);
    // Unsafe inputs never reach git.
    assert!(
        git_push_cas(
            &repos.work,
            "-evil",
            &repos.branch,
            &repos.old_sha,
            &repos.new_sha,
            None
        )
        .await
        .expect_err("option-like remote")
        .definitive
    );
    assert!(
        git_push_cas(
            &repos.work,
            "https://user:pw@example.invalid/x.git",
            &repos.branch,
            &repos.old_sha,
            &repos.new_sha,
            None
        )
        .await
        .expect_err("credentials in URL")
        .definitive
    );
    assert!(
        git_push_cas(
            &repos.work,
            &repos.bare_url(),
            "--force",
            &repos.old_sha,
            &repos.new_sha,
            None
        )
        .await
        .expect_err("bad branch")
        .definitive
    );
}

#[tokio::test]
async fn pr_external_action_git_push_never_leaks_the_token() {
    let repos = repos();
    let secret = Secret::new("ghp_never_leak_this_token_value".to_owned());
    let failure = git_push_cas(
        &repos.work,
        "https://127.0.0.1:9/acme/widgets.git",
        &repos.branch,
        &repos.old_sha,
        &repos.new_sha,
        Some(&secret),
    )
    .await
    .expect_err("nothing listens there");
    let basic = base64::engine::general_purpose::STANDARD
        .encode(format!("x-access-token:{}", secret.expose()));
    assert!(!failure.message.contains(secret.expose()));
    assert!(!failure.message.contains(&basic));
}

// ---------------------------------------------------------------------
// push_update
// ---------------------------------------------------------------------

#[tokio::test]
async fn pr_external_action_automatic_push_update_fast_forwards_and_readmits() {
    let r = rig("automatic", "off").await;
    let action = r
        .service
        .ensure_push_update(&r.merge2)
        .await
        .expect("push_update");
    assert_eq!(action.state, PrExternalActionState::Approved);
    assert_eq!(
        action.authorization_mode,
        PrFeedbackAuthorization::Automatic
    );
    assert_eq!(
        action.expected_remote_head_sha.as_deref(),
        Some(r.repos.old_sha.as_str())
    );
    assert_eq!(
        action.subject_commit_sha.as_deref(),
        Some(r.repos.new_sha.as_str())
    );
    // Idempotent per admission.
    let again = r
        .service
        .ensure_push_update(&r.merge2)
        .await
        .expect("again");
    assert_eq!(again.id, action.id);

    r.service.run_once().await.expect("executor pass");

    assert_eq!(
        r.action(&action.id).await.state,
        PrExternalActionState::Succeeded
    );
    assert_eq!(r.repos.remote_head(), r.repos.new_sha);
    assert_eq!(r.admission_state().await, "open");
    let supersession = r.supersession_state().await;
    assert_eq!(supersession.state, PrReworkState::Readmitted);
    assert_eq!(
        supersession.readmission_merge_operation_id.as_deref(),
        Some(r.merge2.as_str())
    );
    for line in r.mock.requests() {
        assert!(!line.contains(TOKEN), "the token is never in a URL");
    }
    // A replayed pass is a no-op.
    r.service.run_once().await.expect("replay");
    assert_eq!(r.repos.remote_head(), r.repos.new_sha);
}

#[tokio::test]
async fn pr_external_action_publish_readmission_reuses_the_open_pull_request() {
    let r = rig("automatic", "off").await;
    let task = TaskRepo::get_by_id(&*r.w.db, &r.w.task_id, false)
        .await
        .expect("task lookup")
        .expect("task exists");
    let repo_id = task.repo_id.as_deref().expect("Task repo");
    let repo = RepoRepo::get_by_id(&*r.w.db, repo_id)
        .await
        .expect("repo lookup")
        .expect("repo exists");

    let published = r
        .service
        .pr
        .publish_pr(&task, &repo, &r.repos.branch, "main")
        .await
        .expect("same-PR readmission");
    assert_eq!(published.metadata.provider_pr_id.as_deref(), Some("77"));
    assert_eq!(r.admission_state().await, "admitted");
    assert_eq!(r.mock.pull_creations(), 0, "readmission never creates a PR");
    assert!(r
        .mock
        .requests()
        .iter()
        .any(|line| line.starts_with("GET ") && line.contains("/pulls?head=")));

    let action = PrExternalActionRepo::get_pr_external_action_by_idempotency_key(
        &*r.w.db,
        &format!("push_update:{}", r.merge2),
    )
    .await
    .expect("action lookup")
    .expect("push action created");
    assert_eq!(action.state, PrExternalActionState::Approved);
    r.service.run_once().await.expect("executor pass");
    assert_eq!(r.repos.remote_head(), r.repos.new_sha);
    assert_eq!(
        r.supersession_state().await.state,
        PrReworkState::Readmitted
    );
}

#[tokio::test]
async fn pr_external_action_push_requires_policy_or_decision() {
    let r = rig("human_approval", "off").await;
    let action = r
        .service
        .ensure_push_update(&r.merge2)
        .await
        .expect("push_update");
    assert_eq!(action.state, PrExternalActionState::Proposed);
    assert!(
        action.proposal_id.is_some(),
        "a Proposal targets the action"
    );

    // Nothing is pushed while the action only is proposed.
    r.service.run_once().await.expect("executor pass");
    assert_eq!(r.repos.remote_head(), r.repos.old_sha);
    assert_eq!(
        r.action(&action.id).await.state,
        PrExternalActionState::Proposed
    );

    // Only a Project owner/admin decides.
    assert!(r.service.approve(&action.id, "nobody", None).await.is_err());
    let approved = r
        .service
        .approve(&action.id, &r.owner, Some("Looks right".to_owned()))
        .await
        .expect("approve");
    assert_eq!(approved.state, PrExternalActionState::Approved);
    assert!(approved.decision_id.is_some());
    let replay = r
        .service
        .approve(&action.id, &r.owner, None)
        .await
        .expect("idempotent approval");
    assert_eq!(replay.version, approved.version);
    assert!(r.service.reject(&action.id, &r.owner, None).await.is_err());

    r.service.run_once().await.expect("executor pass");
    assert_eq!(
        r.action(&action.id).await.state,
        PrExternalActionState::Succeeded
    );
    assert_eq!(r.repos.remote_head(), r.repos.new_sha);
}

#[tokio::test]
async fn pr_external_action_rejected_push_fails_the_readmission() {
    let r = rig("human_approval", "off").await;
    let action = r
        .service
        .ensure_push_update(&r.merge2)
        .await
        .expect("push_update");
    let rejected = r
        .service
        .reject(&action.id, &r.owner, None)
        .await
        .expect("reject");
    assert_eq!(rejected.state, PrExternalActionState::Rejected);
    assert_eq!(r.admission_state().await, "publication_failed");
    assert_eq!(r.repos.remote_head(), r.repos.old_sha);
    assert_eq!(r.supersession_state().await.state, PrReworkState::Open);
}

#[tokio::test]
async fn pr_external_action_remote_head_change_requires_reconciliation() {
    let r = rig("automatic", "off").await;
    r.mock.set_head("dddddddddddddddddddddddddddddddddddddddd");
    let action = r
        .service
        .ensure_push_update(&r.merge2)
        .await
        .expect("push_update");
    r.service.run_once().await.expect("executor pass");
    assert_eq!(
        r.action(&action.id).await.state,
        PrExternalActionState::ReconciliationRequired
    );
    assert_eq!(r.admission_state().await, "reconciliation_required");
    assert_eq!(r.repos.remote_head(), r.repos.old_sha, "never forced");
}

#[tokio::test]
async fn pr_external_action_not_fast_forward_fails_without_pushing() {
    let r = rig("automatic", "off").await;
    // The admitted head does not descend from the paused head.
    git(&r.repos.work, &["checkout", "-q", "--orphan", "other"]);
    std::fs::write(r.repos.work.join("b.txt"), "x\n").expect("write");
    git(&r.repos.work, &["add", "b.txt"]);
    git(&r.repos.work, &["commit", "-q", "-m", "unrelated"]);
    let unrelated = git(&r.repos.work, &["rev-parse", "HEAD"]);
    exec(
        &r.w.db,
        "UPDATE remote_pr_admission SET admitted_source_sha = ? WHERE task_merge_operation_id = ?",
        &[&unrelated, &r.merge2],
    )
    .await;
    let action = r
        .service
        .ensure_push_update(&r.merge2)
        .await
        .expect("push_update");
    r.service.run_once().await.expect("executor pass");
    let failed = r.action(&action.id).await;
    assert_eq!(failed.state, PrExternalActionState::Failed);
    assert!(failed
        .provider_result_json
        .as_deref()
        .unwrap_or_default()
        .contains(CODE_NOT_FAST_FORWARD));
    assert_eq!(r.repos.remote_head(), r.repos.old_sha);
    assert_eq!(r.admission_state().await, "publication_failed");
}

#[tokio::test]
async fn pr_external_action_running_after_restart_is_recovered_from_the_remote_head() {
    // A crash after `running` but before the push: the head is still the
    // paused one, so the push is performed (once).
    let r = rig("automatic", "off").await;
    let action = r
        .service
        .ensure_push_update(&r.merge2)
        .await
        .expect("push_update");
    let running = r
        .service
        .transition(&action, PrExternalActionState::Running, None, None, None)
        .await
        .expect("running");
    assert_eq!(running.state, PrExternalActionState::Running);
    r.service.run_once().await.expect("recovery pass");
    assert_eq!(
        r.action(&action.id).await.state,
        PrExternalActionState::Succeeded
    );
    assert_eq!(r.repos.remote_head(), r.repos.new_sha);
}

#[tokio::test]
async fn pr_external_action_outcome_unknown_resolves_when_the_push_already_landed() {
    // A crash after the push: the remote already has the admitted head.
    let r = rig("automatic", "off").await;
    let action = r
        .service
        .ensure_push_update(&r.merge2)
        .await
        .expect("push_update");
    git_push_cas(
        &r.repos.work,
        &r.repos.bare_url(),
        &r.repos.branch,
        &r.repos.old_sha,
        &r.repos.new_sha,
        None,
    )
    .await
    .expect("the push landed");
    r.mock.set_head(&r.repos.new_sha);
    let running = r
        .service
        .transition(&action, PrExternalActionState::Running, None, None, None)
        .await
        .expect("running");
    r.service
        .transition(
            &running,
            PrExternalActionState::OutcomeUnknown,
            None,
            None,
            None,
        )
        .await
        .expect("unknown");
    r.service.run_once().await.expect("recovery pass");
    let done = r.action(&action.id).await;
    assert_eq!(done.state, PrExternalActionState::Succeeded);
    assert!(done
        .provider_result_json
        .as_deref()
        .unwrap_or_default()
        .contains("already_at_subject"));
    assert_eq!(r.admission_state().await, "open");
    assert_eq!(
        r.supersession_state().await.state,
        PrReworkState::Readmitted
    );
}

#[tokio::test]
async fn pr_external_action_executor_creates_the_missing_readmission_push() {
    let r = rig("automatic", "off").await;
    assert!(
        PrExternalActionRepo::get_open_pr_external_action(&*r.w.db, &r.w.task_id)
            .await
            .expect("open")
            .is_none()
    );
    let stats = r.service.run_once().await.expect("executor pass");
    assert_eq!(stats.created, 1);
    assert_eq!(r.repos.remote_head(), r.repos.new_sha);
    assert_eq!(r.admission_state().await, "open");
}

// ---------------------------------------------------------------------
// post_comment
// ---------------------------------------------------------------------

async fn summary_artifact(r: &Rig) -> (String, String) {
    let id = new_uuid_v4();
    let summary = build_pr_feedback_summary(&[], &id);
    exec(
        &r.w.db,
        "INSERT INTO artifact (id, task_id, kind, storage_kind, content, metadata_json,
            digest, created_at)
         VALUES (?, ?, 'summary', 'inline', ?, '{}', ?, ?)",
        &[
            &id,
            &r.w.task_id,
            &summary.text,
            &summary.digest,
            &now_rfc3339(),
        ],
    )
    .await;
    (id, summary.text)
}

#[tokio::test]
async fn pr_external_action_post_comment_off_creates_nothing() {
    let r = rig("automatic", "off").await;
    assert!(r
        .service
        .request_post_comment(&r.w.merge_id, "any-artifact")
        .await
        .expect("off")
        .is_none());
}

#[tokio::test]
async fn pr_external_action_post_comment_is_posted_once_with_the_marker() {
    let r = rig("automatic", "automatic").await;
    let (artifact_id, text) = summary_artifact(&r).await;
    // The Task projection may already be bound to a later admission. The
    // supersession receipt must keep this comment on the original PR number.
    exec(
        &r.w.db,
        "UPDATE pr_metadata SET provider_pr_id = '88' WHERE task_id = ?",
        &[&r.w.task_id],
    )
    .await;
    let action = r
        .service
        .request_post_comment(&r.w.merge_id, &artifact_id)
        .await
        .expect("request")
        .expect("action");
    assert_eq!(action.state, PrExternalActionState::Approved);
    // Idempotent per (admission, artifact).
    let again = r
        .service
        .request_post_comment(&r.w.merge_id, &artifact_id)
        .await
        .expect("again")
        .expect("action");
    assert_eq!(again.id, action.id);

    r.service.run_once().await.expect("executor pass");
    assert_eq!(
        r.action(&action.id).await.state,
        PrExternalActionState::Succeeded
    );
    let posted = r.mock.state.lock().expect("state").comments.clone();
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].1, text);
    assert!(r
        .mock
        .requests()
        .iter()
        .any(|line| line.contains("/issues/77/comments")));
    assert!(crate::pr_feedback::is_forge_authored(&posted[0].1));
    r.service.run_once().await.expect("replay");
    assert_eq!(r.mock.posts(), 1);
}

#[tokio::test]
async fn pr_external_action_post_comment_requires_its_exact_human_decision() {
    let r = rig("automatic", "human_approval").await;
    let (artifact_id, text) = summary_artifact(&r).await;
    let action = r
        .service
        .request_post_comment(&r.w.merge_id, &artifact_id)
        .await
        .expect("request")
        .expect("action");
    assert_eq!(action.state, PrExternalActionState::Proposed);
    assert!(action.proposal_id.is_some());
    r.service.run_once().await.expect("proposed action waits");
    assert_eq!(r.mock.posts(), 0);

    let approved = r
        .service
        .approve(&action.id, &r.owner, Some("Post the summary".to_owned()))
        .await
        .expect("owner approval");
    assert_eq!(approved.state, PrExternalActionState::Approved);
    assert!(approved.decision_id.is_some());
    r.service.run_once().await.expect("executor pass");
    assert_eq!(
        r.action(&action.id).await.state,
        PrExternalActionState::Succeeded
    );
    assert_eq!(r.mock.state.lock().expect("state").comments[0].1, text);
}

#[tokio::test]
async fn pr_external_action_post_comment_unknown_outcome_is_resolved_by_the_marker() {
    let r = rig("automatic", "automatic").await;
    let (artifact_id, text) = summary_artifact(&r).await;
    r.mock.state.lock().expect("state").post_status = 500;
    let action = r
        .service
        .request_post_comment(&r.w.merge_id, &artifact_id)
        .await
        .expect("request")
        .expect("action");
    r.service.run_once().await.expect("first pass");
    assert_eq!(
        r.action(&action.id).await.state,
        PrExternalActionState::OutcomeUnknown
    );
    // The provider had in fact stored the comment.
    r.mock.state.lock().expect("state").comments.push((9, text));
    r.service.run_once().await.expect("recovery pass");
    assert_eq!(
        r.action(&action.id).await.state,
        PrExternalActionState::Succeeded
    );
    assert_eq!(
        r.mock.posts(),
        1,
        "no second POST after the marker was found"
    );
}

#[tokio::test]
async fn pr_external_action_post_comment_yields_to_the_push_update() {
    let r = rig("automatic", "human_approval").await;
    let (artifact_id, _) = summary_artifact(&r).await;
    let comment = r
        .service
        .request_post_comment(&r.w.merge_id, &artifact_id)
        .await
        .expect("request")
        .expect("action");
    assert_eq!(comment.state, PrExternalActionState::Proposed);
    let push = r
        .service
        .ensure_push_update(&r.merge2)
        .await
        .expect("the proposed comment yields its single open slot");
    assert_eq!(push.kind, PrExternalActionKind::PushUpdate);
    assert_eq!(
        r.action(&comment.id).await.state,
        PrExternalActionState::Abandoned
    );
}
