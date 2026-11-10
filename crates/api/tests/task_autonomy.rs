mod common;

use api_types::{TaskAutonomyLimits, TaskResponse};
use axum::http::{Method, StatusCode};
use common::{create_project_and_repo, json_request, setup_git_repo, test_app, TestDir};
use serde_json::{json, Value};

#[tokio::test]
async fn limits_are_versioned_replay_safe_and_preserve_consumption() {
    let repo = TestDir::new("autonomy-repo");
    let path = setup_git_repo(repo.path());
    let workspaces = TestDir::new("autonomy-workspaces");
    let harness = test_app(workspaces.path(), "autonomy").await;
    let (project_id, _) = create_project_and_repo(&harness.app, "Autonomy", &path).await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title":"Bounded autonomy"}),
        StatusCode::OK,
    )
    .await;
    let uri = format!("/api/v1/tasks/{}/coordination/limits", task.id);
    let initial: TaskAutonomyLimits =
        json_request(&harness.app, Method::GET, &uri, json!({}), StatusCode::OK).await;
    assert_eq!(
        (
            initial.max_turns,
            initial.max_quality_requests,
            initial.max_rework_rounds,
            initial.max_route_requests
        ),
        (32, 64, 16, 16)
    );
    sqlx::query(
        "UPDATE task_coordination SET turn_count=3,quality_request_count=2 WHERE task_id=?",
    )
    .bind(&task.id)
    .execute(harness.state.db.pool())
    .await
    .unwrap();
    let mut limits = serde_json::to_value(&initial).unwrap();
    limits["max_rework_rounds"] = json!(24);
    let request = json!({"expected_version":initial.version,"limits":limits,"reason":"Authorize more rounds","idempotency_key":"extra-rounds-1"});
    let first: TaskAutonomyLimits = json_request(
        &harness.app,
        Method::PUT,
        &uri,
        request.clone(),
        StatusCode::OK,
    )
    .await;
    let replay: TaskAutonomyLimits = json_request(
        &harness.app,
        Method::PUT,
        &uri,
        request.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(replay).unwrap()
    );
    assert_eq!(first.version, initial.version + 1);
    let consumed: (i64, i64) = sqlx::query_as(
        "SELECT turn_count,quality_request_count FROM task_coordination WHERE task_id=?",
    )
    .bind(&task.id)
    .fetch_one(harness.state.db.pool())
    .await
    .unwrap();
    assert_eq!(consumed, (3, 2));
    let mut stale = request.clone();
    stale["idempotency_key"] = json!("stale");
    let _: Value = json_request(&harness.app, Method::PUT, &uri, stale, StatusCode::CONFLICT).await;
    let mut conflicting = request;
    conflicting["reason"] = json!("different request");
    let _: Value = json_request(
        &harness.app,
        Method::PUT,
        &uri,
        conflicting,
        StatusCode::CONFLICT,
    )
    .await;
    let now = db::now_rfc3339();
    db::UserRepo::create_user(
        &*harness.state.db,
        &db::User {
            id: "outsider".into(),
            email: "outsider@example.com".into(),
            password_hash: "fixture".into(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .unwrap();
    let denied = common::raw_json_request_with_bearer(&harness.app, Method::PUT, &uri, &common::test_jwt_for_user("outsider","outsider@example.com",false), json!({"expected_version":first.version,"limits":first,"reason":"Not authorized","idempotency_key":"outsider"})).await;
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    let now = db::now_rfc3339();
    db::ProjectMemberRepo::add_member(
        &*harness.state.db,
        db::CreateProjectMember {
            id: db::new_uuid_v4(),
            project_id,
            user_id: "outsider".into(),
            role: "member".into(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .unwrap();
    let denied = common::raw_json_request_with_bearer(&harness.app, Method::PUT, &uri, &common::test_jwt_for_user("outsider","outsider@example.com",false), json!({"expected_version":first.version,"limits":first,"reason":"Not an administrator","idempotency_key":"ordinary-member"})).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn project_coordinator_preference_is_copied_without_starting_provider_work() {
    let repo = TestDir::new("coordinator-default-repo");
    let path = setup_git_repo(repo.path());
    let workspaces = TestDir::new("coordinator-default-workspaces");
    let harness = test_app(workspaces.path(), "coordinator-default").await;
    let (project_id, _) = create_project_and_repo(&harness.app, "Default coordinator", &path).await;
    let (agent, _) =
        common::create_shell_agents(&harness.app, workspaces.path(), "coordinator-default").await;
    let defaults_uri = format!("/api/v1/projects/{project_id}/task-role-defaults");
    let defaults: Value = json_request(
        &harness.app,
        Method::GET,
        &defaults_uri,
        json!({}),
        StatusCode::OK,
    )
    .await;
    let saved: Value = json_request(&harness.app, Method::PUT, &defaults_uri, json!({
        "expected_version":defaults["version"],"roles":[{"role":"delivery-lead","coordination_mode":"independent","members":[{"kind":"agent","id":agent}],"configuration":{"capabilities":["orchestration"],"coordinator_actor":{"kind":"agent","id":agent}}}]
    }), StatusCode::OK).await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title":"Wait for explicit Start"}),
        StatusCode::OK,
    )
    .await;
    let preference:(String,Option<String>,i64)=sqlx::query_as("SELECT mode,coordinator_actor_id,authorization_version FROM task_coordination WHERE task_id=?").bind(&task.id).fetch_one(harness.state.db.pool()).await.unwrap();
    assert_eq!(preference, ("manual".into(), Some(agent.clone()), 1));
    let runtime = services::orchestrator_runtime::OrchestratorRuntime::new(
        harness.state.db.clone(),
        harness.state.event_bus.clone(),
        harness.state.task_service.clone(),
    );
    let result = runtime.run_once(100).await.unwrap();
    assert_eq!(result.admitted_wakes, 0);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM execution WHERE task_id=?")
        .bind(&task.id)
        .fetch_one(harness.state.db.pool())
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "copying a coordinator preference never launches a provider"
    );
    let without_preference:Value=json_request(&harness.app,Method::PUT,&defaults_uri,json!({"expected_version":saved["version"],"roles":[{"role":"delivery-lead","coordination_mode":"independent","members":[{"kind":"agent","id":agent}],"configuration":{"capabilities":["orchestration"]}}]}),StatusCode::OK).await;
    let manual: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title":"Template without preferred coordinator"}),
        StatusCode::OK,
    )
    .await;
    let preference: Option<String> =
        sqlx::query_scalar("SELECT coordinator_actor_id FROM task_coordination WHERE task_id=?")
            .bind(&manual.id)
            .fetch_one(harness.state.db.pool())
            .await
            .unwrap();
    assert!(preference.is_none());
    assert_eq!(
        runtime.run_once(100).await.unwrap().admitted_wakes,
        0,
        "new Tasks wait for Start even with older templates that have no preferred coordinator"
    );
    let _: Value = json_request(
        &harness.app,
        Method::PUT,
        &defaults_uri,
        json!({"expected_version":without_preference["version"],"roles":[]}),
        StatusCode::OK,
    )
    .await;
    let still: Option<String> =
        sqlx::query_scalar("SELECT coordinator_actor_id FROM task_coordination WHERE task_id=?")
            .bind(&task.id)
            .fetch_one(harness.state.db.pool())
            .await
            .unwrap();
    assert_eq!(
        still,
        Some(agent),
        "later Project changes preserve existing Task defaults"
    );
}
