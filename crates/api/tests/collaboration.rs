mod common;

use api_types::{
    ArtifactResponse, AuthResponse, DecisionResponse, HandoffResponse, MessageResponse,
    PaginatedResponse, ProposalResponse, TaskResponse,
};
use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use common::*;
use serde_json::{json, Value};

#[tokio::test]
async fn generic_api_derives_sender_authorizes_ids_and_hides_content_ref() {
    let repo_dir = TestDir::new("pr4-collaboration-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspace_root = TestDir::new("pr4-collaboration-workspaces");
    let harness = test_app(workspace_root.path(), "pr4-collaboration-api").await;
    let (project_id, _) =
        create_project_and_repo(&harness.app, "PR4 Collaboration", &repo_path).await;
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({ "title": "generic collaboration" }),
        StatusCode::OK,
    )
    .await;

    let now = db::now_rfc3339();
    let execution_id = "pr4-api-human-execution";
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, created_at, updated_at,
             actor_kind, actor_id, purpose
         ) VALUES (?, ?, NULL, 'interactive', 'completed', ?, ?, 'human', 'test-user-id', 'general')",
    )
    .bind(execution_id)
    .bind(&task.id)
    .bind(&now)
    .bind(&now)
    .execute(harness.state.db.pool())
    .await
    .expect("Human Execution");

    let artifact_response = raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/artifacts", task.id),
        json!({
            "kind": "summary",
            "storage_kind": "external",
            "content": null,
            "content_ref": "private-storage://bucket/internal-object",
            "metadata": {"source": "test"},
            "digest": "sha256:test",
            "producer_execution_id": execution_id
        }),
    )
    .await;
    assert_eq!(artifact_response.status(), StatusCode::OK);
    let body = to_bytes(artifact_response.into_body(), usize::MAX)
        .await
        .expect("Artifact response body");
    let value: Value = serde_json::from_slice(&body).expect("Artifact response JSON");
    assert!(value.get("content_ref").is_none());
    let artifact: ArtifactResponse = serde_json::from_value(value).expect("Artifact response");
    assert_eq!(
        artifact.producer,
        api_types::ActorRef::Human("test-user-id".to_owned())
    );
    let artifact_detail: Value = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/artifacts/{}", artifact.id),
        StatusCode::OK,
    )
    .await;
    assert!(artifact_detail.get("content_ref").is_none());

    let spoofed = raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/messages", task.id),
        json!({
            "target": {"kind": "task"},
            "body": "should not be admitted",
            "sender": {"kind": "human", "id": "different-user"}
        }),
    )
    .await;
    assert_eq!(spoofed.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let message: MessageResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/messages", task.id),
        json!({
            "target": {"kind": "task"},
            "body": "valid message",
            "artifact_ids": [artifact.id]
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        message.sender,
        api_types::ActorRef::Human("test-user-id".to_owned())
    );
    let messages: PaginatedResponse<MessageResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/messages", task.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(messages.items.len(), 1);

    let handoff: HandoffResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/handoffs", task.id),
        json!({
            "target": {"kind": "task"},
            "intent": "question",
            "artifact_ids": []
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(handoff.status, api_types::HandoffStatus::Pending);
    let accepted: HandoffResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/handoffs/{}/status", handoff.id),
        json!({"status": "accepted", "expected_version": 1}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(accepted.status, api_types::HandoffStatus::Accepted);
    assert_eq!(accepted.version, 2);
    let handoffs: PaginatedResponse<HandoffResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/handoffs", task.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(handoffs.items.len(), 1);

    let proposal: ProposalResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/proposals", task.id),
        json!({
            "target": {"kind": "task", "id": task.id.clone()},
            "action": "record-api-decision",
            "reason": "exercise the new generic Decision route",
            "required_policy_ref": "policy://test"
        }),
        StatusCode::OK,
    )
    .await;
    let decision: DecisionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/collaboration/decisions", task.id),
        json!({
            "proposal_id": proposal.id,
            "proposal_version": proposal.content_version,
            "outcome": "approve",
            "rationale": "recorded, not executed"
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        decision.actors,
        vec![api_types::ActorRef::Human("test-user-id".to_owned())]
    );
    let replaceable: ProposalResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/proposals", task.id),
        json!({
            "target": {"kind": "task", "id": task.id},
            "action": "withdraw-this-proposal",
            "reason": "no longer needed"
        }),
        StatusCode::OK,
    )
    .await;
    let withdrawn: ProposalResponse = empty_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/proposals/{}/withdraw", replaceable.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(withdrawn.status, api_types::ProposalStatus::Withdrawn);
    let proposals: PaginatedResponse<ProposalResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/proposals", task.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(proposals.items.len(), 2);
    let decisions: PaginatedResponse<DecisionResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/collaboration/decisions", task.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(decisions.items.len(), 1);

    let list: PaginatedResponse<ArtifactResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/artifacts", task.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(list.items.len(), 1);
    assert!(list.items[0].content.is_none(), "list omits inline content");

    let outsider: AuthResponse = json_request(
        &harness.app,
        Method::POST,
        "/api/v1/auth/register",
        json!({
            "email": "outsider@example.test",
            "password": "valid-test-password",
            "display_name": "Outsider"
        }),
        StatusCode::CREATED,
    )
    .await;
    let unauthorized = empty_request_with_bearer::<api_types::ErrorResponse>(
        &harness.app,
        Method::GET,
        &format!("/api/v1/artifacts/{}", artifact.id),
        &outsider.access_token,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert!(!unauthorized.message.contains("private-storage"));
    for path in [
        format!("/api/v1/messages/{}", message.id),
        format!("/api/v1/proposals/{}", proposal.id),
        format!("/api/v1/decisions/{}", decision.id),
    ] {
        let unauthorized = empty_request_with_bearer::<api_types::ErrorResponse>(
            &harness.app,
            Method::GET,
            &path,
            &outsider.access_token,
            StatusCode::NOT_FOUND,
        )
        .await;
        assert!(!unauthorized.message.contains("valid message"));
        assert!(!unauthorized.message.contains("recorded, not executed"));
    }
}
