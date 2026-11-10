#![allow(dead_code)]

mod common;

use std::time::Duration;

use api_types::{
    ActorRef, ExecutionResponse, ExecutionStatus, PaginatedResponse, RequestReworkRequest,
    RequestReworkResponse, ReviewExecutionResponse, ReviewLineResponse, ReviewReportVerdict,
    StartReviewExecutionRequest, SubmitReviewReportRequest, SubmitReviewReportResponse,
    TaskResponse, TaskReworkAutomationIntentsResponse, WorkUnitResponse,
};
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use common::{
    create_allocated_work_unit, create_project_and_repo, create_shell_agents, json_request,
    setup_git_repo, test_app, TestDir,
};

async fn wait_for_execution_completed(app: &axum::Router, task_id: &str, execution_id: &str) {
    for _ in 0..200 {
        let executions: PaginatedResponse<ExecutionResponse> = json_request(
            app,
            Method::GET,
            &format!("/api/v1/tasks/{task_id}/executions"),
            json!({}),
            StatusCode::OK,
        )
        .await;
        if executions.items.iter().any(|execution| {
            execution.id == execution_id && execution.status == ExecutionStatus::Completed
        }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Execution {execution_id} did not complete");
}

async fn transition_dispatch_for_test(
    harness: &common::Harness,
    task_id: &str,
    operation: db::TaskDispatchOperation,
    state: db::TaskDispatchState,
    idempotency_key: &str,
) {
    use db::{TaskLifecycleRepo, TransitionTaskDispatch};
    let dispatch = TaskLifecycleRepo::get_task_dispatch(&*harness.state.db, task_id)
        .await
        .expect("Task dispatch loads")
        .expect("Task dispatch exists");
    let now = db::now_rfc3339();
    TaskLifecycleRepo::transition_task_dispatch(
        &*harness.state.db,
        TransitionTaskDispatch {
            task_id: task_id.to_owned(),
            operation,
            state,
            expected_intent_version: dispatch.version,
            requested_by_kind: "human".to_owned(),
            requested_by_id: Some("test-user-id".to_owned()),
            reason: None,
            idempotency_key: idempotency_key.to_owned(),
            request_digest: format!("{:0<64}", idempotency_key),
            receipt_id: db::new_uuid_v4(),
            created_at: now,
        },
    )
    .await
    .expect("Task dispatch transition persists");
}

#[tokio::test]
async fn formal_review_uses_explicit_custom_task_role_capability() {
    let repo_dir = TestDir::new("role-capability-custom-review-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspaces = TestDir::new("role-capability-custom-review-workspaces");
    let harness = test_app(workspaces.path(), "role-capability-custom-review").await;
    let (project_id, _) =
        create_project_and_repo(&harness.app, "Custom review role", &repo_path).await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Custom formal review"}),
        StatusCode::OK,
    )
    .await;
    let named_reviewer: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        json!({
            "role": "reviewer",
            "coordination_mode": "independent",
            "configuration": {},
            "policy": {}
        }),
        StatusCode::OK,
    )
    .await;
    let named_reviewer_id = named_reviewer["id"]
        .as_str()
        .expect("named reviewer TaskRole ID")
        .to_owned();
    let _: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles/reviewer/members", task.id),
        json!({"actor_ref": {"kind": "human", "id": "test-user-id"}}),
        StatusCode::OK,
    )
    .await;
    let named_reviewer_denied = common::raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/review-executions", task.id),
        serde_json::to_value(StartReviewExecutionRequest {
            task_role_id: named_reviewer_id,
            workspace_id: None,
        })
        .expect("named review start serializes"),
    )
    .await;
    assert!(!named_reviewer_denied.status().is_success());
    let role: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        json!({
            "role": "planning-reviewer",
            "coordination_mode": "independent",
            "configuration": {},
            "policy": {}
        }),
        StatusCode::OK,
    )
    .await;
    let role_id = role["id"].as_str().expect("custom TaskRole ID").to_owned();
    let _: Value = json_request(
        &harness.app,
        Method::POST,
        &format!(
            "/api/v1/tasks/{}/task-roles/planning-reviewer/members",
            task.id
        ),
        json!({"actor_ref": {"kind": "human", "id": "test-user-id"}}),
        StatusCode::OK,
    )
    .await;
    let request = serde_json::to_value(StartReviewExecutionRequest {
        task_role_id: role_id.clone(),
        workspace_id: None,
    })
    .expect("review start request serializes");
    let denied = common::raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/review-executions", task.id),
        request,
    )
    .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let current_roles: Value = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let current_role_version = current_roles
        .as_array()
        .and_then(|roles| roles.iter().find(|role| role["id"] == role_id))
        .and_then(|role| role["version"].as_i64())
        .expect("current custom TaskRole version");

    let _: Value = json_request(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/tasks/{}/task-roles/planning-reviewer", task.id),
        json!({
            "expected_version": current_role_version,
            "configuration": {
                "schema_version": 1,
                "display_label": "Planning reviewer",
                "capabilities": ["formal_review"],
                "rework": null
            }
        }),
        StatusCode::OK,
    )
    .await;
    let current_task: TaskResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let _: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        json!({
            "to_state": "active",
            "expected_lifecycle_version": current_task.lifecycle.version,
            "idempotency_key": "capability-review-test-start",
            "reason_kind": "user_request",
            "reason_ref": "explicit-formal-review-test"
        }),
        StatusCode::OK,
    )
    .await;
    let started: ReviewExecutionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/review-executions", task.id),
        serde_json::to_value(StartReviewExecutionRequest {
            task_role_id: role_id,
            workspace_id: None,
        })
        .expect("review start request serializes"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(started.execution.role, "planning-reviewer");
}

async fn enable_manual_rework_routing(app: &axum::Router, task_id: &str) -> (String, String) {
    let roles: Value = json_request(
        app,
        Method::GET,
        &format!("/api/v1/tasks/{task_id}/task-roles"),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let role = |name: &str| {
        roles
            .as_array()
            .and_then(|roles| roles.iter().find(|role| role["role"] == name))
            .expect("configured TaskRole exists")
    };
    let implementer = role("implementer");
    let reviewer = role("reviewer");
    let implementer_id = implementer["id"].as_str().expect("role ID").to_owned();
    let reviewer_id = reviewer["id"].as_str().expect("role ID").to_owned();
    let _: Value = json_request(
        app,
        Method::PATCH,
        &format!("/api/v1/tasks/{task_id}/task-roles/reviewer"),
        json!({
            "expected_version": reviewer["version"],
            "configuration": {
                "schema_version": 1,
                "display_label": "Revisor técnico",
                "capabilities": ["formal_review", "request_rework"],
                "rework": {
                    "mode": "manual",
                    "allowed_destination_role_ids": [implementer_id],
                    "next_review_role_id": reviewer_id,
                    "correction_actor": null,
                    "review_actor": null
                }
            }
        }),
        StatusCode::OK,
    )
    .await;
    (implementer_id, reviewer_id)
}

#[tokio::test]
async fn repository_correction_workspace_uses_the_reviewed_result_commit() {
    let repo_dir = TestDir::new("rework-repo-base");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspaces = TestDir::new("rework-repo-base-workspaces");
    let harness = test_app(workspaces.path(), "rework-repo-base").await;
    let (project_id, _repo_id) =
        create_project_and_repo(&harness.app, "Repository rework", &repo_path).await;
    let (agent_id, _) = create_shell_agents(&harness.app, workspaces.path(), "rework-repo").await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Repository correction base"}),
        StatusCode::OK,
    )
    .await;
    for (role, actor) in [
        ("implementer", json!({"kind": "agent", "id": agent_id})),
        ("reviewer", json!({"kind": "human", "id": "test-user-id"})),
    ] {
        let _: Value = json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/tasks/{}/task-roles", task.id),
            json!({
                "role": role,
                "coordination_mode": "independent",
                "policy": {},
                "configuration": if role == "reviewer" {
                    json!({"capabilities":["formal_review"]})
                } else {
                    json!({})
                }
            }),
            StatusCode::OK,
        )
        .await;
        let _: Value = json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/tasks/{}/task-roles/{role}/members", task.id),
            json!({"actor_ref": actor}),
            StatusCode::OK,
        )
        .await;
    }
    let (implementer_role_id, _reviewer_role_id) =
        enable_manual_rework_routing(&harness.app, &task.id).await;
    let unit = create_allocated_work_unit(
        &harness.app,
        &task.id,
        "implementer",
        &agent_id,
        "Repository implementation",
        "printf 'base\\n' > repo-result.txt && git add repo-result.txt && git commit -m 'repo result'",
        true,
    )
    .await;
    let execution = common::launch_work_unit_execution(
        &harness.app,
        &task.id,
        &unit,
        Some(
            "printf 'base\\n' > repo-result.txt && git add repo-result.txt && git commit -m 'repo result'",
        ),
    )
    .await;
    wait_for_execution_completed(&harness.app, &task.id, &execution.id).await;
    let execution_row = db::ExecutionRepo::get_by_id(&*harness.state.db, &execution.id)
        .await
        .expect("Execution loads")
        .expect("Execution exists");
    let reviewed_commit = execution_row
        .after_sha
        .clone()
        .expect("repository Execution has a result commit");
    let unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", unit.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let review: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title":"Review I1","scope":"Review the exact implementation","role":"reviewer",
            "assigned_actor":{"kind":"human","id":"test-user-id"},"requires_integration":false,
            "review_subject":{"reviewed_work_unit_id":unit.id}
        }),
        StatusCode::CREATED,
    )
    .await;
    let current_unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", unit.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let _: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/status", unit.id),
        json!({"expected_version": current_unit.version, "status": "completed"}),
        StatusCode::OK,
    )
    .await;
    let submitted: SubmitReviewReportResponse = json_request(&harness.app,Method::POST,
        &format!("/api/v1/work-units/{}/review-report",review.id),json!({
            "expected_version":review.version,
            "report":{"verdict":"request_changes","summary":"Apply correction","criteria":["acceptance"],"findings":["missing correction"]}
        }),StatusCode::OK).await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let rework: RequestReworkResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/rework", task.id),
        serde_json::to_value(RequestReworkRequest {
            expected_task_version: task.version,
            expected_lifecycle_version: task.lifecycle.version,
            review_report_artifact_id: submitted.report.id,
            reviewed_work_unit_id: unit.id.clone(),
            reviewed_work_unit_version: unit.version,
            correction_title: "Repository correction".to_owned(),
            correction_scope: "Correct the reviewed repository result".to_owned(),
            correction_role_id: implementer_role_id,
            correction_actor: Some(ActorRef::Agent(agent_id)),
            review_title: "Repository review".to_owned(),
            review_scope: "Review the exact corrected repository result".to_owned(),
            review_actor: Some(ActorRef::Human("test-user-id".to_owned())),
            idempotency_key: "repo-rework-round-1".to_owned(),
        })
        .expect("rework serializes"),
        StatusCode::OK,
    )
    .await;
    let workspace: api_types::WorkspaceResponse = json_request(
        &harness.app,
        Method::POST,
        &format!(
            "/api/v1/work-units/{}/workspace",
            rework.correction_work_unit.id
        ),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        workspace.before_sha.as_deref(),
        Some(reviewed_commit.as_str())
    );
    let dependency_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM work_unit_dependency WHERE work_unit_id = ?")
            .bind(&rework.review_work_unit.id)
            .fetch_one(harness.state.db.pool())
            .await
            .expect("review dependency count");
    assert_eq!(
        dependency_count, 0,
        "review must not require correction integration"
    );
}

#[tokio::test]
async fn request_changes_creates_replay_safe_correction_and_review_round() {
    let repo_dir = TestDir::new("rework-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspaces = TestDir::new("rework-workspaces");
    let harness = test_app(workspaces.path(), "rework").await;
    let (project_id, _repo_id) = create_project_and_repo(&harness.app, "Rework", &repo_path).await;
    let (agent_id, other_agent_id) =
        create_shell_agents(&harness.app, workspaces.path(), "rework").await;

    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Rework the delivered result"}),
        StatusCode::OK,
    )
    .await;
    for (role, actor) in [
        ("implementer", json!({"kind": "agent", "id": agent_id})),
        ("reviewer", json!({"kind": "human", "id": "test-user-id"})),
    ] {
        let _: Value = json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/tasks/{}/task-roles", task.id),
            json!({
                "role": role,
                "coordination_mode": "independent",
                "policy": {},
                "configuration": if role == "reviewer" {
                    json!({"capabilities":["formal_review"]})
                } else {
                    json!({})
                }
            }),
            StatusCode::OK,
        )
        .await;
        let _: Value = json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/tasks/{}/task-roles/{role}/members", task.id),
            json!({"actor_ref": actor}),
            StatusCode::OK,
        )
        .await;
    }
    let (implementer_role_id, _reviewer_role_id) =
        enable_manual_rework_routing(&harness.app, &task.id).await;
    let _: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles/reviewer/members", task.id),
        json!({"actor_ref": {"kind": "agent", "id": agent_id}}),
        StatusCode::OK,
    )
    .await;
    let unit = create_allocated_work_unit(
        &harness.app,
        &task.id,
        "implementer",
        &agent_id,
        "Initial implementation",
        "printf 'initial\\n'",
        false,
    )
    .await;
    let execution = common::launch_work_unit_execution(
        &harness.app,
        &task.id,
        &unit,
        Some("printf 'initial\\n'"),
    )
    .await;
    wait_for_execution_completed(&harness.app, &task.id, &execution.id).await;
    let unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", unit.id),
        json!({}),
        StatusCode::OK,
    )
    .await;

    let review: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title":"Review I1","scope":"Review the exact implementation","role":"reviewer",
            "assigned_actor":{"kind":"human","id":"test-user-id"},"requires_integration":false,
            "review_subject":{"reviewed_work_unit_id":unit.id}
        }),
        StatusCode::CREATED,
    )
    .await;
    let current_unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", unit.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let completed_unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/status", unit.id),
        json!({"expected_version": current_unit.version, "status": "completed"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(completed_unit.version, unit.version + 1);
    let submitted: SubmitReviewReportResponse = json_request(&harness.app,Method::POST,
        &format!("/api/v1/work-units/{}/review-report",review.id),json!({
            "expected_version":review.version,
            "report":{"verdict":"request_changes","summary":"Apply correction","criteria":["acceptance"],"findings":["missing correction"]}
        }),StatusCode::OK).await;
    let report = submitted.report;
    let report_id = report.id.clone();
    let initial_lines: Vec<ReviewLineResponse> = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/review-lines", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let initial_line = initial_lines
        .iter()
        .find(|line| line.root_work_unit_id == unit.id)
        .expect("initial exact result has a durable review line");
    assert_eq!(
        initial_line.latest_verdict.as_deref(),
        Some("request_changes")
    );
    assert_eq!(
        initial_line.latest_report_artifact_id.as_deref(),
        Some(report_id.as_str())
    );
    let _ = other_agent_id;

    let task: TaskResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let request = RequestReworkRequest {
        expected_task_version: task.version,
        expected_lifecycle_version: task.lifecycle.version,
        review_report_artifact_id: report.id,
        reviewed_work_unit_id: unit.id.clone(),
        reviewed_work_unit_version: unit.version,
        correction_title: "Correction C1".to_owned(),
        correction_scope: "Apply exactly the requested correction".to_owned(),
        correction_role_id: implementer_role_id,
        correction_actor: Some(ActorRef::Agent(agent_id.clone())),
        review_title: "Review R2".to_owned(),
        review_scope: "Review the exact correction result".to_owned(),
        review_actor: Some(ActorRef::Human("test-user-id".to_owned())),
        idempotency_key: "rework-round-1".to_owned(),
    };
    let first: RequestReworkResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/rework", task.id),
        serde_json::to_value(&request).expect("rework serializes"),
        StatusCode::OK,
    )
    .await;
    let advanced_lines: Vec<ReviewLineResponse> = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/review-lines", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let advanced_line = advanced_lines
        .iter()
        .find(|line| line.root_work_unit_id == unit.id)
        .expect("rework advances the existing review line");
    assert_eq!(
        advanced_line.current_work_unit_id,
        first.correction_work_unit.id
    );
    assert_eq!(advanced_line.latest_verdict, None);
    assert!(advanced_line.revision > initial_line.revision);
    assert!(advanced_line.history.iter().any(|revision| {
        revision.review_report_artifact_id.as_deref() == Some(report_id.as_str())
            && revision.verdict.as_deref() == Some("request_changes")
    }));
    assert_eq!(
        first.correction_work_unit.status,
        api_types::WorkUnitStatus::Open
    );
    assert_eq!(
        first.review_work_unit.status,
        api_types::WorkUnitStatus::Open
    );
    assert_eq!(first.rounds_remaining, 15);
    assert!(!first.replayed);
    assert_eq!(
        first
            .review_subject
            .as_ref()
            .map(|subject| subject.reviewed_work_unit_id.as_str()),
        Some(first.correction_work_unit.id.as_str())
    );

    let replay: RequestReworkResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/rework", task.id),
        serde_json::to_value(&request).expect("rework serializes"),
        StatusCode::OK,
    )
    .await;
    assert!(replay.replayed);
    assert_eq!(
        replay.correction_work_unit.id,
        first.correction_work_unit.id
    );
    assert_eq!(replay.review_work_unit.id, first.review_work_unit.id);
    assert_eq!(replay.rounds_remaining, 15);

    let independence_rejected = common::raw_json_request(
        &harness.app,
        Method::POST,
        &format!(
            "/api/v1/work-units/{}/allocation",
            first.review_work_unit.id
        ),
        json!({
            "expected_version": first.review_work_unit.version,
            "role": "reviewer",
            "assigned_actor": {"kind": "agent", "id": agent_id},
        }),
    )
    .await;
    assert_eq!(independence_rejected.status(), StatusCode::BAD_REQUEST);

    let roles: Value = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let reviewer_role_id = roles
        .as_array()
        .and_then(|roles| roles.iter().find(|role| role["role"] == "reviewer"))
        .and_then(|role| role["id"].as_str())
        .expect("formal_review TaskRole exists")
        .to_owned();
    let generic_review: ReviewExecutionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/review-executions", task.id),
        serde_json::to_value(StartReviewExecutionRequest {
            task_role_id: reviewer_role_id,
            workspace_id: None,
        })
        .expect("review start serializes"),
        StatusCode::OK,
    )
    .await;
    let generic_report: SubmitReviewReportResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/review-executions/{}", generic_review.execution.id),
        serde_json::to_value(SubmitReviewReportRequest {
            verdict: ReviewReportVerdict::RequestChanges,
            summary: "Generic Task report without a WorkUnit subject.".to_owned(),
            criteria: vec!["acceptance".to_owned()],
            findings: vec!["generic".to_owned()],
            questions: vec![],
            evidence_ids: vec![],
            artifact_ids: vec![],
            reviewed_work_unit_id: None,
            reviewed_work_unit_version: None,
        })
        .expect("review report serializes"),
        StatusCode::OK,
    )
    .await;
    let mut generic_request = request.clone();
    generic_request.idempotency_key = "generic-report-rework".to_owned();
    generic_request.review_report_artifact_id = generic_report.report.id;
    let generic_rejected = common::raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/rework", task.id),
        serde_json::to_value(generic_request).expect("rework serializes"),
    )
    .await;
    assert_eq!(generic_rejected.status(), StatusCode::BAD_REQUEST);

    let mut conflicting = request.clone();
    conflicting.idempotency_key = "rework-round-1-conflict".to_owned();
    conflicting.review_actor = Some(ActorRef::Agent(agent_id.clone()));
    let rejected = common::raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/rework", task.id),
        serde_json::to_value(conflicting).expect("rework serializes"),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn automatic_rework_replays_once_and_waits_for_task_resume() {
    let repo_dir = TestDir::new("automatic-rework-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspaces = TestDir::new("automatic-rework-workspaces");
    let harness = test_app(workspaces.path(), "automatic-rework").await;
    let (project_id, _) =
        create_project_and_repo(&harness.app, "Automatic rework", &repo_path).await;
    let (agent_id, _) =
        create_shell_agents(&harness.app, workspaces.path(), "automatic-rework").await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Automatic exact rework"}),
        StatusCode::OK,
    )
    .await;
    for (role, actor) in [
        ("implementer", json!({"kind":"agent","id":agent_id})),
        ("reviewer", json!({"kind":"human","id":"test-user-id"})),
    ] {
        let _: Value = json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/tasks/{}/task-roles", task.id),
            json!({
                "role": role,
                "coordination_mode": "independent",
                "configuration": if role == "reviewer" {
                    json!({"capabilities":["formal_review"]})
                } else {
                    json!({})
                },
                "policy": {}
            }),
            StatusCode::OK,
        )
        .await;
        let _: Value = json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/tasks/{}/task-roles/{role}/members", task.id),
            json!({"actor_ref": actor}),
            StatusCode::OK,
        )
        .await;
    }
    let roles: Value = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let get_role = |name: &str| {
        roles
            .as_array()
            .and_then(|roles| roles.iter().find(|role| role["role"] == name))
            .expect("TaskRole exists")
    };
    let implementer_role_id = get_role("implementer")["id"]
        .as_str()
        .expect("implementer TaskRole ID")
        .to_owned();
    let reviewer = get_role("reviewer");
    let reviewer_role_id = reviewer["id"]
        .as_str()
        .expect("reviewer TaskRole ID")
        .to_owned();
    let _: Value = json_request(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/tasks/{}/task-roles/reviewer", task.id),
        json!({
            "expected_version": reviewer["version"],
            "configuration": {
                "schema_version": 1,
                "display_label": "Planning reviewer",
                "capabilities": ["formal_review", "request_rework"],
                "rework": {
                    "mode": "automatic",
                    "allowed_destination_role_ids": [implementer_role_id],
                    "next_review_role_id": reviewer_role_id,
                    "correction_actor": {
                        "kind": "explicit",
                        "actor_ref": {"kind":"agent","id":agent_id}
                    },
                    "review_actor": {"kind":"human","id":"test-user-id"}
                }
            }
        }),
        StatusCode::OK,
    )
    .await;

    let unit = create_allocated_work_unit(
        &harness.app,
        &task.id,
        "implementer",
        &agent_id,
        "Initial package",
        "printf 'initial result\\n'",
        false,
    )
    .await;
    let execution = common::launch_work_unit_execution(
        &harness.app,
        &task.id,
        &unit,
        Some("printf 'initial result\\n'"),
    )
    .await;
    wait_for_execution_completed(&harness.app, &task.id, &execution.id).await;
    let unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", unit.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let review: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title":"Review initial package",
            "scope":"Review the exact package result",
            "role":"reviewer",
            "assigned_actor":{"kind":"human","id":"test-user-id"},
            "requires_integration":false,
            "review_subject":{"reviewed_work_unit_id":unit.id}
        }),
        StatusCode::CREATED,
    )
    .await;
    let current_unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", unit.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let _: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/status", unit.id),
        json!({"expected_version":current_unit.version,"status":"completed"}),
        StatusCode::OK,
    )
    .await;
    let submitted: SubmitReviewReportResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/review-report", review.id),
        json!({
            "expected_version": review.version,
            "report": {
                "verdict":"request_changes",
                "summary":"The exact package is incomplete",
                "criteria":["package completeness"],
                "findings":["missing correction evidence"]
            }
        }),
        StatusCode::OK,
    )
    .await;
    transition_dispatch_for_test(
        &harness,
        &task.id,
        db::TaskDispatchOperation::Launch,
        db::TaskDispatchState::Running,
        "auto-test-launch",
    )
    .await;
    transition_dispatch_for_test(
        &harness,
        &task.id,
        db::TaskDispatchOperation::Pause,
        db::TaskDispatchState::Paused,
        "auto-test-pause",
    )
    .await;
    let event = db::DomainEvent {
        sequence: 1,
        id: "test-automatic-rework-event".to_owned(),
        event_type: "artifact.created".to_owned(),
        entity_type: "artifact".to_owned(),
        entity_id: submitted.report.id.clone(),
        actor_type: "human".to_owned(),
        actor_id: Some("test-user-id".to_owned()),
        scope_type: "task".to_owned(),
        scope_id: task.id.clone(),
        correlation_id: "test-automatic-rework-event".to_owned(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: None,
        payload_json: "{}".to_owned(),
        created_at: db::now_rfc3339(),
    };
    let rework = services::TaskReworkService::new(
        std::sync::Arc::clone(&harness.state.db),
        std::sync::Arc::clone(&harness.state.event_bus),
    );
    assert!(rework
        .process_domain_event(&event)
        .await
        .expect("automatic request is captured"));
    assert_eq!(
        rework
            .process_pending_automatic_rework(100)
            .await
            .expect("paused intent remains durable"),
        0
    );
    let current_task: TaskResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let paused_rework = rework
        .request_rework(
            services::CollaborationActorSource::Human("test-user-id".to_owned()),
            services::RequestReworkInput {
                task_id: task.id.clone(),
                expected_task_version: current_task.version,
                expected_lifecycle_version: current_task.lifecycle.version,
                review_report_artifact_id: submitted.report.id.clone(),
                reviewed_work_unit_id: unit.id.clone(),
                reviewed_work_unit_version: unit.version,
                correction_title: "Paused correction must not materialize".to_owned(),
                correction_scope: "This attempt must be rejected while paused".to_owned(),
                correction_role_id: implementer_role_id.clone(),
                correction_actor: Some(ActorRef::Agent(agent_id.clone())),
                review_title: "Paused review must not materialize".to_owned(),
                review_scope: "This attempt must be rejected while paused".to_owned(),
                review_actor: Some(ActorRef::Human("test-user-id".to_owned())),
                idempotency_key: "paused-before-rework-transaction".to_owned(),
                expected_source_configuration_json: None,
            },
        )
        .await
        .expect_err("transaction rechecks dispatch pause before creating the pair");
    assert!(
        paused_rework
            .to_string()
            .contains("Task dispatch is paused"),
        "pause rejection identifies current dispatch state: {paused_rework}"
    );
    let paused_attempt_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_rework_receipt
         WHERE task_id = ? AND idempotency_key = 'paused-before-rework-transaction'",
    )
    .bind(&task.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("paused request leaves no receipt");
    assert_eq!(paused_attempt_receipts, 0);
    let intents: TaskReworkAutomationIntentsResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/rework/automation", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(intents.items.len(), 1);
    assert_eq!(
        intents.items[0].state,
        api_types::TaskReworkAutomationState::Pending
    );

    transition_dispatch_for_test(
        &harness,
        &task.id,
        db::TaskDispatchOperation::Resume,
        db::TaskDispatchState::Running,
        "auto-test-resume",
    )
    .await;
    let current_task: TaskResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    rework
        .request_rework(
            services::CollaborationActorSource::Human("test-user-id".to_owned()),
            services::RequestReworkInput {
                task_id: task.id.clone(),
                expected_task_version: current_task.version,
                expected_lifecycle_version: current_task.lifecycle.version,
                review_report_artifact_id: submitted.report.id.clone(),
                reviewed_work_unit_id: unit.id.clone(),
                reviewed_work_unit_version: unit.version,
                correction_title: "Manual exact correction".to_owned(),
                correction_scope: "Resolve the exact finding manually".to_owned(),
                correction_role_id: implementer_role_id.clone(),
                correction_actor: Some(ActorRef::Agent(agent_id.clone())),
                review_title: "Manual exact re-review".to_owned(),
                review_scope: "Review the exact manual correction".to_owned(),
                review_actor: Some(ActorRef::Human("test-user-id".to_owned())),
                idempotency_key: "manual-before-automatic-reconcile".to_owned(),
                expected_source_configuration_json: None,
            },
        )
        .await
        .expect("manual intervention creates the one exact correction receipt");
    let manual_receipt_id: String = sqlx::query_scalar(
        "SELECT id FROM task_rework_receipt
         WHERE task_id = ? AND idempotency_key = 'manual-before-automatic-reconcile'",
    )
    .bind(&task.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("manual receipt is durable before the automatic worker resumes");
    assert_eq!(
        rework
            .process_pending_automatic_rework(100)
            .await
            .expect("manual receipt completes the pending automatic intent"),
        1
    );
    assert!(!rework
        .process_domain_event(&event)
        .await
        .expect("event replay is safe"));
    assert_eq!(
        rework
            .process_pending_automatic_rework(100)
            .await
            .expect("completed intent is not recreated"),
        0
    );
    let intents: TaskReworkAutomationIntentsResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/rework/automation", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(intents.items.len(), 1);
    assert_eq!(
        intents.items[0].state,
        api_types::TaskReworkAutomationState::Completed
    );
    assert!(intents.items[0].correction_work_unit_id.is_some());
    assert!(intents.items[0].review_work_unit_id.is_some());
    assert_eq!(
        intents.items[0].reconciled_manual_receipt_id.as_deref(),
        Some(manual_receipt_id.as_str())
    );
    assert_eq!(intents.items[0].rework_round, Some(1));
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_rework_receipt WHERE review_report_artifact_id = ?",
    )
    .bind(&submitted.report.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("exact report receipt count loads");
    assert_eq!(receipt_count, 1);
}
