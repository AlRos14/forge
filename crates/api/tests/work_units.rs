mod common;

use api_types::{
    AgentResponse, ExecutionResponse, ProjectResponse, TaskResponse, WorkUnitResponse,
};
use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn work_unit_api_creates_lists_and_rejects_cross_task_parent() {
    let repo_dir = TestDir::new("pr5-work-unit-api-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspace_root = TestDir::new("pr5-work-unit-api-workspaces");
    let harness = test_app(workspace_root.path(), "pr5-work-unit-api").await;
    let (project_id, _) = create_project_and_repo(&harness.app, "PR5 WorkUnits", &repo_path).await;

    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({ "title": "WorkUnit API scope" }),
        StatusCode::OK,
    )
    .await;
    let other_task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({ "title": "Other WorkUnit API scope" }),
        StatusCode::OK,
    )
    .await;

    let role = format!("pr5-{}", uuid::Uuid::new_v4());
    let now = db::now_rfc3339();
    db::TaskRoleRepo::create(
        &*harness.state.db,
        db::CreateTaskRole {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: task.id.clone(),
            role: role.clone(),
            coordination_mode: None,
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("TaskRole exists for WorkUnit allocation context");

    let unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Implement isolated scope",
            "scope": "Implement the endpoint and focused persistence",
            "role": role.clone(),
            "parent_work_unit_id": null,
            "assigned_actor": null,
            "requires_integration": true,
            "provenance": null
        }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(unit.task_id, task.id);
    assert_eq!(unit.status, api_types::WorkUnitStatus::Open);
    assert!(unit.readiness.runnable);
    assert!(unit.readiness.ready_for_allocation);

    let units: Vec<WorkUnitResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].id, unit.id);

    let cross_task_parent = raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", other_task.id),
        json!({
            "title": "Cross Task parent must be hidden",
            "scope": "Cannot resolve parent in another Task",
            "role": role.clone(),
            "parent_work_unit_id": unit.id,
            "assigned_actor": null,
            "requires_integration": true,
            "provenance": null
        }),
    )
    .await;
    assert_eq!(cross_task_parent.status(), StatusCode::NOT_FOUND);

    let unknown_field = raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Reject unknown fields",
            "scope": "Strict request DTO",
            "role": role,
            "parent_work_unit_id": null,
            "assigned_actor": null,
            "requires_integration": true,
            "provenance": null,
            "workspace_path": "/tmp/forbidden"
        }),
    )
    .await;
    assert_eq!(unknown_field.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn non_repository_agent_work_unit_starts_through_task_executions_rest_route() {
    let workspace_root = TestDir::new("pr13-rest-non-repository-context");
    let harness = test_app(workspace_root.path(), "pr13-rest-non-repository-context").await;
    let project: ProjectResponse = json_request(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        json!({ "name": "PR13 non-repository REST" }),
        StatusCode::OK,
    )
    .await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        json!({ "title": "REST isolated execution" }),
        StatusCode::OK,
    )
    .await;

    let now = db::now_rfc3339();
    let daemon_id = uuid::Uuid::new_v4().to_string();
    db::DaemonRepo::upsert_by_machine_id(
        &*harness.state.db,
        db::UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: services::embedded_daemon::embedded_machine_id(),
            hostname: "pr13-rest-embedded".to_owned(),
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
    .expect("embedded execution host is recorded");
    db::DaemonRepo::update_report(
        &*harness.state.db,
        db::UpdateDaemonReport {
            id: daemon_id.clone(),
            detected_clis_json:
                r#"[{"kind":"shell","availability":"authenticated","path":"/bin/sh"}]"#.to_owned(),
            labels_json: None,
            status: db::DaemonStatus::Online,
            last_report_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("embedded Shell adapter is available");

    let agent_id = uuid::Uuid::new_v4().to_string();
    db::AgentRepo::create(
        &*harness.state.db,
        db::CreateAgent {
            id: agent_id.clone(),
            name: "PR13 REST isolated Agent".to_owned(),
            description: None,
            executor_type: "shell".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some(daemon_id),
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: db::AgentStatus::Idle,
            last_heartbeat_at: Some(now.clone()),
            is_default: false,
            paused: false,
            owner_id: Some("test-user-id".to_owned()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Agent is routed to the embedded host");
    harness
        .state
        .task_service
        .create_task_role(
            &task.id,
            "implementer",
            db::CoordinationMode::Independent,
            "{}".to_owned(),
        )
        .await
        .expect("TaskRole creates");
    harness
        .state
        .task_service
        .add_task_role_member(
            &task.id,
            "implementer",
            api_types::ActorRef::Agent(agent_id.clone()),
        )
        .await
        .expect("Agent joins its exact TaskRole");

    let work_unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Isolated REST WorkUnit",
            "scope": "Write one marker in its isolated context",
            "role": "implementer",
            "parent_work_unit_id": null,
            "assigned_actor": api_types::ActorRef::Agent(agent_id.clone()),
            "requires_integration": false,
            "provenance": null
        }),
        StatusCode::CREATED,
    )
    .await;

    let execution: ExecutionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/executions", task.id),
        json!({
            "work_unit_id": work_unit.id,
            "expected_work_unit_version": work_unit.version,
            "purpose": "implement",
            "prompt": "printf 'rest-ok\\n' > rest-result.txt",
            "input_artifact_ids": [],
            "overrides": null
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        execution.actor_ref,
        Some(api_types::ActorRef::Agent(agent_id))
    );
    assert!(execution.workspace_id.is_none());

    let expected_context = workspace_root
        .path()
        .join(".forge-workunit-context")
        .join(&task.id)
        .join(&work_unit.id);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let completed = loop {
        let current = db::ExecutionRepo::get_by_id(&*harness.state.db, &execution.id)
            .await
            .expect("Execution reloads")
            .expect("Execution exists");
        if current.status == db::ExecutionStatus::Completed {
            break current;
        }
        assert_ne!(
            current.status,
            db::ExecutionStatus::Failed,
            "REST-launched isolated Execution must complete"
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "Execution completes"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    assert_eq!(completed.status, db::ExecutionStatus::Completed);
    assert_eq!(
        std::fs::read_to_string(expected_context.join("rest-result.txt"))
            .expect("marker is written inside the WorkUnit context"),
        "rest-ok\n"
    );
    let leases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspace_lease WHERE execution_id = ? OR work_unit_id = ?",
    )
    .bind(&execution.id)
    .bind(&work_unit.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("non-repository WorkspaceLease count loads");
    assert_eq!(leases, 0);
}

#[tokio::test]
async fn rest_and_mcp_cannot_route_an_account_to_another_accounts_embedded_shell() {
    let workspace_root = TestDir::new("f01-cross-account-shell");
    let harness = test_app(workspace_root.path(), "f01-cross-account-shell").await;
    let now = db::now_rfc3339();
    let foreign_user = "account-b";
    db::UserRepo::create_user(
        &*harness.state.db,
        &db::User {
            id: foreign_user.to_owned(),
            email: "account-b@security.test".to_owned(),
            password_hash: "x".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("foreign account creates");
    let bearer = common::test_jwt_for_user(foreign_user, "account-b@security.test", false);

    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &bearer,
        json!({ "name": "Account B shell isolation" }),
        StatusCode::OK,
    )
    .await;
    let task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &bearer,
        json!({ "title": "Account B shell isolation" }),
        StatusCode::OK,
    )
    .await;
    let ready: api_types::TaskLifecycleTransitionResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        &bearer,
        json!({ "to_state":"ready", "expected_lifecycle_version":task.lifecycle.version,
            "idempotency_key":"f01-cross-account-ready", "reason_kind":"user_request", "reason_ref":"f01" }),
        StatusCode::OK,
    )
    .await;
    let _: api_types::TaskLifecycleTransitionResponse = common::json_request_with_bearer(
        &harness.app, Method::POST, &format!("/api/v1/tasks/{}/lifecycle", task.id), &bearer,
        json!({ "to_state":"active", "expected_lifecycle_version":ready.lifecycle.version,
            "idempotency_key":"f01-cross-account-start", "reason_kind":"user_request", "reason_ref":"f01" }),
        StatusCode::OK,
    ).await;

    let daemon_id = uuid::Uuid::new_v4().to_string();
    db::DaemonRepo::upsert_by_machine_id(
        &*harness.state.db,
        db::UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: services::embedded_daemon::embedded_machine_id(),
            hostname: "account-a-local".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            agent_version: Some("test".to_owned()),
            labels_json: "{}".to_owned(),
            status: db::DaemonStatus::Online,
            registration_token_hash: None,
            owner_id: Some("test-user-id".to_owned()),
            visibility: "global".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Account A embedded shell is seeded");
    db::DaemonRepo::update_report(
        &*harness.state.db,
        db::UpdateDaemonReport {
            id: daemon_id.clone(),
            detected_clis_json:
                r#"[{"kind":"shell","availability":"authenticated","path":"/bin/sh"}]"#.to_owned(),
            labels_json: None,
            status: db::DaemonStatus::Online,
            last_report_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("authenticated shell capability is seeded");

    let owner_bearer = common::admin_jwt();
    let owner_agent: AgentResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/agents",
        &owner_bearer,
        json!({ "name": "account-a-own-shell", "executor_type": "shell", "daemon_id": daemon_id }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(owner_agent.effective_status.as_deref(), Some("active"));
    let provider_entries = common::raw_json_request_with_bearer(
        &harness.app,
        Method::GET,
        "/api/v1/providers",
        &bearer,
        serde_json::Value::Null,
    )
    .await;
    assert!(provider_entries.status().is_success());
    let provider_entries = axum::body::to_bytes(provider_entries.into_body(), usize::MAX)
        .await
        .unwrap();
    let provider_entries: serde_json::Value = serde_json::from_slice(&provider_entries).unwrap();
    assert!(
        provider_entries["cli_runtimes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|runtime| runtime["daemon_id"] != daemon_id),
        "provider discovery does not announce Account A's daemon to B"
    );
    let explicit_foreign_binding = common::raw_json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/agents",
        &bearer,
        json!({ "name": "account-b-foreign-shell", "executor_type": "shell", "daemon_id": daemon_id }),
    )
    .await;
    assert!(
        !explicit_foreign_binding.status().is_success(),
        "REST refuses to pin B's Agent to A's private daemon"
    );
    let agent: AgentResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/agents",
        &bearer,
        json!({ "name": "account-b-unpinned-shell", "executor_type": "shell" }),
        StatusCode::OK,
    )
    .await;
    let foreign_profile_id = uuid::Uuid::new_v4().to_string();
    db::AgentProfileRepo::create_profile(
        &*harness.state.db,
        db::CreateAgentProfile {
            id: foreign_profile_id.clone(),
            identity_id: agent.id.clone(),
            backend_kind: "cli".to_owned(),
            executor_type: "shell".to_owned(),
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            tool_policy_json: "{}".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some(daemon_id.clone()),
            created_at: db::now_rfc3339(),
            updated_at: db::now_rfc3339(),
        },
    )
    .await
    .expect("foreign-pinned historical profile fixture creates");
    let profile_selection = common::raw_json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!(
            "/api/v1/agents/{}/profiles/{foreign_profile_id}/select",
            agent.id
        ),
        &bearer,
        json!({ "version": agent.version }),
    )
    .await;
    assert!(
        !profile_selection.status().is_success(),
        "profile selection cannot bind B's Agent to A's daemon"
    );
    let elevated_claim = common::test_jwt_for_user(foreign_user, "account-b@security.test", true);
    let explicit_foreign_update = common::raw_json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/agents/{}", agent.id),
        &elevated_claim,
        json!({ "daemon_id": daemon_id, "version": agent.version }),
    )
    .await;
    assert!(
        !explicit_foreign_update.status().is_success(),
        "an elevated token claim cannot bind B's Agent to A's private daemon"
    );
    let marker = workspace_root.path().join("must-not-exist");
    let prompt = format!("printf compromised > {}", marker.display());
    harness
        .state
        .task_service
        .create_task_role(
            &task.id,
            "implementer",
            db::CoordinationMode::Independent,
            "{}".to_owned(),
        )
        .await
        .expect("TaskRole creates");
    harness
        .state
        .task_service
        .add_task_role_member(
            &task.id,
            "implementer",
            api_types::ActorRef::Agent(agent.id.clone()),
        )
        .await
        .expect("Account B Agent joins its own TaskRole");
    let work_unit = harness
        .state
        .work_unit_service
        .create(
            services::CollaborationActorSource::Human(foreign_user.to_owned()),
            services::CreateWorkUnitInput {
                task_id: task.id.clone(),
                title: "Try cross-account shell".to_owned(),
                scope: "Attempt shell marker execution".to_owned(),
                role: "implementer".to_owned(),
                parent_work_unit_id: None,
                assigned_actor: Some(db::ActorRef::Agent(agent.id.clone())),
                requires_integration: false,
                provenance: None,
            },
        )
        .await
        .expect("WorkUnit created for B's own Agent");

    let discovery: api_types::DiscoveredOptionsResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        "/api/v1/executor-types/shell/discovered-options",
        &bearer,
        StatusCode::OK,
    )
    .await;
    assert!(
        discovery.available_daemons.is_empty(),
        "Account A daemon is not advertised to B"
    );

    let request = json!({ "work_unit_id": work_unit.id, "expected_work_unit_version": work_unit.version,
        "purpose": "implement", "prompt": prompt, "input_artifact_ids": [], "overrides": null });
    let rest = common::raw_json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/executions", task.id),
        &bearer,
        request.clone(),
    )
    .await;
    assert!(
        !rest.status().is_success(),
        "REST admission rejects B's unpinned Agent"
    );
    assert!(
        !marker.exists(),
        "REST rejection leaves the external shell marker absent"
    );

    let mcp = harness.app.clone().oneshot(
        axum::http::Request::builder().method(Method::POST).uri(format!("/mcp?project_id={}", project.id))
            .header(axum::http::header::AUTHORIZATION, format!("Bearer {bearer}"))
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(json!({ "jsonrpc":"2.0", "id":7, "method":"tools/call", "params": {
                "name":"forge_start_execution", "arguments": {
                    "work_unit_id":work_unit.id, "expected_work_unit_version":work_unit.version,
                    "purpose":"implement", "prompt":prompt, "input_artifact_ids":[]
                }
            }}).to_string())).unwrap()
    ).await.expect("MCP response");
    assert!(
        mcp.status().is_success(),
        "MCP returns a JSON-RPC tool result"
    );
    let body = axum::body::to_bytes(mcp.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        value.get("error").is_some(),
        "MCP rejects the unauthorized Provider Start: {value}"
    );
    assert!(
        !marker.exists(),
        "MCP rejection leaves the external shell marker absent"
    );
    let executions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
        .bind(&task.id)
        .fetch_one(harness.state.db.pool())
        .await
        .unwrap();
    assert_eq!(
        executions, 0,
        "neither public route admits an unauthorized Execution"
    );
}
