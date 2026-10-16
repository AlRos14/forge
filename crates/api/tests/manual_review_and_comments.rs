#![allow(dead_code, clippy::assertions_on_constants)]
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use api::{build_router, AppState};
use api_types::{
    AuthorType, CommentResponse, PaginatedResponse, ProjectResponse, RepoResponse, TaskResponse,
};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use db::{
    new_uuid_v4, now_rfc3339, CommentAuthorType, CreateProject, CreateRepo, CreateTask,
    CreateTaskComment, ProjectRepo, RepoRepo, TaskCommentRepo, TaskRepo, UpdateProject,
};
use events::EventBus;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::process::Stdio;
use tower::ServiceExt;

// ── Test 13.6: comment CRUD ──

#[tokio::test]
async fn comment_create_list_delete() {
    let workspace_root = TestDir::new("forge-comments-crud");
    let harness = test_app(workspace_root.path()).await;
    let (repo_path, _) = setup_git_repo(workspace_root.path(), "comments");

    let project: ProjectResponse = json_request(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        json!({ "name": "Comment Test" }),
        StatusCode::OK,
    )
    .await;

    let _repo: RepoResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/repos", project.id),
        json!({
            "name": "repo",
            "local_path": repo_path.to_string_lossy(),
            "remote_url": repo_path.to_string_lossy(),
            "default_branch": "main"
        }),
        StatusCode::OK,
    )
    .await;

    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        json!({ "title": "Comment test task",
            "description": "test"
        }),
        StatusCode::OK,
    )
    .await;

    // Create a user comment
    let comment: CommentResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/comments", task.id),
        json!({ "content": "Hello world", "author_name": "Tester" }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(comment.content, "Hello world");
    assert_eq!(comment.author_name, "Tester");
    assert_eq!(comment.author_type, AuthorType::User);

    // List comments
    let listed: PaginatedResponse<CommentResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/comments", task.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].content, "Hello world");

    // Delete user comment
    let delete_response = raw_request(
        &harness.app,
        Method::DELETE,
        &format!("/api/v1/comments/{}", comment.id),
        json!(null),
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    // Verify it's gone
    let listed: PaginatedResponse<CommentResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/comments", task.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.items.len(), 0);
}

#[tokio::test]
async fn system_comments_cannot_be_deleted() {
    let workspace_root = TestDir::new("forge-comments-system-delete");
    let harness = test_app(workspace_root.path()).await;

    let task_id = seed_task_with_system_comment(&harness.state.db).await;

    let listed: PaginatedResponse<CommentResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{task_id}/comments"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.items.len(), 1);
    let system_comment_id = &listed.items[0].id;

    let delete_response = raw_request(
        &harness.app,
        Method::DELETE,
        &format!("/api/v1/comments/{system_comment_id}"),
        json!(null),
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::FORBIDDEN);
}

// ── Harness ──

struct TestHarness {
    app: Router,
    state: Arc<AppState>,
    _web_dist_dir: TestDir,
}

async fn test_app(workspace_root: &Path) -> TestHarness {
    let pool = db::create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    db::run_migrations(&pool).await.expect("migrations run");

    let db = Arc::new(db::SqliteDb::new(pool));
    let adapter_registry = Arc::new(executors::HarnessAdapterRegistry::new());
    let event_bus = Arc::new(EventBus::new(256));
    let merge_service = Arc::new(services::MergeService::new(
        Arc::clone(&db),
        Arc::clone(&event_bus),
        workspace_root.to_path_buf(),
    ));
    let cleanup_scheduler = Arc::new(services::WorkspaceCleanupScheduler::new(
        Arc::clone(&db),
        Arc::clone(&event_bus),
        workspace_root.to_path_buf(),
    ));
    let state = Arc::new(AppState::with_adapter_registry_services_and_shutdown(
        db,
        event_bus,
        true,
        adapter_registry,
        merge_service,
        cleanup_scheduler,
        api::state::ShutdownSignal::new(),
        api::state::test_workflows_dir(),
        api::state::test_jwt_secret(),
        api::state::test_bcrypt_cost(),
    ));

    let web_dist_dir = TestDir::new("forge-manual-review-web");
    std::fs::write(web_dist_dir.path().join("index.html"), "<html></html>").expect("write index");
    let app = build_router((*state).clone(), web_dist_dir.path().to_path_buf());

    TestHarness {
        app,
        state,
        _web_dist_dir: web_dist_dir,
    }
}

async fn seed_task_with_system_comment(db: &db::SqliteDb) -> String {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let task_id = new_uuid_v4();

    ProjectRepo::create(
        db,
        CreateProject {
            id: project_id.clone(),
            name: "SystemComment".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");
    RepoRepo::create(
        db,
        CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "repo".to_owned(),
            local_path: Some("/tmp/forge-sys-comment-repo".to_owned()),
            work_mode: db::WorkMode::DirectMerge,
            remote_url: String::new(),
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("repo creates");
    ProjectRepo::update(
        db,
        UpdateProject {
            id: project_id.clone(),
            name: None,
            settings: None,
            primary_repo_id: Some(Some(repo_id.clone())),
            paused_at: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("project primary repo updates");
    TaskRepo::create(
        db,
        CreateTask {
            id: task_id.clone(),
            project_id,
            repo_id: Some(repo_id),
            parent_task_id: None,
            subtask_order: None,
            title: "System comment test".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task creates");

    TaskCommentRepo::create_comment(
        db,
        CreateTaskComment {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            author_type: CommentAuthorType::System,
            author_id: None,
            author_name: "Forge".to_owned(),
            content: "System generated comment".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("system comment creates");

    task_id
}

// ── Git helper ──

fn setup_git_repo(workspace_root: &Path, task_id: &str) -> (PathBuf, PathBuf) {
    let repo_path = workspace_root.join("repos").join(task_id);
    std::fs::create_dir_all(&repo_path).expect("create repo dir");
    run_git(&repo_path, &["init", "--initial-branch=main"]);
    run_git(&repo_path, &["config", "user.email", "test@test.com"]);
    run_git(&repo_path, &["config", "user.name", "Test"]);
    std::fs::write(repo_path.join("file.txt"), "base\n").expect("write base file");
    run_git(&repo_path, &["add", "."]);
    run_git(&repo_path, &["commit", "-m", "initial"]);

    let worktree_path = workspace_root.join("worktrees").join(task_id).join("repo");
    std::fs::create_dir_all(worktree_path.parent().unwrap()).expect("create worktree parent");
    run_git(
        &repo_path,
        &[
            "worktree",
            "add",
            worktree_path.to_str().unwrap(),
            "-b",
            &::workspace::task_branch_name(task_id),
        ],
    );
    std::fs::write(worktree_path.join("file.txt"), "feature\n").expect("write feature file");
    run_git(&worktree_path, &["add", "."]);
    run_git(&worktree_path, &["commit", "-m", "feature"]);

    (repo_path, worktree_path)
}

fn run_git(path: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {} failed\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// ── Request helpers ──

fn test_jwt() -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "test-user-id",
        "email": "test@example.com",
        "iat": now,
        "exp": now + 900,
    });
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(b"test-jwt-secret-for-development"),
    )
    .expect("encode test jwt")
}

async fn json_request<T>(
    app: &Router,
    method: Method,
    uri: &str,
    body: serde_json::Value,
    expected_status: StatusCode,
) -> T
where
    T: DeserializeOwned,
{
    let token = test_jwt();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .expect("build request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    assert_eq!(
        status,
        expected_status,
        "unexpected status with body: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("parse JSON")
}

async fn empty_request<T>(app: &Router, method: Method, uri: &str, expected_status: StatusCode) -> T
where
    T: DeserializeOwned,
{
    let token = test_jwt();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    assert_eq!(
        status,
        expected_status,
        "unexpected status with body: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("parse JSON")
}

async fn raw_request(
    app: &Router,
    method: Method,
    uri: &str,
    body: serde_json::Value,
) -> axum::http::Response<Body> {
    let token = test_jwt();
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(body)
                .expect("build request"),
        )
        .await
        .expect("router response")
}

// ── TestDir ──

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("temp dir creates");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
