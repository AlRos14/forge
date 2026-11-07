use api_types::{ProjectResponse, TaskResponse};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

mod common;

async fn mcp(app: &axum::Router, token: &str, params: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/mcp")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(params.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn copilot_mcp_delegation_is_scoped_attributed_and_revocable() {
    let directory = common::TestDir::new("copilot-mcp-authority");
    let harness = common::test_app(directory.path(), "copilot-mcp-authority").await;
    let user_token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &user_token,
        json!({"name":"Copilot scope"}),
        StatusCode::OK,
    )
    .await;
    let other: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &user_token,
        json!({"name":"Other scope"}),
        StatusCode::OK,
    )
    .await;
    let task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &user_token,
        json!({"title":"Plan scope"}),
        StatusCode::OK,
    )
    .await;
    let other_task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", other.id),
        &user_token,
        json!({"title":"Other Task"}),
        StatusCode::OK,
    )
    .await;
    let sibling_task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &user_token,
        json!({"title":"Another Task in the same Project"}),
        StatusCode::OK,
    )
    .await;
    let _: Value = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles", sibling_task.id),
        &user_token,
        json!({"role":"planner","coordination_mode":"independent","policy":{}}),
        StatusCode::OK,
    )
    .await;
    let _: Value = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        &user_token,
        json!({"role":"planner","coordination_mode":"independent","policy":{}}),
        StatusCode::OK,
    )
    .await;
    let initial_agent = common::create_harness_agent(&harness.app, &user_token, "Copilot actor")
        .await
        .agent;
    let now = db::now_rfc3339();
    let daemon = db::DaemonRepo::upsert_by_machine_id(
        &*harness.state.db,
        db::UpsertDaemon {
            id: db::new_uuid_v4(),
            machine_id: services::embedded_daemon::embedded_machine_id(),
            hostname: "fixture-host".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            agent_version: None,
            labels_json: "{}".to_owned(),
            status: db::DaemonStatus::Online,
            registration_token_hash: None,
            owner_id: Some("test-user-id".to_owned()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .unwrap();
    db::DaemonRepo::update_report(
        &*harness.state.db,
        db::UpdateDaemonReport {
            id: daemon.id.clone(),
            last_report_at: now.clone(),
            status: db::DaemonStatus::Online,
            detected_clis_json: json!([{"kind":"codex","availability":"authenticated"}])
                .to_string(),
            labels_json: None,
            updated_at: now.clone(),
        },
    )
    .await
    .unwrap();
    let agent: api_types::AgentResponse = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/agents/{}", initial_agent.id),
        &user_token,
        json!({"version":initial_agent.version,"daemon_id":daemon.id}),
        StatusCode::OK,
    )
    .await;
    let durable_agent = db::AgentRepo::get_by_id(&*harness.state.db, &agent.id)
        .await
        .unwrap()
        .unwrap();
    let durable_daemon = db::DaemonRepo::get_by_id(&*harness.state.db, &daemon.id)
        .await
        .unwrap()
        .unwrap();
    let snapshot = json!({"agent_id":durable_agent.id,"agent_version":durable_agent.version,"executor_type":durable_agent.executor_type,"profile_id":durable_agent.profile_id,"backend_kind":"cli","config":{},"daemon_authority":db::daemon_authority_snapshot(&durable_agent,&durable_daemon)}).to_string();
    let token = "fgc_abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";
    let hash = hex::encode(Sha256::digest(token.as_bytes()));
    let expires = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    sqlx::query("INSERT INTO project_copilot (project_id,agent_id,version,created_at,updated_at) VALUES (?,?,1,?,?) ON CONFLICT(project_id) DO UPDATE SET agent_id=excluded.agent_id,version=1")
        .bind(&project.id).bind(&agent.id).bind(&now).bind(&now)
        .execute(harness.state.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO project_copilot_run (id,project_id,agent_id,user_id,binding_version,agent_version,agent_snapshot_json,context_task_id,status,idempotency_key,request_digest,mcp_token_hash,mcp_token_expires_at,created_at,updated_at) VALUES ('run-copilot',?,?,'test-user-id',1,?,?,?,'running','fixture',?,?,?, ?,?)")
        .bind(&project.id).bind(&agent.id).bind(agent.version).bind(snapshot).bind(&task.id)
        .bind("a".repeat(64)).bind(&hash).bind(&expires).bind(&now).bind(&now)
        .execute(harness.state.db.pool()).await.unwrap();

    db::ProjectCopilotRepo::authorize_run(&*harness.state.db, "run-copilot")
        .await
        .expect("fixture has current run authority");
    let (status, tools) = mcp(
        &harness.app,
        token,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect();
    assert!(names.contains(&"forge_create_work_unit"));
    assert!(!names.contains(&"forge_list_tasks"));
    assert!(!names.contains(&"forge_start_execution"));
    assert!(!names.contains(&"forge_create_project"));
    assert!(!names.contains(&"forge_list_agents"));
    assert!(!names.contains(&"forge_list_agent_profiles"));
    let request = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"forge_create_work_unit","arguments":{"task_id":task.id,
        "title":"Inspect baseline","scope":"Read approved baseline; output source index",
        "role":"planner","requires_integration":false,"idempotency_key":"baseline"}}});
    let (_, created) = mcp(&harness.app, token, request.clone()).await;
    assert!(created.get("error").is_none(), "{created}");
    let unit: Value =
        serde_json::from_str(created["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(unit["created_by"], json!({"kind":"agent","id":agent.id}));
    let (_, replayed) = mcp(&harness.app, token, request).await;
    assert_eq!(replayed["result"], created["result"]);
    let events_before_denied_calls: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM domain_event WHERE correlation_id='run-copilot'")
            .fetch_one(harness.state.db.pool())
            .await
            .unwrap();

    let activity: api_types::ProjectCopilotActivityResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}/copilot/activity?limit=2", project.id),
        &user_token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(activity.items.len(), 2);
    assert!(activity
        .items
        .iter()
        .all(|item| item.actor_id == agent.id && item.requested_by_user_id == "test-user-id"));
    assert!(!serde_json::to_string(&activity).unwrap().contains(token));
    let next = activity
        .next_cursor
        .as_deref()
        .expect("activity has older records");
    let older: api_types::ProjectCopilotActivityResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!(
            "/api/v1/projects/{}/copilot/activity?limit=2&cursor={next}",
            project.id
        ),
        &user_token,
        StatusCode::OK,
    )
    .await;
    assert!(older
        .items
        .iter()
        .all(|item| !activity.items.iter().any(|newer| newer.id == item.id)));
    let other_activity: api_types::ProjectCopilotActivityResponse =
        common::empty_request_with_bearer(
            &harness.app,
            Method::GET,
            &format!("/api/v1/projects/{}/copilot/activity", other.id),
            &user_token,
            StatusCode::OK,
        )
        .await;
    assert!(other_activity.items.is_empty());

    let mismatched_scope = harness
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/mcp?project_id={}", other.id))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":7,"method":"tools/list"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mismatched_scope.status(), StatusCode::FORBIDDEN);

    for (tool, arguments) in [
        ("forge_list_tasks", json!({})),
        ("forge_get_task", json!({"task_id":other_task.id})),
        ("forge_start_execution", json!({})),
        ("forge_list_agents", json!({})),
        ("forge_list_agent_profiles", json!({"agent_id":agent.id})),
        (
            "forge_create_work_unit",
            json!({"task_id":sibling_task.id,"title":"Wrong target",
            "scope":"Must not be created outside the attached Task", "role":"planner",
            "requires_integration":false,"idempotency_key":"wrong-task"}),
        ),
    ] {
        let (_, result) = mcp(&harness.app, token, json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":tool,"arguments":arguments}})).await;
        assert!(result.get("error").is_some(), "{result}");
    }
    let events_after_denied_calls: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM domain_event WHERE correlation_id='run-copilot'")
            .fetch_one(harness.state.db.pool())
            .await
            .unwrap();
    assert_eq!(
        events_after_denied_calls, events_before_denied_calls,
        "forbidden and out-of-context tools create no Copilot activity events"
    );
    let sibling_units: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_unit WHERE task_id=?")
        .bind(&sibling_task.id)
        .fetch_one(harness.state.db.pool())
        .await
        .unwrap();
    assert_eq!(sibling_units, 0);
    let response = common::raw_json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        token,
        json!({"title":"No REST delegation"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    sqlx::query("UPDATE project_copilot SET version=version+1 WHERE project_id=?")
        .bind(&project.id)
        .execute(harness.state.db.pool())
        .await
        .unwrap();
    let (status, _) = mcp(
        &harness.app,
        token,
        json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM domain_event WHERE correlation_id='run-copilot' AND actor_type='agent' AND actor_id=?")
        .bind(&agent.id).fetch_one(harness.state.db.pool()).await.unwrap();
    assert!(
        event_count >= 3,
        "tool and WorkUnit activity must retain Agent attribution"
    );
}
