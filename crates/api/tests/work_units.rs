mod common;

use api_types::{ExecutionResponse, ProjectResponse, TaskResponse, WorkUnitResponse};
use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

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
            owner_id: None,
            visibility: "global".to_owned(),
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
            owner_id: None,
            visibility: "global".to_owned(),
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
