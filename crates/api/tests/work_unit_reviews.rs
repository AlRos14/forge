#![allow(dead_code)]
mod common;

use api_types::{ExecutionResponse, SubmitReviewReportResponse, TaskResponse, WorkUnitResponse};
use axum::http::{Method, StatusCode};
use common::*;
use serde_json::{json, Value};

#[tokio::test]
async fn human_reviews_an_exact_work_unit_without_creating_an_execution_manually() {
    let repo = TestDir::new("human-review-unit-repo");
    let repo_path = setup_git_repo(repo.path());
    let workspace = TestDir::new("human-review-unit-workspaces");
    let harness = test_app(workspace.path(), "human-review-unit").await;
    let (project_id, _) = create_project_and_repo(&harness.app, "Human review", &repo_path).await;
    let (agent_id, _) =
        create_shell_agents(&harness.app, workspace.path(), "human-review-unit").await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title":"Deliver and independently review"}),
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
            json!({"role":role,"coordination_mode":"independent","policy":{},"configuration":if role=="reviewer" {json!({"capabilities":["formal_review"]})}else{json!({})}}),
            StatusCode::OK,
        )
        .await;
        let _: Value = json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/tasks/{}/task-roles/{role}/members", task.id),
            json!({"actor_ref":actor}),
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
    let role = |name: &str| {
        roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == name)
            .unwrap()
    };
    let implementer_role_id = role("implementer")["id"].as_str().unwrap().to_owned();
    let _: Value = json_request(&harness.app,Method::PATCH,&format!("/api/v1/tasks/{}/task-roles/reviewer",task.id),json!({
        "expected_version":role("reviewer")["version"],
        "configuration":{"capabilities":["formal_review","request_rework"],"rework":{
            "mode":"manual","allowed_destination_role_ids":[implementer_role_id],"next_review_role_id":role("reviewer")["id"],
            "correction_actor":null,"review_actor":{"kind":"human","id":"test-user-id"}
        }}
    }),StatusCode::OK).await;
    let command =
        "printf delivered > evidence.txt && git add evidence.txt && git commit -m delivered";
    let implementation = create_allocated_work_unit(
        &harness.app,
        &task.id,
        "implementer",
        &agent_id,
        "Deliver I1",
        command,
        true,
    )
    .await;
    let review: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title":"Review I1", "scope":"Review the exact delivered result", "role":"reviewer",
            "assigned_actor":{"kind":"human","id":"test-user-id"}, "requires_integration":false,
            "review_subject":{"reviewed_work_unit_id":implementation.id}
        }),
        StatusCode::CREATED,
    )
    .await;
    let attempt =
        launch_work_unit_execution(&harness.app, &task.id, &implementation, Some(command)).await;
    for _ in 0..400 {
        let current: ExecutionResponse = empty_request(
            &harness.app,
            Method::GET,
            &format!("/api/v1/executions/{}", attempt.id),
            StatusCode::OK,
        )
        .await;
        if current.status == api_types::ExecutionStatus::Completed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let implementation: WorkUnitResponse = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", implementation.id),
        StatusCode::OK,
    )
    .await;
    let _: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/status", implementation.id),
        json!({"expected_version":implementation.version,"status":"completed"}),
        StatusCode::OK,
    )
    .await;
    let body = json!({"expected_version":review.version,"report":{
        "verdict":"request_changes","summary":"Fix the missing acceptance evidence",
        "criteria":["evidence"],"findings":["Missing evidence"]
    }});
    let submitted: SubmitReviewReportResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/review-report", review.id),
        body.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        submitted.review_execution.execution.work_unit_id.as_deref(),
        Some(review.id.as_str())
    );
    assert!(submitted
        .review_execution
        .execution
        .harness_session_id
        .is_none());
    let content: Value =
        serde_json::from_str(submitted.report.content.as_deref().unwrap()).unwrap();
    assert_eq!(
        content["subject"]["reviewed_work_unit_id"],
        implementation.id
    );
    assert_eq!(
        content["subject"]["reviewed_work_unit_version"],
        implementation.version
    );
    assert!(content["subject"]["reviewed_result"]["workspace_id"].is_string());
    assert_eq!(
        content["subject"]["reviewed_result"]["commit_sha"]
            .as_str()
            .unwrap()
            .len(),
        40
    );
    assert_eq!(
        content["subject"]["reviewed_result"]["workspace_snapshot_digest"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    let replay: SubmitReviewReportResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/review-report", review.id),
        body,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay.report.id, submitted.report.id);
    assert_eq!(
        replay.review_execution.execution.id,
        submitted.review_execution.execution.id
    );
    let current: WorkUnitResponse = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", review.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(current.status, api_types::WorkUnitStatus::Completed);

    // A Gate on I1 consumes a distinct reviewer WorkUnit's exact result report.
    // The reviewer's own workspace remains absent; that must not weaken the
    // source-result pin or confuse the reviewer with the reviewed WorkUnit.
    let passing_review: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title":"Verify I1 evidence", "scope":"Check I1 exact result", "role":"reviewer",
            "assigned_actor":{"kind":"human","id":"test-user-id"},"requires_integration":false,
            "review_subject":{"reviewed_work_unit_id":implementation.id}
        }),
        StatusCode::CREATED,
    )
    .await;
    let passed: SubmitReviewReportResponse = json_request(&harness.app,Method::POST,
        &format!("/api/v1/work-units/{}/review-report",passing_review.id),json!({
            "expected_version":passing_review.version,
            "report":{"verdict":"pass","summary":"The pinned evidence is sufficient","criteria":["evidence"]}
        }),StatusCode::OK).await;
    let passed_content: Value =
        serde_json::from_str(passed.report.content.as_deref().unwrap()).unwrap();
    let exact = &passed_content["subject"]["reviewed_result"];
    use sha2::Digest;
    for wrong_pin in [false, true] {
        let gate: api_types::GateResponse = json_request(&harness.app,Method::POST,
            &format!("/api/v1/tasks/{}/gates",task.id),json!({
                "gate_kind":if wrong_pin {"wrong_source_pin"} else {"exact_source_pin"},
                "policy": {"schema_version":1,"review":{
                    "mode":"one_acceptable","required_count":1,"human_required":true,
                    "allow_humans":true,"allow_agents":false,
                    "allowed_actor_refs":[{"kind":"human","id":"test-user-id"}],
                    "candidates":[{"artifact_id":passed.report.id,"digest":passed.report.digest,
                        "expected_actor":{"kind":"human","id":"test-user-id"},"required":true,
                        "subject":{"workspace_id":null,"base_commit_sha":null,"head_commit_sha":null,"workspace_snapshot_digest":null,
                            "reviewed_result":{"work_unit_id":implementation.id,"work_unit_version":implementation.version,
                                "execution_id":if wrong_pin {Value::String("another-result".to_owned())} else {exact["execution_id"].clone()},
                                "artifact_id":exact["artifact_id"],"commit_sha":exact["commit_sha"],
                                "workspace_id":exact["workspace_id"],"workspace_snapshot_digest":exact["workspace_snapshot_digest"]}
                        }}]},
                    "validations":[],"decisions":[],"work_units":[{
                        "work_unit_id":implementation.id,"version":implementation.version+1,
                        "dependency_digest":hex::encode(sha2::Sha256::digest(b"[]")),
                        "execution_id":attempt.id,"execution_result_sha":exact["commit_sha"],
                        "require_integration":false,"integration_id":null,"integration_version":null
                    }]}
            }),StatusCode::OK).await;
        let evaluated: api_types::GateEvaluationResponse = json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/gates/{}/evaluate", gate.gate.id),
            json!({}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            evaluated.outcome,
            if wrong_pin {
                "unsatisfied"
            } else {
                "satisfied"
            }
        );
    }
    // Rework requires an explicit Actor. Later allocation and instruction edits
    // must not strand R2 on the preparation version of C1.
    let task: TaskResponse = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        StatusCode::OK,
    )
    .await;
    let mut rework_request = json!({
        "expected_task_version":task.version,"expected_lifecycle_version":task.lifecycle.version,
        "review_report_artifact_id":submitted.report.id,
        "reviewed_work_unit_id":implementation.id,"reviewed_work_unit_version":implementation.version,
        "correction_title":"C1","correction_scope":"Add missing evidence","correction_role_id":implementer_role_id,
        "correction_actor":null,"review_title":"R2","review_scope":"Review corrected evidence",
        "review_actor":{"kind":"human","id":"test-user-id"},
        "idempotency_key":"late-allocation-round"
    });
    let rework_url = format!("/api/v1/tasks/{}/rework", task.id);
    let _: Value = json_request(
        &harness.app,
        Method::POST,
        &rework_url,
        rework_request.clone(),
        StatusCode::BAD_REQUEST,
    )
    .await;
    rework_request["correction_actor"] = json!({"kind":"agent","id":agent_id});
    let round: api_types::RequestReworkResponse = json_request(
        &harness.app,
        Method::POST,
        &rework_url,
        rework_request,
        StatusCode::OK,
    )
    .await;
    let lines: Vec<api_types::ReviewLineResponse> = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/review-lines", task.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    let line = lines
        .iter()
        .find(|line| line.root_work_unit_id == implementation.id)
        .unwrap();
    assert_eq!(line.current_work_unit_id, round.correction_work_unit.id);
    assert!(line.current_result_execution_id.is_none());
    assert!(line.latest_report_artifact_id.is_none());
    assert!(line
        .history
        .iter()
        .any(|revision| revision.review_report_artifact_id.as_deref()
            == Some(submitted.report.id.as_str())));
    let correction: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!(
            "/api/v1/work-units/{}/allocation",
            round.correction_work_unit.id
        ),
        json!({
            "expected_version":round.correction_work_unit.version,"role":"implementer",
            "assigned_actor":{"kind":"agent","id":agent_id}
        }),
        StatusCode::OK,
    )
    .await;
    let command =
        "printf corrected > correction.txt && git add correction.txt && git commit -m corrected\nexit $?\n";
    let attempt =
        launch_work_unit_execution(&harness.app, &task.id, &correction, Some(command)).await;
    let mut completed = false;
    for _ in 0..400 {
        let current: ExecutionResponse = empty_request(
            &harness.app,
            Method::GET,
            &format!("/api/v1/executions/{}", attempt.id),
            StatusCode::OK,
        )
        .await;
        assert!(
            current.status != api_types::ExecutionStatus::Failed,
            "correction failed: {current:?}"
        );
        if current.status == api_types::ExecutionStatus::Completed {
            completed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(completed, "correction must finish before acceptance");
    let correction: WorkUnitResponse = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/work-units/{}", correction.id),
        StatusCode::OK,
    )
    .await;
    let _: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/status", correction.id),
        json!({"expected_version":correction.version,"status":"completed"}),
        StatusCode::OK,
    )
    .await;
    let report: SubmitReviewReportResponse = json_request(
        &harness.app,
        Method::POST,
        &format!(
            "/api/v1/work-units/{}/review-report",
            round.review_work_unit.id
        ),
        json!({
            "expected_version":round.review_work_unit.version,
            "report":{"verdict":"pass","summary":"Correction accepted","criteria":["evidence"]}
        }),
        StatusCode::OK,
    )
    .await;
    let content: Value = serde_json::from_str(report.report.content.as_deref().unwrap()).unwrap();
    assert_eq!(
        content["subject"]["reviewed_result"]["execution_id"],
        attempt.id
    );
    assert_eq!(
        content["subject"]["reviewed_work_unit_version"],
        correction.version
    );
}
