#![allow(dead_code)]

mod common;

use api_types::{
    ErrorResponse, ExternalFeedbackItemListResponse, PrExternalActionListResponse,
    PrFeedbackPolicyResponse, PrReworkListResponse, ProviderEntriesResponse, ProviderEntryResponse,
    RepoResponse, TaskResponse,
};
use axum::http::{Method, StatusCode};
use common::{
    create_project_and_repo, json_request_with_bearer, setup_git_repo, test_app, test_jwt_for_user,
    TestDir,
};
use serde_json::json;

struct Fixture {
    harness: common::Harness,
    project_id: String,
    repo_id: String,
    task_id: String,
    token: String,
    _workspace: TestDir,
    _repo: TestDir,
}

async fn fixture(prefix: &str) -> Fixture {
    let workspace = TestDir::new(&format!("{prefix}-workspaces"));
    let repo_root = TestDir::new(&format!("{prefix}-repo"));
    let repo_path = setup_git_repo(repo_root.path());
    let harness = test_app(workspace.path(), prefix).await;
    let (project_id, repo_id) =
        create_project_and_repo(&harness.app, "PR feedback loop", &repo_path).await;
    let token = common::test_jwt();
    let task: TaskResponse = json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        &token,
        json!({"title": "PR feedback task"}),
        StatusCode::OK,
    )
    .await;
    Fixture {
        harness,
        project_id,
        repo_id,
        task_id: task.id,
        token,
        _workspace: workspace,
        _repo: repo_root,
    }
}

async fn create_scm_credential(
    fixture: &Fixture,
    token: &str,
    label: &str,
) -> ProviderEntryResponse {
    json_request_with_bearer(
        &fixture.harness.app,
        Method::POST,
        "/api/v1/providers",
        token,
        json!({
            "provider": "github",
            "label": label,
            "credential": "ghp-phase-two-secret"
        }),
        StatusCode::OK,
    )
    .await
}

#[tokio::test]
async fn scm_credentials_are_secret_free_and_repo_reference_checks_owner() {
    let f = fixture("pr-loop-scm").await;
    let credential = create_scm_credential(&f, &f.token, "task SCM").await;
    assert_eq!(credential.provider, "github");
    assert!(!serde_json::to_string(&credential)
        .expect("credential entry serializes")
        .contains("ghp-phase-two-secret"));

    let model_entries: ProviderEntriesResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        "/api/v1/providers",
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert!(model_entries
        .items
        .iter()
        .all(|item| item.id != credential.id));
    let scm_entries: ProviderEntriesResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        "/api/v1/providers?purpose=scm",
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert!(scm_entries
        .items
        .iter()
        .any(|item| item.id == credential.id));
    assert!(!serde_json::to_string(&scm_entries)
        .expect("SCM entries serialize")
        .contains("ghp-phase-two-secret"));

    let configured: RepoResponse = json_request_with_bearer(
        &f.harness.app,
        Method::PATCH,
        &format!("/api/v1/repos/{}", f.repo_id),
        &f.token,
        json!({
            "pr_provider": "github",
            "pr_provider_config": {
                "credential_id": credential.id,
                "polling_interval_seconds": 60
            }
        }),
        StatusCode::OK,
    )
    .await;
    assert!(configured
        .pr_provider_status
        .as_ref()
        .is_some_and(|status| status.has_credential));
    assert!(!serde_json::to_string(&configured)
        .expect("repo serializes")
        .contains("ghp-phase-two-secret"));

    let provider_mismatch: ErrorResponse = json_request_with_bearer(
        &f.harness.app,
        Method::PATCH,
        &format!("/api/v1/repos/{}", f.repo_id),
        &f.token,
        json!({
            "pr_provider": "gitlab",
            "pr_provider_config": {"credential_id": credential.id.clone()}
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(
        provider_mismatch.code,
        "repo.pr_provider_credential_invalid"
    );

    let retained_credential: ErrorResponse = json_request_with_bearer(
        &f.harness.app,
        Method::PATCH,
        &format!("/api/v1/repos/{}", f.repo_id),
        &f.token,
        json!({"pr_provider": "gitlab"}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(
        retained_credential.code,
        "repo.pr_provider_credential_invalid"
    );

    let now = db::now_rfc3339();
    db::UserRepo::create_user(
        &*f.harness.state.db,
        &db::User {
            id: "foreign-scm-owner".to_owned(),
            email: "foreign-scm-owner@example.invalid".to_owned(),
            password_hash: "$2b$04$placeholder".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("foreign credential owner seeds");
    let foreign_token = test_jwt_for_user(
        "foreign-scm-owner",
        "foreign-scm-owner@example.invalid",
        false,
    );
    let foreign_credential = create_scm_credential(&f, &foreign_token, "foreign SCM").await;
    let error: ErrorResponse = json_request_with_bearer(
        &f.harness.app,
        Method::PATCH,
        &format!("/api/v1/repos/{}", f.repo_id),
        &f.token,
        json!({
            "pr_provider_config": {"credential_id": foreign_credential.id}
        }),
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(error.code, "not_found");
}

#[tokio::test]
async fn repo_tokens_are_retired_and_feedback_policy_is_validated() {
    let f = fixture("pr-loop-policy").await;
    let _: RepoResponse = json_request_with_bearer(
        &f.harness.app,
        Method::PATCH,
        &format!("/api/v1/repos/{}", f.repo_id),
        &f.token,
        json!({"pr_provider": "github", "pr_provider_config": {}}),
        StatusCode::OK,
    )
    .await;

    for field in ["token", "token_secret_ref"] {
        let mut config = serde_json::Map::new();
        config.insert(field.to_owned(), json!("legacy-secret"));
        let error: ErrorResponse = json_request_with_bearer(
            &f.harness.app,
            Method::PATCH,
            &format!("/api/v1/repos/{}", f.repo_id),
            &f.token,
            json!({"pr_provider_config": config}),
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(error.code, "repo.pr_provider_token_retired");
        assert!(!serde_json::to_string(&error)
            .expect("error serializes")
            .contains("legacy-secret"));
    }

    let initial: PrFeedbackPolicyResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &format!("/api/v1/repos/{}/pr-feedback-policy", f.repo_id),
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(initial.policy.max_items_per_poll, 50);
    assert_eq!(
        initial.policy.post_summary,
        api_types::PrFeedbackSummaryMode::Off
    );

    let mut enabled = initial.policy.clone();
    enabled.ingest = true;
    let saved: PrFeedbackPolicyResponse = json_request_with_bearer(
        &f.harness.app,
        Method::PUT,
        &format!("/api/v1/repos/{}/pr-feedback-policy", f.repo_id),
        &f.token,
        json!({"expected_updated_at": initial.updated_at, "policy": enabled}),
        StatusCode::OK,
    )
    .await;
    assert!(saved.policy.ingest);

    let stale: ErrorResponse = json_request_with_bearer(
        &f.harness.app,
        Method::PUT,
        &format!("/api/v1/repos/{}/pr-feedback-policy", f.repo_id),
        &f.token,
        json!({
            "expected_updated_at": initial.updated_at,
            "policy": { "schema_version": 1, "ingest": false,
                "trusted_author_associations": [], "trusted_logins": [],
                "max_items_per_poll": 50, "push_corrections": "human_approval",
                "post_summary": "off" }
        }),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(stale.code, "pr_feedback.policy_version_conflict");

    let mut invalid = saved.policy;
    invalid.max_items_per_poll = 0;
    let error: ErrorResponse = json_request_with_bearer(
        &f.harness.app,
        Method::PUT,
        &format!("/api/v1/repos/{}/pr-feedback-policy", f.repo_id),
        &f.token,
        json!({"policy": invalid}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(error.code, "pr_feedback.policy_invalid");
}

#[tokio::test]
async fn task_pr_reads_are_paged_and_action_decisions_check_version() {
    let f = fixture("pr-loop-reads").await;
    let feedback: ExternalFeedbackItemListResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/pr-feedback?limit=1", f.task_id),
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert!(feedback.items.is_empty());
    assert!(feedback.next_cursor.is_none());

    let rework: PrReworkListResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/pr-rework?limit=1", f.task_id),
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert!(rework.items.is_empty());
    assert!(rework.next_cursor.is_none());

    let actions: PrExternalActionListResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/pr-actions?limit=1", f.task_id),
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert!(actions.items.is_empty());
    assert!(actions.next_cursor.is_none());

    // The route's stale-version check is intentionally exercised with a
    // synthetic persisted row. Creating a valid action requires the complete
    // merge/publish/Gate/Proposal graph, which this API lane does not create.
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(f.harness.state.db.pool())
        .await
        .expect("disable FKs for synthetic route fixture");
    sqlx::query("DROP TRIGGER IF EXISTS pr_external_action_insert_guard")
        .execute(f.harness.state.db.pool())
        .await
        .expect("disable admission guard for synthetic route fixture");
    sqlx::query("DROP TRIGGER IF EXISTS pr_external_action_scope_guard")
        .execute(f.harness.state.db.pool())
        .await
        .expect("disable action scope guard for synthetic route fixture");
    sqlx::query("DROP TRIGGER IF EXISTS pr_rework_supersession_insert_guard")
        .execute(f.harness.state.db.pool())
        .await
        .expect("disable rework guard for synthetic route fixture");
    sqlx::query("DROP TRIGGER IF EXISTS external_feedback_item_insert_guard")
        .execute(f.harness.state.db.pool())
        .await
        .expect("disable feedback guard for synthetic route fixture");
    sqlx::query("DROP TRIGGER IF EXISTS remote_pr_admission_insert_guard")
        .execute(f.harness.state.db.pool())
        .await
        .expect("disable admission guard for synthetic route fixture");
    sqlx::query("DROP TRIGGER IF EXISTS task_merge_gate_admission_guard")
        .execute(f.harness.state.db.pool())
        .await
        .expect("disable Gate guard for synthetic route fixture");
    sqlx::query("DROP TRIGGER IF EXISTS task_integration_operation_remote_admission_guard")
        .execute(f.harness.state.db.pool())
        .await
        .expect("disable merge scope guard for synthetic route fixture");
    for (merge_id, publish_id, metadata_id) in [
        (
            "feedback-admission-newest",
            "publish-feedback-newest",
            "metadata-feedback-newest",
        ),
        (
            "feedback-admission-oldest",
            "publish-feedback-oldest",
            "metadata-feedback-oldest",
        ),
        (
            "merge-newest",
            "publish-merge-newest",
            "metadata-merge-newest",
        ),
        (
            "merge-oldest",
            "publish-merge-oldest",
            "metadata-merge-oldest",
        ),
        (
            "synthetic-admission",
            "publish-synthetic",
            "metadata-synthetic",
        ),
        (
            "synthetic-admission-terminal",
            "publish-synthetic-terminal",
            "metadata-synthetic-terminal",
        ),
    ] {
        let now = db::now_rfc3339();
        sqlx::query(
            "INSERT INTO task_integration_operation (
                id, task_id, repo_id, repository_merge_batch_id, kind, owner_id,
                status, version, created_at, updated_at, finished_at,
                gate_evaluation_id, remote_waiting, parent_operation_id, result_event_id
             ) VALUES (?, ?, ?, NULL, 'task_merge', 'synthetic-owner',
                'succeeded', 1, ?, ?, ?, NULL, 1, NULL, NULL)",
        )
        .bind(merge_id)
        .bind(&f.task_id)
        .bind(&f.repo_id)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .execute(f.harness.state.db.pool())
        .await
        .expect("synthetic merge operation creates");
        sqlx::query(
            "INSERT INTO remote_pr_admission (
                task_merge_operation_id, publish_operation_id, metadata_id, task_id,
                repo_id, provider_config_id, provider_type, provider_config_revision,
                provider_config_digest, remote_repo_identity, source_branch, target_branch,
                admitted_source_sha, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, 'github', ?, ?, ?, 'task/test', 'main', ?, ?, ?)",
        )
        .bind(merge_id)
        .bind(publish_id)
        .bind(metadata_id)
        .bind(&f.task_id)
        .bind(&f.repo_id)
        .bind(format!("config-{merge_id}"))
        .bind(&now)
        .bind("a".repeat(64))
        .bind("https://example.invalid/synthetic.git")
        .bind("synthetic-source-sha")
        .bind(&now)
        .bind(&now)
        .execute(f.harness.state.db.pool())
        .await
        .expect("synthetic route admission creates");
    }
    let newest = "2026-01-02T00:00:00Z";
    let oldest = "2026-01-01T00:00:00Z";
    for (id, admission, item, digest, ingested_at) in [
        (
            "feedback-newest",
            "feedback-admission-newest",
            "issue_comment:2",
            "b".repeat(64),
            newest,
        ),
        (
            "feedback-oldest",
            "feedback-admission-oldest",
            "issue_comment:1",
            "a".repeat(64),
            oldest,
        ),
    ] {
        sqlx::query(
            "INSERT INTO external_feedback_item (
                id, task_id, admission_id, provider_item_id, kind, author_login,
                author_association, body, body_truncated, body_digest, trust_class,
                head_state, ingested_at
             ) VALUES (?, ?, ?, ?, 'issue_comment', 'reviewer', 'MEMBER',
                'provider feedback', 0, ?, 'trusted', 'current', ?)",
        )
        .bind(id)
        .bind(&f.task_id)
        .bind(admission)
        .bind(item)
        .bind(digest)
        .bind(ingested_at)
        .execute(f.harness.state.db.pool())
        .await
        .expect("synthetic feedback row creates");
    }
    for (id, admission, request, digest, created_at, state) in [
        (
            "rework-newest",
            "merge-newest",
            "request-newest",
            "b".repeat(64),
            newest,
            "remote_closed",
        ),
        (
            "rework-oldest",
            "merge-oldest",
            "request-oldest",
            "a".repeat(64),
            oldest,
            "abandoned",
        ),
    ] {
        sqlx::query(
            "INSERT INTO pr_rework_supersession (
                id, task_id, task_merge_operation_id, publish_operation_id, metadata_id,
                request_id, triage_set_digest, provider_pr_id, superseded_head_sha,
                result_event_id, state, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, '77', 'abcdef1234567', ?, ?, 1, ?, ?)",
        )
        .bind(id)
        .bind(&f.task_id)
        .bind(admission)
        .bind(format!("publish-{id}"))
        .bind(format!("metadata-{id}"))
        .bind(request)
        .bind(digest)
        .bind(format!("event-{id}"))
        .bind(state)
        .bind(created_at)
        .bind(created_at)
        .execute(f.harness.state.db.pool())
        .await
        .expect("synthetic rework row creates");
    }
    sqlx::query(
        "INSERT INTO pr_external_action (
            id, task_id, admission_id, kind, state, authorization_mode,
            summary_artifact_id, body_digest, idempotency_key, version, created_at, updated_at
         ) VALUES (?, ?, ?, 'post_comment', 'proposed', 'human_approval', ?, ?, ?, 2, ?, ?)",
    )
    .bind("synthetic-action")
    .bind(&f.task_id)
    .bind("synthetic-admission")
    .bind("synthetic-artifact")
    .bind("a".repeat(64))
    .bind("synthetic-idempotency")
    .bind(db::now_rfc3339())
    .bind(db::now_rfc3339())
    .execute(f.harness.state.db.pool())
    .await
    .expect("synthetic action row creates");
    sqlx::query(
        "INSERT INTO pr_external_action (
            id, task_id, admission_id, kind, state, authorization_mode,
            summary_artifact_id, body_digest, idempotency_key, version,
            created_at, updated_at, finished_at
         ) VALUES (?, ?, ?, 'post_comment', 'rejected', 'human_approval', ?, ?, ?, 2, ?, ?, ?)",
    )
    .bind("synthetic-action-terminal")
    .bind(&f.task_id)
    .bind("synthetic-admission-terminal")
    .bind("synthetic-artifact-terminal")
    .bind("c".repeat(64))
    .bind("synthetic-idempotency-terminal")
    .bind(oldest)
    .bind(oldest)
    .bind(oldest)
    .execute(f.harness.state.db.pool())
    .await
    .expect("synthetic terminal action row creates");

    let feedback_first: ExternalFeedbackItemListResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/pr-feedback?limit=1", f.task_id),
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(feedback_first.items[0].id, "feedback-newest");
    let feedback_cursor = feedback_first.next_cursor.expect("feedback next cursor");
    let feedback_second: ExternalFeedbackItemListResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &format!(
            "/api/v1/tasks/{}/pr-feedback?limit=1&cursor={feedback_cursor}",
            f.task_id
        ),
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(feedback_second.items[0].id, "feedback-oldest");
    assert!(feedback_second.next_cursor.is_none());

    let rework_first: PrReworkListResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/pr-rework?limit=1", f.task_id),
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(rework_first.items[0].id, "rework-newest");
    let rework_cursor = rework_first.next_cursor.expect("rework next cursor");
    let rework_second: PrReworkListResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &format!(
            "/api/v1/tasks/{}/pr-rework?limit=1&cursor={rework_cursor}",
            f.task_id
        ),
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(rework_second.items[0].id, "rework-oldest");
    assert!(rework_second.next_cursor.is_none());

    let actions_first: PrExternalActionListResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/pr-actions?limit=1", f.task_id),
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(actions_first.items[0].id, "synthetic-action");
    let action_cursor = actions_first.next_cursor.expect("action next cursor");
    let actions_second: PrExternalActionListResponse = common::empty_request_with_bearer(
        &f.harness.app,
        Method::GET,
        &format!(
            "/api/v1/tasks/{}/pr-actions?limit=1&cursor={action_cursor}",
            f.task_id
        ),
        &f.token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(actions_second.items[0].id, "synthetic-action-terminal");
    assert!(actions_second.next_cursor.is_none());

    let error: ErrorResponse = json_request_with_bearer(
        &f.harness.app,
        Method::POST,
        &format!(
            "/api/v1/tasks/{}/pr-actions/synthetic-action/approve",
            f.task_id
        ),
        &f.token,
        json!({"expected_version": 1, "idempotency_key": "stale-decision"}),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(error.code, "pr_action.version_conflict");
}
