#![allow(dead_code, clippy::assertions_on_constants)]
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use api::{build_router, AppState};
use api_types::{
    ActorRef, ErrorResponse, ReviewExecutionResponse, ReviewReportVerdict,
    StartReviewExecutionRequest, SubmitReviewReportRequest, SubmitReviewReportResponse,
    TaskResponse, ValidationRunResponse,
};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use db::{
    new_uuid_v4, now_rfc3339, AssigneeKind, CreateExecution, CreateProject, CreateRepo,
    CreateReview, CreateTask, CreateTaskRoleAssignment, CreateWorkspace, ExecutionRepo,
    ExecutionStatus, ProjectRepo, RepoRepo, ReviewRepo, ReviewStatus, TaskRepo,
    TaskRoleAssignmentRepo, UpdateProject, ValidationRunStatus, WorkspaceRepo, WorkspaceStatus,
};
use events::EventBus;
use serde::de::DeserializeOwned;
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn legacy_review_rows_are_not_projected_as_current_reviews() {
    let workspace_root = TestDir::new("forge-reviews-workspaces");
    let harness = test_app(workspace_root.path()).await;
    let step_results_json = json!({
        "ci_steps": [{
            "index": 0,
            "command": "cargo test",
            "exit_code": 0,
            "stderr_tail": ""
        }],
        "auditor": {
            "verdict": "pass",
            "reason": null
        }
    })
    .to_string();
    let (_review_id, _execution_id, task_id) =
        seed_review(&harness.state.db, step_results_json).await;
    TaskRepo::set_review_passed_at(
        &*harness.state.db,
        &task_id,
        Some(now_rfc3339()),
        &now_rfc3339(),
    )
    .await
    .expect("legacy review timestamp seeds");

    let reviews: Vec<ReviewExecutionResponse> = json_empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{task_id}/reviews"),
        StatusCode::OK,
    )
    .await;

    assert!(
        reviews.is_empty(),
        "legacy Review rows are not current authority"
    );
    let task: TaskResponse = json_empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{task_id}"),
        StatusCode::OK,
    )
    .await;
    assert!(
        task.review_passed_at.is_none(),
        "legacy pass timestamp is not projected as current Review authority"
    );
}

#[tokio::test]
async fn human_review_creates_an_exact_human_execution_and_report() {
    let workspace_root = TestDir::new("forge-reviews-workspaces");
    let harness = test_app(workspace_root.path()).await;
    let (_review_id, _legacy_execution_id, task_id) =
        seed_review(&harness.state.db, "[]".to_owned()).await;
    TaskRoleAssignmentRepo::assign(
        &*harness.state.db,
        CreateTaskRoleAssignment {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            role_name: "reviewer".to_owned(),
            assignee_type: Some(AssigneeKind::User),
            assignee_id: Some("test-user-id".to_owned()),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Human reviewer assignment creates");

    let started: ReviewExecutionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/review"),
        serde_json::to_value(StartReviewExecutionRequest { workspace_id: None }).unwrap(),
        StatusCode::OK,
    )
    .await;

    assert_eq!(
        started.execution.actor_ref,
        Some(ActorRef::Human("test-user-id".to_owned()))
    );
    assert_eq!(started.execution.purpose.as_deref(), Some("review"));
    assert_eq!(
        started.execution.status,
        api_types::ExecutionStatus::Running
    );
    assert!(started.execution.harness_session_id.is_none());
    assert!(started.report.is_none());

    let submitted: SubmitReviewReportResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/reviews/{}", started.execution.id),
        serde_json::to_value(SubmitReviewReportRequest {
            verdict: ReviewReportVerdict::Pass,
            summary: "The exact subject is sound.".to_owned(),
            criteria: vec!["correctness".to_owned()],
            findings: Vec::new(),
            questions: Vec::new(),
            evidence_ids: Vec::new(),
            artifact_ids: Vec::new(),
        })
        .unwrap(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        submitted.review_execution.execution.id,
        started.execution.id
    );
    assert_eq!(
        submitted.review_execution.execution.status,
        api_types::ExecutionStatus::Completed
    );
    assert_eq!(submitted.report.kind, api_types::ArtifactKind::ReviewReport);
    let artifact = db::CollaborationRepo::get_artifact(&*harness.state.db, &submitted.report.id)
        .await
        .expect("ReviewReport loads")
        .expect("ReviewReport persists");
    assert!(matches!(
        artifact.producer,
        db::ArtifactProducer::Execution { execution_id, actor: db::ActorRef::Human(user_id) }
            if execution_id == started.execution.id && user_id == "test-user-id"
    ));
    assert!(
        db::ValidationRunRepo::list_validation_runs_by_task(&*harness.state.db, &task_id)
            .await
            .expect("ValidationRun history loads")
            .is_empty(),
        "Review PASS does not imply Validation PASS or a ValidationRun"
    );
}

#[tokio::test]
async fn legacy_review_gate_approval_projects_to_the_running_human_execution() {
    let workspace_root = TestDir::new("forge-review-gate-compatibility");
    let harness = test_app(workspace_root.path()).await;
    let (_legacy_review_id, _legacy_execution_id, task_id) =
        seed_review(&harness.state.db, "[]".to_owned()).await;
    TaskRoleAssignmentRepo::assign(
        &*harness.state.db,
        CreateTaskRoleAssignment {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            role_name: "reviewer".to_owned(),
            assignee_type: Some(AssigneeKind::User),
            assignee_id: Some("test-user-id".to_owned()),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Human reviewer assignment creates");
    let started: ReviewExecutionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/review"),
        serde_json::to_value(StartReviewExecutionRequest { workspace_id: None }).unwrap(),
        StatusCode::OK,
    )
    .await;
    let task = TaskRepo::get_by_id(&*harness.state.db, &task_id, false)
        .await
        .expect("Task loads")
        .expect("Task exists");

    let decided: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/gates/review/approve"),
        serde_json::json!({
            "version": task.version,
            "reason": "Approve the exact Human Review Execution"
        }),
        StatusCode::OK,
    )
    .await;
    assert_ne!(decided.status, "review");
    let execution = ExecutionRepo::get_by_id(&*harness.state.db, &started.execution.id)
        .await
        .expect("Review Execution loads")
        .expect("Review Execution exists");
    assert_eq!(execution.status, ExecutionStatus::Completed);
    let report = db::CollaborationRepo::get_execution_artifact_output(
        &*harness.state.db,
        &execution.id,
        db::ArtifactKind::ReviewReport,
    )
    .await
    .expect("exact ReviewReport loads")
    .expect("Human gate projection creates a ReviewReport");
    assert!(matches!(
        report.producer,
        db::ArtifactProducer::Execution { execution_id, actor: db::ActorRef::Human(user_id) }
            if execution_id == execution.id && user_id == "test-user-id"
    ));
    assert_eq!(
        db::ReviewRepo::list_by_task(&*harness.state.db, &task_id)
            .await
            .expect("legacy Review history loads")
            .len(),
        1,
        "compatibility approval does not append to the legacy Review table"
    );
}

#[tokio::test]
async fn legacy_review_gate_rejection_creates_report_and_collaboration_message() {
    let workspace_root = TestDir::new("forge-review-reject-gate-compatibility");
    let harness = test_app(workspace_root.path()).await;
    let (_legacy_review_id, _legacy_execution_id, task_id) =
        seed_review(&harness.state.db, "[]".to_owned()).await;
    TaskRoleAssignmentRepo::assign(
        &*harness.state.db,
        CreateTaskRoleAssignment {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            role_name: "reviewer".to_owned(),
            assignee_type: Some(AssigneeKind::User),
            assignee_id: Some("test-user-id".to_owned()),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Human reviewer assignment creates");
    let started: ReviewExecutionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/review"),
        serde_json::to_value(StartReviewExecutionRequest { workspace_id: None }).unwrap(),
        StatusCode::OK,
    )
    .await;
    let task = TaskRepo::get_by_id(&*harness.state.db, &task_id, false)
        .await
        .expect("Task loads")
        .expect("Task exists");

    let changed: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/gates/review/reject"),
        serde_json::json!({
            "version": task.version,
            "reason": "Add the missing edge-case evidence"
        }),
        StatusCode::OK,
    )
    .await;
    assert_ne!(changed.status, "review");
    let report = db::CollaborationRepo::get_execution_artifact_output(
        &*harness.state.db,
        &started.execution.id,
        db::ArtifactKind::ReviewReport,
    )
    .await
    .expect("exact ReviewReport loads")
    .expect("Human gate projection creates a ReviewReport");
    let messages = db::CollaborationRepo::list_messages(
        &*harness.state.db,
        &task_id,
        db::PageRequest {
            cursor: None,
            limit: 20,
            include_total: false,
            sort_by: db::SortBy::CreatedAt,
            sort_order: db::SortOrder::Desc,
        },
    )
    .await
    .expect("Review collaboration messages load");
    assert!(messages.items.iter().any(|message| {
        message.artifact_ids.contains(&report.id)
            && matches!(message.sender, db::ActorRef::Human(ref user_id) if user_id == "test-user-id")
    }));
}

#[tokio::test]
async fn unknown_review_returns_standard_not_found() {
    let workspace_root = TestDir::new("forge-reviews-workspaces");
    let harness = test_app(workspace_root.path()).await;

    let error: ErrorResponse = json_empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/reviews/{}", new_uuid_v4()),
        StatusCode::NOT_FOUND,
    )
    .await;

    assert_eq!(error.code, "not_found");
}

#[tokio::test]
async fn validation_api_returns_exact_run_and_evidence_identities() {
    let workspace_root = TestDir::new("forge-validation-workspaces");
    let harness = test_app(workspace_root.path()).await;
    let (_review_id, _execution_id, task_id) =
        seed_review(&harness.state.db, "[]".to_owned()).await;
    let task = TaskRepo::get_by_id(&*harness.state.db, &task_id, false)
        .await
        .expect("Task loads")
        .expect("Task exists");
    let worktree_path = workspace_root.path().join("validation-worktree");
    std::fs::create_dir_all(&worktree_path).expect("worktree directory creates");
    for (args, label) in [
        (vec!["init"], "git initializes"),
        (
            vec!["config", "user.email", "test@forge.dev"],
            "git email configures",
        ),
        (
            vec!["config", "user.name", "Forge Test"],
            "git name configures",
        ),
    ] {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&worktree_path)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "{label}: {:?}", output.stderr);
    }
    std::fs::write(worktree_path.join("README.md"), "validation API\n")
        .expect("worktree file writes");
    for args in [vec!["add", "-A"], vec!["commit", "-m", "baseline"]] {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&worktree_path)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git mutation succeeds: {:?}",
            output.stderr
        );
    }
    let workspace = WorkspaceRepo::create(
        &*harness.state.db,
        CreateWorkspace {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            repo_id: task.repo_id.expect("Task repo exists"),
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            branch: "validation-api".to_owned(),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Workspace creates");
    let check = services::ValidationService::new(
        Arc::clone(&harness.state.db),
        Arc::clone(&harness.state.event_bus),
    )
    .run_command(
        &task_id,
        &workspace.id,
        "printf 'validation api proof\\n'",
        0,
        None,
        None,
    )
    .await
    .expect("deterministic ValidationRun completes");
    assert_eq!(check.run.status, ValidationRunStatus::Passed);

    let listed: Vec<ValidationRunResponse> = json_empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{task_id}/validations"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, check.run.id);
    assert_eq!(listed[0].workspace_id, workspace.id);
    assert_eq!(listed[0].commit_sha, check.run.commit_sha);
    assert_eq!(listed[0].evidence_ids, vec![check.evidence[0].id.clone()]);

    let fetched: ValidationRunResponse = json_empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/validations/{}", check.run.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(fetched.id, check.run.id);
    assert_eq!(fetched.evidence_ids, listed[0].evidence_ids);
    let evidence: api_types::EvidenceResponse = json_empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/evidence/{}", check.evidence[0].id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(evidence.id, check.evidence[0].id);
    assert_eq!(evidence.producer_validation_run_id, check.run.id);
    assert_eq!(evidence.content["commit_sha"], check.run.commit_sha);
    assert_eq!(evidence.content["workspace_id"], workspace.id);
}

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
    let now = now_rfc3339();
    db::UserRepo::create_user(
        &*db,
        &db::User {
            id: "test-user-id".to_owned(),
            email: "test@example.com".to_owned(),
            password_hash: "$2b$04$placeholder".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("test Human exists");
    let adapter_registry = Arc::new(cli_adapters::default_registry());
    services::ensure_default_agents(db.as_ref(), &adapter_registry)
        .await
        .expect("default agents upsert");
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
    let review_runner = Arc::new(review::ReviewRunner::new(
        Arc::clone(&db),
        Arc::clone(&event_bus),
        Arc::clone(&adapter_registry),
    ));
    let state = Arc::new(AppState::with_adapter_registry_services_and_shutdown(
        db,
        event_bus,
        true,
        adapter_registry,
        merge_service,
        cleanup_scheduler,
        review_runner,
        api::state::ShutdownSignal::new(),
        api::state::test_workflows_dir(),
        api::state::test_jwt_secret(),
        api::state::test_bcrypt_cost(),
    ));

    let web_dist_dir = TestDir::new("forge-reviews-web");
    std::fs::write(web_dist_dir.path().join("index.html"), "<html></html>").expect("write index");
    let app = build_router((*state).clone(), web_dist_dir.path().to_path_buf());

    TestHarness {
        app,
        state,
        _web_dist_dir: web_dist_dir,
    }
}

async fn seed_review(db: &db::SqliteDb, step_results_json: String) -> (String, String, String) {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let task_id = new_uuid_v4();
    let execution_id = new_uuid_v4();
    let review_id = new_uuid_v4();

    ProjectRepo::create(
        db,
        CreateProject {
            id: project_id.clone(),
            name: "Reviews".to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_string(),
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
            local_path: Some("/tmp/forge-reviews-repo".to_owned()),
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
            assignee_type: None,
            assignee_id: None,
            title: "Review details".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            status: "review".to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task creates");
    ExecutionRepo::create(
        db,
        CreateExecution {
            id: execution_id.clone(),
            task_id: task_id.clone(),
            agent_id: None,
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("execution creates");
    ReviewRepo::create(
        db,
        CreateReview {
            id: review_id.clone(),
            task_id: task_id.clone(),
            execution_id: execution_id.clone(),
            attempt_number: 1,
            status: ReviewStatus::Passed,
            step_results_json,
            started_at: now.clone(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("review creates");

    (review_id, execution_id, task_id)
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
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {}", test_jwt()))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&body).expect("body serializes"),
                ))
                .expect("build JSON request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    assert_eq!(
        status,
        expected_status,
        "unexpected response status with body: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("parse JSON response")
}

async fn json_empty_request<T>(
    app: &Router,
    method: Method,
    uri: &str,
    expected_status: StatusCode,
) -> T
where
    T: DeserializeOwned,
{
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {}", test_jwt()))
                .body(Body::empty())
                .expect("build empty request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    assert_eq!(
        status,
        expected_status,
        "unexpected response status with body: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("parse JSON response")
}

fn test_jwt() -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "test-user-id",
        "email": "test@example.com",
        "is_admin": true,
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
