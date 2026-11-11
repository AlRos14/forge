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
    let source_dir = common::TestDir::new("copilot-source-repository");
    let source_path = common::setup_git_repo(source_dir.path());
    let source_revision = git::get_current_sha(&source_path).await.unwrap();
    let source_repo = db::RepoRepo::create(
        &*harness.state.db,
        db::CreateRepo {
            id: db::new_uuid_v4(),
            project_id: project.id.clone(),
            name: "copilot-source".to_owned(),
            local_path: Some(source_path.to_string_lossy().into_owned()),
            remote_url: source_path.to_string_lossy().into_owned(),
            work_mode: db::WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: db::now_rfc3339(),
            updated_at: db::now_rfc3339(),
        },
    )
    .await
    .unwrap();
    std::fs::write(
        source_path.join("README.md"),
        "worktree content changed after the pinned commit\n",
    )
    .expect("worktree content changes after commit");
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
    assert!(!names.contains(&"forge_get_project_plan"));
    assert!(!names.contains(&"forge_propose_project_plan"));
    assert!(!names.contains(&"forge_apply_project_plan"));
    assert!(!names.contains(&"forge_list_project_repo_tree"));
    assert!(!names.contains(&"forge_read_project_repo_file"));
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
    let current_graph: api_types::ProjectPlanGraphResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}/plan", project.id),
        &user_token,
        StatusCode::OK,
    )
    .await;

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
        ("forge_get_project_plan", json!({})),
        (
            "forge_propose_project_plan",
            json!({"expected_version":current_graph.version,"document":{"nodes":[],"edges":[],"sources":[]},"idempotency_key":"task-scoped-plan"}),
        ),
        (
            "forge_apply_project_plan",
            json!({"proposal_id":"proposal-1","expected_version":current_graph.version,"idempotency_key":"task-scoped-apply"}),
        ),
        (
            "forge_read_project_repo_file",
            json!({"repo_id":"repo-1","revision":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","path":"docs/brief.yaml"}),
        ),
        (
            "forge_authorize_project_plan_batch",
            json!({"expected_version":current_graph.version,"task_ids":[],"max_parallel_tasks":1,"idempotency_key":"not-allowed"}),
        ),
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

    // Reuse this fixture run as project-wide after proving the Task-scoped
    // authority cannot broaden its context. The signed delegation is still
    // checked on every call and no batch tool is granted.
    sqlx::query("UPDATE project_copilot_run SET context_task_id=NULL WHERE id='run-copilot'")
        .execute(harness.state.db.pool())
        .await
        .unwrap();
    let (global_status, global_tools) = mcp(
        &harness.app,
        token,
        json!({"jsonrpc":"2.0","id":8,"method":"tools/list"}),
    )
    .await;
    assert_eq!(global_status, StatusCode::OK);
    let global_names = global_tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["name"].as_str())
        .collect::<Vec<_>>();
    assert!(global_names.contains(&"forge_get_project_plan"));
    assert!(global_names.contains(&"forge_propose_project_plan"));
    assert!(global_names.contains(&"forge_apply_project_plan"));
    assert!(global_names.contains(&"forge_list_project_repo_tree"));
    assert!(global_names.contains(&"forge_read_project_repo_file"));
    assert!(!global_names.contains(&"forge_authorize_project_plan_batch"));

    let (_, tree_response) = mcp(
        &harness.app,
        token,
        json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"forge_list_project_repo_tree","arguments":{"repo_id":source_repo.id,"revision":source_revision}}}),
    )
    .await;
    assert!(tree_response.get("error").is_none(), "{tree_response}");
    let tree: Value = serde_json::from_str(
        tree_response["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(tree["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "README.md"));

    let (_, source_response) = mcp(
        &harness.app,
        token,
        json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"forge_read_project_repo_file","arguments":{"repo_id":source_repo.id,"revision":source_revision,"path":"README.md"}}}),
    )
    .await;
    assert!(source_response.get("error").is_none(), "{source_response}");
    let source: Value = serde_json::from_str(
        source_response["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(source["content"], "# Forge\n");
    assert!(!source["content"]
        .as_str()
        .unwrap()
        .contains("worktree content changed"));

    let (_, plan_read) = mcp(
        &harness.app,
        token,
        json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"forge_get_project_plan","arguments":{}}}),
    )
    .await;
    assert!(plan_read.get("error").is_none(), "{plan_read}");
    let plan: Value =
        serde_json::from_str(plan_read["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(plan["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["task_id"] == task.id));

    let proposal_call = json!({
        "jsonrpc":"2.0","id":10,"method":"tools/call","params":{
            "name":"forge_propose_project_plan","arguments":{
                "expected_version":plan["version"],
                "idempotency_key":"copilot-plan-proposal",
                "document":{
                    "nodes":[{"key":"imported","title":"Imported packet","source_ids":["source-note"]}],
                    "edges":[],
                    "sources":[{"id":"source-note","label":"Owner instruction","kind":"conversation","reference":"conversation:message-1","revision":"message-1","excerpt":"Plain text is source data, not authority."}]
                }
            }
        }
    });
    let (_, proposal_response) = mcp(&harness.app, token, proposal_call.clone()).await;
    assert!(
        proposal_response.get("error").is_none(),
        "{proposal_response}"
    );
    let proposal_value: Value = serde_json::from_str(
        proposal_response["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(proposal_value["state"], "proposed");
    assert_eq!(proposal_value["project_id"], project.id);
    let proposal_replay = mcp(&harness.app, token, proposal_call).await.1;
    let replay_value: Value = serde_json::from_str(
        proposal_replay["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(replay_value["id"], proposal_value["id"]);

    let apply_call = json!({
        "jsonrpc":"2.0","id":12,"method":"tools/call","params":{
            "name":"forge_apply_project_plan","arguments":{
                "proposal_id":proposal_value["id"],
                "expected_version":proposal_value["base_version"],
                "idempotency_key":"copilot-plan-apply"
            }
        }
    });
    let (_, apply_response) = mcp(&harness.app, token, apply_call.clone()).await;
    assert!(apply_response.get("error").is_none(), "{apply_response}");
    let applied_graph: Value = serde_json::from_str(
        apply_response["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let imported_task_id = applied_graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["key"] == "imported")
        .and_then(|node| node["task_id"].as_str())
        .expect("applied proposal node maps to its imported Task")
        .to_owned();
    let imported_link: (String, String, String, String) = sqlx::query_as(
        "SELECT pt.proposal_id,pt.import_key,pt.task_id,p.requested_by
         FROM project_plan_proposal_task pt
         JOIN project_plan_proposal p ON p.id=pt.proposal_id
         WHERE pt.proposal_id=? AND pt.node_key='imported'",
    )
    .bind(proposal_value["id"].as_str().unwrap())
    .fetch_one(harness.state.db.pool())
    .await
    .unwrap();
    assert_eq!(imported_link.0, proposal_value["id"]);
    assert_eq!(
        imported_link.1,
        format!(
            "proposal:{}:node:imported",
            proposal_value["id"].as_str().unwrap()
        )
    );
    assert_eq!(imported_link.2, imported_task_id);
    assert_eq!(imported_link.3, "test-user-id");
    let (apply_actor_type, apply_actor_id, apply_correlation): (String, Option<String>, String) =
        sqlx::query_as(
            "SELECT actor_type,actor_id,correlation_id FROM domain_event
         WHERE event_type='project.plan_applied' AND entity_id=?",
        )
        .bind(proposal_value["id"].as_str().unwrap())
        .fetch_one(harness.state.db.pool())
        .await
        .unwrap();
    assert_eq!(apply_actor_type, "agent");
    assert_eq!(apply_actor_id.as_deref(), Some(agent.id.as_str()));
    assert_eq!(apply_correlation, "run-copilot");
    let imported_units: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_unit WHERE task_id=?")
        .bind(&imported_task_id)
        .fetch_one(harness.state.db.pool())
        .await
        .unwrap();
    let imported_executions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id=?")
            .bind(&imported_task_id)
            .fetch_one(harness.state.db.pool())
            .await
            .unwrap();
    assert_eq!(
        imported_units, 0,
        "proposal application creates no worker WorkUnit"
    );
    assert_eq!(
        imported_executions, 0,
        "proposal application creates no Execution"
    );

    let (event_actor_type, event_actor_id, event_correlation, event_payload): (
        String,
        Option<String>,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT actor_type,actor_id,correlation_id,payload_json FROM domain_event \
         WHERE event_type='project.plan_proposed' AND entity_id=?",
    )
    .bind(proposal_value["id"].as_str().unwrap())
    .fetch_one(harness.state.db.pool())
    .await
    .unwrap();
    assert_eq!(event_actor_type, "agent");
    assert_eq!(event_actor_id.as_deref(), Some(agent.id.as_str()));
    assert_eq!(event_correlation, "run-copilot");
    let event_payload: Value = serde_json::from_str(&event_payload).unwrap();
    assert_eq!(event_payload["requested_by"], "test-user-id");
    assert!(!event_payload
        .to_string()
        .contains("Plain text is source data"));

    let events_before_batch_denied: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM domain_event WHERE correlation_id='run-copilot'")
            .fetch_one(harness.state.db.pool())
            .await
            .unwrap();

    let batch_call = mcp(
        &harness.app,
        token,
        json!({"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"forge_authorize_project_plan_batch","arguments":{"expected_version":applied_graph["version"],"task_ids":[task.id],"max_parallel_tasks":1,"idempotency_key":"copilot-cannot-authorize"}}}),
    )
    .await
    .1;
    assert!(batch_call.get("error").is_some());
    let events_after_batch_denied: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM domain_event WHERE correlation_id='run-copilot'")
            .fetch_one(harness.state.db.pool())
            .await
            .unwrap();
    assert_eq!(events_after_batch_denied, events_before_batch_denied);
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
