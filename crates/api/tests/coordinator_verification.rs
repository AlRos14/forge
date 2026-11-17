#![allow(dead_code)]

//! Phase-1 REST contract of coordinator verification: validation catalog,
//! check request/comparison reads, human decisions and Gate policy schema v2.

mod common;

use api_types::{
    GateResponse, TaskResponse, ValidationCheckDefinitionResponse, ValidationCheckListResponse,
    ValidationCheckRequestListResponse, ValidationCheckState, ValidationComparisonListResponse,
};
use axum::http::{Method, StatusCode};
use common::{
    create_project_and_repo, empty_request, json_request, json_request_with_bearer, setup_git_repo,
    test_app, test_jwt_for_user, TestDir,
};
use serde_json::{json, Value};

struct Fixture {
    harness: common::Harness,
    project_id: String,
    task: TaskResponse,
    _workspace: TestDir,
    _repo: TestDir,
}

async fn fixture(prefix: &str) -> Fixture {
    let workspace = TestDir::new(&format!("{prefix}-workspaces"));
    let repo_root = TestDir::new(&format!("{prefix}-repo"));
    let repo_path = setup_git_repo(repo_root.path());
    let harness = test_app(workspace.path(), prefix).await;
    let (project_id, _repo_id) =
        create_project_and_repo(&harness.app, "Verification", &repo_path).await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Verification task", "description": "catalog and decisions"}),
        StatusCode::OK,
    )
    .await;
    Fixture {
        harness,
        project_id,
        task,
        _workspace: workspace,
        _repo: repo_root,
    }
}

fn check_body(command: &str, key: &str) -> Value {
    json!({
        "repo_id": null,
        "check_key": key,
        "command": command,
        "timeout_seconds": 60,
        "idempotency_key": format!("publish-{key}-{command}"),
    })
}

async fn add_member(fixture: &Fixture, user_id: &str, role: &str) -> String {
    let now = db::now_rfc3339();
    db::UserRepo::create_user(
        &*fixture.harness.state.db,
        &db::User {
            id: user_id.to_owned(),
            email: format!("{user_id}@example.com"),
            password_hash: "$2b$04$placeholder".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("member user creates");
    db::ProjectMemberRepo::add_member(
        &*fixture.harness.state.db,
        db::CreateProjectMember {
            id: db::new_uuid_v4(),
            project_id: fixture.project_id.clone(),
            user_id: user_id.to_owned(),
            role: role.to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("member adds");
    test_jwt_for_user(user_id, &format!("{user_id}@example.com"), false)
}

#[tokio::test]
async fn catalog_versions_replay_and_retire_with_typed_conflicts() {
    let f = fixture("cv-catalog").await;
    let url = format!("/api/v1/projects/{}/validation-checks", f.project_id);

    let v1: ValidationCheckDefinitionResponse = json_request(
        &f.harness.app,
        Method::POST,
        &url,
        check_body("true", "unit"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(v1.version, 1);
    assert_eq!(v1.state, ValidationCheckState::Active);
    assert_eq!(v1.definition_digest.len(), 64);

    // A retried publish with identical content returns the active version.
    let replay: ValidationCheckDefinitionResponse = json_request(
        &f.harness.app,
        Method::POST,
        &url,
        check_body("true", "unit"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay.id, v1.id);

    // Changed content mints version 2 and retires version 1.
    let v2: ValidationCheckDefinitionResponse = json_request(
        &f.harness.app,
        Method::POST,
        &url,
        check_body("true && true", "unit"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(v2.version, 2);
    assert_ne!(v2.definition_digest, v1.definition_digest);

    let active: ValidationCheckListResponse = empty_request(
        &f.harness.app,
        Method::GET,
        &format!("{url}?state=active"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(active.items.len(), 1);
    assert_eq!(active.items[0].id, v2.id);
    let all: ValidationCheckListResponse =
        empty_request(&f.harness.app, Method::GET, &url, StatusCode::OK).await;
    assert_eq!(all.items.len(), 2);

    // v1 is already retired: typed 409.
    let conflict: Value = json_request(
        &f.harness.app,
        Method::POST,
        &format!("/api/v1/validation-checks/{}/retire", v1.id),
        json!({"expected_state": "active"}),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(conflict["code"], "validation.catalog_state_conflict");

    let retired: ValidationCheckDefinitionResponse = json_request(
        &f.harness.app,
        Method::POST,
        &format!("/api/v1/validation-checks/{}/retire", v2.id),
        json!({"expected_state": "active"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(retired.state, ValidationCheckState::Retired);
}

#[tokio::test]
async fn catalog_writes_require_owner_or_admin_and_reads_require_membership() {
    let f = fixture("cv-authority").await;
    let url = format!("/api/v1/projects/{}/validation-checks", f.project_id);
    let member = add_member(&f, "plain-member", "member").await;

    let forbidden: Value = json_request_with_bearer(
        &f.harness.app,
        Method::POST,
        &url,
        &member,
        check_body("true", "unit"),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(forbidden["code"], "validation.catalog_forbidden");

    // Members still read the catalog.
    let listed: ValidationCheckListResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &url,
        &member,
        StatusCode::OK,
    )
    .await;
    assert!(listed.items.is_empty());

    // Invalid input is rejected before any version exists.
    let _: Value = json_request(
        &f.harness.app,
        Method::POST,
        &url,
        json!({"repo_id": null, "check_key": "Bad Key", "command": "true",
               "timeout_seconds": 60, "idempotency_key": "k"}),
        StatusCode::BAD_REQUEST,
    )
    .await;
}

#[tokio::test]
async fn task_verification_reads_are_paginated_and_empty_without_runner_output() {
    let f = fixture("cv-reads").await;
    let requests: ValidationCheckRequestListResponse = empty_request(
        &f.harness.app,
        Method::GET,
        &format!(
            "/api/v1/tasks/{}/validation-check-requests?limit=5",
            f.task.id
        ),
        StatusCode::OK,
    )
    .await;
    assert!(requests.items.is_empty());
    assert!(requests.next_cursor.is_none());
    let comparisons: ValidationComparisonListResponse = empty_request(
        &f.harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/validation-comparisons", f.task.id),
        StatusCode::OK,
    )
    .await;
    assert!(comparisons.items.is_empty());
    let decisions: Value = empty_request(
        &f.harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/human-decisions", f.task.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(decisions["items"], json!([]));

    // No REST route enqueues a check or answers an unknown decision.
    let missing: Value = json_request(
        &f.harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/human-decisions/missing/answer", f.task.id),
        json!({"expected_version": 1, "option_id": "a", "note": null, "idempotency_key": "k"}),
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(missing["code"], "not_found");
}

#[tokio::test]
async fn gate_policy_schema_v2_accepts_required_checks_and_v1_rejects_them() {
    let f = fixture("cv-gate-v2").await;
    let catalog_url = format!("/api/v1/projects/{}/validation-checks", f.project_id);
    let definition: ValidationCheckDefinitionResponse = json_request(
        &f.harness.app,
        Method::POST,
        &catalog_url,
        check_body("true", "unit"),
        StatusCode::OK,
    )
    .await;
    let gates_url = format!("/api/v1/tasks/{}/gates", f.task.id);

    // v1 documents cannot carry required_checks (fail closed).
    let _: Value = json_request(
        &f.harness.app,
        Method::POST,
        &gates_url,
        json!({"gate_kind": "merge_readiness", "policy": {
            "schema_version": 1,
            "required_checks": [{"check_key": "unit",
                "definition_digest": definition.definition_digest,
                "required_outcome": "passed"}]}}),
        StatusCode::BAD_REQUEST,
    )
    .await;

    // A non-passed required outcome is rejected.
    let _: Value = json_request(
        &f.harness.app,
        Method::POST,
        &gates_url,
        json!({"gate_kind": "merge_readiness", "policy": {
            "schema_version": 2,
            "required_checks": [{"check_key": "unit",
                "definition_digest": definition.definition_digest,
                "required_outcome": "failed"}]}}),
        StatusCode::BAD_REQUEST,
    )
    .await;

    let gate: GateResponse = json_request(
        &f.harness.app,
        Method::POST,
        &gates_url,
        json!({"gate_kind": "merge_readiness", "policy": {
            "schema_version": 2,
            "required_checks": [{"check_key": "unit",
                "definition_digest": definition.definition_digest,
                "required_outcome": "passed"}]}}),
        StatusCode::OK,
    )
    .await;
    let policy = gate.active_policy.expect("initial policy");
    assert_eq!(policy.schema_version, 2);
    assert_eq!(policy.policy["required_checks"][0]["check_key"], "unit");

    // Revising to v2 again requires the exact active revision.
    let stale: Value = json_request(
        &f.harness.app,
        Method::PUT,
        &format!("/api/v1/gates/{}/policy", gate.gate.id),
        json!({"expected_active_revision": 99, "policy": {
            "schema_version": 2,
            "required_checks": [{"check_key": "unit",
                "definition_digest": definition.definition_digest,
                "required_outcome": "passed"}]}}),
        StatusCode::CONFLICT,
    )
    .await;
    assert!(stale["code"].is_string());
}
