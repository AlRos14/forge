#![allow(dead_code)]

mod common;

use api_types::{GateEvaluationResponse, GateResponse, TaskLifecycleResponse, TaskResponse};
use axum::http::{Method, StatusCode};
use common::{create_project_and_repo, json_request, setup_git_repo, test_app, TestDir};
use serde_json::json;

#[tokio::test]
async fn task_gate_policy_and_exact_evaluation_are_available_over_rest() {
    let workspace = TestDir::new("pr9-gate-api-workspaces");
    let repo_root = TestDir::new("pr9-gate-api-repo");
    let repo_path = setup_git_repo(repo_root.path());
    let harness = test_app(workspace.path(), "pr9-gate-api").await;
    let (project_id, _repo_id) =
        create_project_and_repo(&harness.app, "Gate API", &repo_path).await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Gate API task", "description": "Exercise a Task-scoped Gate"}),
        StatusCode::OK,
    )
    .await;
    let lifecycle: TaskLifecycleResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        json!(null),
        StatusCode::OK,
    )
    .await;
    assert_eq!(lifecycle.task_id, task.id);
    assert_eq!(lifecycle.state, api_types::TaskLifecycleState::Ready);

    let policy = json!({
        "schema_version": 1,
        "scope_requirement": null,
        "review": null,
        "validations": [{
            "validation_run_id": "missing-run",
            "evidence_id": "missing-evidence",
            "evidence_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "check_identity": "cargo-test",
            "config_digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "workspace_id": "workspace-exact",
            "commit_sha": "cccccccccccccccccccccccccccccccccccccccc",
            "workspace_snapshot_digest": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "required_outcome": "passed"
        }],
        "decisions": [],
        "work_units": []
    });
    let gate: GateResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates", task.id),
        json!({"gate_kind": "validation", "policy": policy}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(gate.gate.task_id, task.id);
    assert_eq!(gate.gate.scope_kind, "task");
    assert_eq!(gate.gate.scope_id, task.id);
    assert_eq!(gate.gate.active_policy_revision, Some(1));

    let evaluation: GateEvaluationResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/gates/{}/evaluate", gate.gate.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(evaluation.gate_id, gate.gate.id);
    assert_eq!(evaluation.task_id, task.id);
    assert_eq!(evaluation.policy_revision, 1);
    assert_eq!(evaluation.outcome, "unsatisfied");
    assert!(evaluation.inputs.is_empty());

    let replay: GateEvaluationResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/gate-evaluations/{}", evaluation.id),
        json!(null),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, evaluation);

    let current_gate: GateResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/gates/{}", gate.gate.id),
        json!(null),
        StatusCode::OK,
    )
    .await;
    assert_eq!(current_gate.gate.active_policy_revision, Some(1));
    assert_eq!(current_gate.active_policy.unwrap().policy, policy);
}

#[tokio::test]
async fn legacy_workflow_gate_actions_cannot_write_task_lifecycle() {
    let workspace = TestDir::new("pr9-retired-gate-workspaces");
    let repo_root = TestDir::new("pr9-retired-gate-repo");
    let repo_path = setup_git_repo(repo_root.path());
    let harness = test_app(workspace.path(), "pr9-retired-gate-api").await;
    let (project_id, _repo_id) =
        create_project_and_repo(&harness.app, "Retired Gate API", &repo_path).await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Legacy Gate cannot transition lifecycle"}),
        StatusCode::OK,
    )
    .await;

    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates/review/approve", task.id),
        json!({"version": task.version, "reason": "reviewed"}),
        StatusCode::CONFLICT,
    )
    .await;
    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates/review/reject", task.id),
        json!({"version": task.version, "reason": "changes required"}),
        StatusCode::CONFLICT,
    )
    .await;

    let lifecycle: TaskLifecycleResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        json!(null),
        StatusCode::OK,
    )
    .await;
    assert_eq!(lifecycle.state, api_types::TaskLifecycleState::Ready);
    assert_eq!(lifecycle.version, 1);
}

#[tokio::test]
async fn rejected_decision_cannot_authorize_merge_and_invalid_creation_leaves_no_gate() {
    let workspace = TestDir::new("pr9-merge-decision-policy-workspaces");
    let repo_root = TestDir::new("pr9-merge-decision-policy-repo");
    let repo_path = setup_git_repo(repo_root.path());
    let harness = test_app(workspace.path(), "pr9-merge-decision-policy").await;
    let (project_id, _repo_id) =
        create_project_and_repo(&harness.app, "Merge Decision Policy", &repo_path).await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "A rejection must never authorize merge"}),
        StatusCode::OK,
    )
    .await;

    let failed_validation = json!({
        "schema_version": 1,
        "review": null,
        "validations": [{
            "validation_run_id": "failed-run",
            "evidence_id": "failed-evidence",
            "evidence_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "check_identity": "cargo-test",
            "config_digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "workspace_id": "workspace-exact",
            "commit_sha": "cccccccccccccccccccccccccccccccccccccccc",
            "workspace_snapshot_digest": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "required_outcome": "failed"
        }],
        "decisions": [],
        "work_units": []
    });
    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates", task.id),
        json!({"gate_kind": "merge_readiness", "policy": failed_validation}),
        StatusCode::BAD_REQUEST,
    )
    .await;

    let rejected = json!({
        "schema_version": 1,
        "review": null,
        "validations": [],
        "decisions": [{
            "proposal_id": "proposal-exact",
            "proposal_version": 1,
            "decision_id": "decision-exact",
            "outcome": "reject",
            "policy_ref": "merge-policy",
            "policy_version": 1,
            "policy_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "permitted_deciders": []
        }],
        "work_units": []
    });
    let _: serde_json::Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates", task.id),
        json!({"gate_kind": "merge_readiness", "policy": rejected}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let gate_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM gate WHERE task_id = ?")
        .bind(&task.id)
        .fetch_one(harness.state.db.pool())
        .await
        .expect("invalid policy leaves no durable Gate");
    assert_eq!(gate_count, 0);

    let approved = json!({
        "schema_version": 1,
        "review": null,
        "validations": [],
        "decisions": [{
            "proposal_id": "proposal-exact",
            "proposal_version": 1,
            "decision_id": "decision-exact",
            "outcome": "approve",
            "policy_ref": "merge-policy",
            "policy_version": 1,
            "policy_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "permitted_deciders": []
        }],
        "work_units": []
    });
    let gate: GateResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates", task.id),
        json!({"gate_kind": "merge_readiness", "policy": approved}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(gate.gate.active_policy_revision, Some(1));
    let gate_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM gate WHERE task_id = ?")
        .bind(&task.id)
        .fetch_one(harness.state.db.pool())
        .await
        .expect("Gate count after successful atomic create");
    assert_eq!(gate_count, 1);
}
