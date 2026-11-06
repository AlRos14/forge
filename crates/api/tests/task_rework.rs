#![allow(dead_code)]

mod common;

use std::time::Duration;

use api_types::{
    ActorRef, ExecutionResponse, ExecutionStatus, PaginatedResponse, RequestReworkRequest,
    RequestReworkResponse, ReviewExecutionResponse, ReviewReportVerdict,
    StartReviewExecutionRequest, SubmitReviewReportRequest, SubmitReviewReportResponse,
    TaskResponse, WorkUnitResponse,
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
            json!({"role": role, "coordination_mode": "independent", "policy": {}}),
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
            correction_role: "implementer".to_owned(),
            correction_actor: Some(ActorRef::Agent(agent_id)),
            review_title: "Repository review".to_owned(),
            review_scope: "Review the exact corrected repository result".to_owned(),
            review_role: None,
            review_actor: None,
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
            json!({"role": role, "coordination_mode": "independent", "policy": {}}),
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
        correction_role: "implementer".to_owned(),
        correction_actor: Some(ActorRef::Agent(agent_id.clone())),
        review_title: "Review R2".to_owned(),
        review_scope: "Review the exact correction result".to_owned(),
        review_role: None,
        review_actor: None,
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
    assert_eq!(
        first.correction_work_unit.status,
        api_types::WorkUnitStatus::Open
    );
    assert_eq!(
        first.review_work_unit.status,
        api_types::WorkUnitStatus::Open
    );
    assert_eq!(first.rounds_remaining, 7);
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
    assert_eq!(replay.rounds_remaining, 7);

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

    let generic_review: ReviewExecutionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/review-executions", task.id),
        serde_json::to_value(StartReviewExecutionRequest { workspace_id: None })
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
