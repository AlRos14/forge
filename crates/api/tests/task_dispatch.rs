#![allow(dead_code)]

mod common;

use api_types::{
    TaskDispatchActionResponse, TaskLaunchPreflightResponse, TaskLaunchResponse,
    TaskLifecycleState, TaskResponse, WorkUnitResponse,
};
use axum::http::{Method, StatusCode};
use serde_json::json;

use common::{
    create_allocated_work_unit, create_project_and_repo, create_shell_agents, json_request,
    setup_git_repo, test_app, TestDir,
};

#[tokio::test]
async fn task_launch_is_idempotent_and_pause_blocks_new_admissions() {
    let repo_dir = TestDir::new("task-dispatch-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspaces = TestDir::new("task-dispatch-workspaces");
    let harness = test_app(workspaces.path(), "task-dispatch").await;
    let (project_id, _repo_id) =
        create_project_and_repo(&harness.app, "Task dispatch", &repo_path).await;
    let (agent_id, second_agent_id) =
        create_shell_agents(&harness.app, workspaces.path(), "task-dispatch").await;

    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Dispatch exactly once"}),
        StatusCode::OK,
    )
    .await;
    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        json!({"role": "implementer", "coordination_mode": "independent", "policy": {}}),
        StatusCode::OK,
    )
    .await;
    for member in [&agent_id, &second_agent_id] {
        let _: serde_json::Value = json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/tasks/{}/task-roles/implementer/members", task.id),
            json!({"actor_ref": {"kind": "agent", "id": member}}),
            StatusCode::OK,
        )
        .await;
    }

    let first = create_allocated_work_unit(
        &harness.app,
        &task.id,
        "implementer",
        &agent_id,
        "First unit",
        "printf 'first\\n'",
        false,
    )
    .await;
    let second = create_allocated_work_unit(
        &harness.app,
        &task.id,
        "implementer",
        &second_agent_id,
        "Second unit",
        "printf 'second\\n'",
        false,
    )
    .await;
    for unit in [&first, &second] {
        let _: WorkUnitResponse = json_request(
            &harness.app,
            Method::PATCH,
            &format!("/api/v1/work-units/{}", unit.id),
            json!({
                "expected_version": unit.version,
                "title": null,
                "scope": null,
                "execution_instructions": unit.scope,
                "requires_integration": null,
            }),
            StatusCode::OK,
        )
        .await;
    }

    let preflight: TaskLaunchPreflightResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/dispatch", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert!(preflight.can_launch);
    assert_eq!(
        preflight
            .work_units
            .iter()
            .filter(|unit| unit.state == "ready")
            .count(),
        2
    );

    let key = "dispatch-once";
    let launch: TaskLaunchResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/launch", task.id),
        json!({
            "expected_task_version": task.version,
            "expected_lifecycle_version": task.lifecycle.version,
            "idempotency_key": key,
            "reason": null,
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(launch.admitted_execution_ids.len(), 2);

    let replay: TaskLaunchResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/launch", task.id),
        json!({
            "expected_task_version": task.version,
            "expected_lifecycle_version": task.lifecycle.version,
            "idempotency_key": key,
            "reason": null,
        }),
        StatusCode::OK,
    )
    .await;
    assert!(replay.admitted_execution_ids.is_empty());
    assert!(replay.started_execution_ids.is_empty());

    let current: TaskResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let paused: TaskDispatchActionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/pause", task.id),
        json!({
            "expected_task_version": current.version,
            "expected_lifecycle_version": current.lifecycle.version,
            "idempotency_key": "pause-once",
            "reason": "hold new admissions",
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(paused.dispatch.state, api_types::TaskDispatchState::Paused);

    // Replaying the original launch after a later pause must return its exact
    // receipt, not a historical-version lookup failure.
    let replay_after_pause: TaskLaunchResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/launch", task.id),
        json!({
            "expected_task_version": task.version,
            "expected_lifecycle_version": task.lifecycle.version,
            "idempotency_key": key,
            "reason": null,
        }),
        StatusCode::OK,
    )
    .await;
    assert!(replay_after_pause.admitted_execution_ids.is_empty());

    // Let the first two short shell Executions finish so capacity and
    // awaiting-acceptance facts are stable before adding the next unit.
    for _ in 0..100 {
        let executions: api_types::PaginatedResponse<api_types::ExecutionResponse> = json_request(
            &harness.app,
            Method::GET,
            &format!("/api/v1/tasks/{}/executions", task.id),
            json!({}),
            StatusCode::OK,
        )
        .await;
        if executions.items.len() >= 2
            && executions
                .items
                .iter()
                .all(|execution| execution.status != api_types::ExecutionStatus::Running)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let third = create_allocated_work_unit(
        &harness.app,
        &task.id,
        "implementer",
        &second_agent_id,
        "Third unit",
        "printf 'third\\n'",
        false,
    )
    .await;
    let _: WorkUnitResponse = json_request(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/work-units/{}", third.id),
        json!({
            "expected_version": third.version,
            "title": null,
            "scope": null,
            "execution_instructions": third.scope,
            "requires_integration": null,
        }),
        StatusCode::OK,
    )
    .await;
    let rejected = common::raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/launch", task.id),
        json!({
            "expected_task_version": current.version,
            "expected_lifecycle_version": current.lifecycle.version,
            "idempotency_key": "launch-while-paused",
            "reason": null,
        }),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::CONFLICT);

    let resumed: TaskDispatchActionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/resume", task.id),
        json!({
            "expected_task_version": current.version,
            "expected_lifecycle_version": current.lifecycle.version,
            "idempotency_key": "resume-once",
            "reason": null,
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        resumed.dispatch.state,
        api_types::TaskDispatchState::Running
    );

    let third_executions: api_types::PaginatedResponse<api_types::ExecutionResponse> =
        json_request(
            &harness.app,
            Method::GET,
            &format!("/api/v1/tasks/{}/executions", task.id),
            json!({}),
            StatusCode::OK,
        )
        .await;
    assert!(third_executions
        .items
        .iter()
        .any(|execution| execution.work_unit_id.as_deref() == Some(third.id.as_str())));

    let cancel_key = "cancel-once";
    let cancelled: TaskDispatchActionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/cancel", task.id),
        json!({
            "expected_task_version": current.version,
            "expected_lifecycle_version": current.lifecycle.version,
            "idempotency_key": cancel_key,
            "reason": "test cancellation",
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        cancelled.dispatch.state,
        api_types::TaskDispatchState::Cancelled
    );
    let cancel_replay: TaskDispatchActionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/cancel", task.id),
        json!({
            "expected_task_version": current.version,
            "expected_lifecycle_version": current.lifecycle.version,
            "idempotency_key": cancel_key,
            "reason": "test cancellation",
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        cancel_replay.dispatch.state,
        api_types::TaskDispatchState::Cancelled
    );

    let changed_cancel_replay = common::raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/cancel", task.id),
        json!({
            "expected_task_version": current.version,
            "expected_lifecycle_version": current.lifecycle.version,
            "idempotency_key": cancel_key,
            "reason": "different cancellation content",
        }),
    )
    .await;
    assert_eq!(
        changed_cancel_replay.status(),
        StatusCode::CONFLICT,
        "reusing a cancellation key with changed request content conflicts"
    );
}

#[tokio::test]
async fn pause_blocks_a_pending_provider_start_without_claiming_it() {
    let repo_dir = TestDir::new("task-dispatch-pending-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspaces = TestDir::new("task-dispatch-pending-workspaces");
    let harness = test_app(workspaces.path(), "task-dispatch-pending").await;
    let (project_id, _repo_id) =
        create_project_and_repo(&harness.app, "Pending start", &repo_path).await;
    let (agent_id, other_agent_id) =
        create_shell_agents(&harness.app, workspaces.path(), "pending-start").await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Pause pending start"}),
        StatusCode::OK,
    )
    .await;
    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        json!({"role": "implementer", "coordination_mode": "independent", "policy": {}}),
        StatusCode::OK,
    )
    .await;
    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles/implementer/members", task.id),
        json!({"actor_ref": {"kind": "agent", "id": agent_id}}),
        StatusCode::OK,
    )
    .await;
    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles/implementer/members", task.id),
        json!({"actor_ref": {"kind": "agent", "id": other_agent_id}}),
        StatusCode::OK,
    )
    .await;
    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles/implementer/members", task.id),
        json!({"actor_ref": {"kind": "human", "id": "test-user-id"}}),
        StatusCode::OK,
    )
    .await;
    let _human_unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Human pending unit",
            "scope": "Pending human work",
            "role": "implementer",
            "parent_work_unit_id": null,
            "assigned_actor": {"kind": "human", "id": "test-user-id"},
            "requires_integration": false,
            "provenance": null,
        }),
        StatusCode::CREATED,
    )
    .await;
    let launch: TaskLaunchResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/launch", task.id),
        json!({
            "expected_task_version": task.version,
            "expected_lifecycle_version": task.lifecycle.version,
            "idempotency_key": "launch-before-pending-start",
            "reason": null,
        }),
        StatusCode::OK,
    )
    .await;
    assert!(launch.admitted_execution_ids.is_empty());

    let unit = create_allocated_work_unit(
        &harness.app,
        &task.id,
        "implementer",
        &agent_id,
        "Pending unit",
        "printf 'pending\\n'",
        false,
    )
    .await;
    let unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/work-units/{}", unit.id),
        json!({
            "expected_version": unit.version,
            "title": null,
            "scope": null,
            "execution_instructions": "printf 'pending\\n'",
            "requires_integration": null,
        }),
        StatusCode::OK,
    )
    .await;

    let launched = harness
        .state
        .task_service
        .launch_work_unit_execution(
            &unit.id,
            unit.version,
            "printf 'pending\\n'".to_owned(),
            Vec::new(),
            None,
        )
        .await
        .expect("Execution admits with a pending provider Start");
    let current: TaskResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let _: TaskDispatchActionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/pause", task.id),
        json!({
            "expected_task_version": current.version,
            "expected_lifecycle_version": current.lifecycle.version,
            "idempotency_key": "pause-pending-start",
            "reason": "hold provider starts",
        }),
        StatusCode::OK,
    )
    .await;
    let start = harness
        .state
        .task_service
        .start_execution(launched.execution.id.clone())
        .await
        .expect("paused pending start returns without an external effect");
    assert!(!start.accepted);

    let paused_unit = create_allocated_work_unit(
        &harness.app,
        &task.id,
        "implementer",
        &other_agent_id,
        "Blocked by pause",
        "printf 'blocked\\n'",
        false,
    )
    .await;
    let admission_error = match harness
        .state
        .task_service
        .launch_work_unit_execution(
            &paused_unit.id,
            paused_unit.version,
            "printf 'blocked\\n'".to_owned(),
            Vec::new(),
            None,
        )
        .await
    {
        Ok(_) => panic!("paused Task accepted a new automatic admission"),
        Err(error) => error,
    };
    assert!(admission_error.to_string().contains("paused"));
    let attempted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.provider_start_attempted' AND entity_id = ?",
    )
    .bind(&launched.execution.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("provider-start receipt count");
    assert_eq!(attempted, 0);
}

#[tokio::test]
async fn human_assigned_work_unit_is_pending_without_a_fake_execution() {
    let repo_dir = TestDir::new("task-dispatch-human-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspaces = TestDir::new("task-dispatch-human-workspaces");
    let harness = test_app(workspaces.path(), "task-dispatch-human").await;
    let (project_id, _repo_id) =
        create_project_and_repo(&harness.app, "Human dispatch", &repo_path).await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Human work"}),
        StatusCode::OK,
    )
    .await;
    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        json!({"role": "implementer", "coordination_mode": "independent", "policy": {}}),
        StatusCode::OK,
    )
    .await;
    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles/implementer/members", task.id),
        json!({"actor_ref": {"kind": "human", "id": "test-user-id"}}),
        StatusCode::OK,
    )
    .await;
    let unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Human review packet",
            "scope": "Prepare the exact human deliverable",
            "execution_instructions": null,
            "role": "implementer",
            "parent_work_unit_id": null,
            "assigned_actor": {"kind": "human", "id": "test-user-id"},
            "requires_integration": false,
            "provenance": null,
        }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(unit.status, api_types::WorkUnitStatus::Open);

    let launch: TaskLaunchResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/dispatch/launch", task.id),
        json!({
            "expected_task_version": task.version,
            "expected_lifecycle_version": task.lifecycle.version,
            "idempotency_key": "human-launch",
            "reason": null,
        }),
        StatusCode::OK,
    )
    .await;
    assert!(launch.admitted_execution_ids.is_empty());
    assert!(launch
        .preflight
        .work_units
        .iter()
        .any(|work_unit| work_unit.state == "waiting_human"));
    let executions: api_types::PaginatedResponse<api_types::ExecutionResponse> = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/executions", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert!(executions.items.is_empty());
    assert_eq!(launch.preflight.lifecycle.state, TaskLifecycleState::Ready);
}
