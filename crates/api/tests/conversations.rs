#![allow(dead_code, clippy::assertions_on_constants)]
use std::{collections::HashSet, sync::Arc, time::Duration};

use api::{build_router, AppState};
use api_types::{
    AgentResponse, ConversationMessageResponse, ConversationResponse, ErrorResponse,
    PaginatedResponse, ProjectResponse, SendMessageResponse,
};
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use db::{
    create_sqlite_pool, run_migrations, DaemonRepo, DaemonStatus, RuntimeRepo, RuntimeStatus,
    UpdateDaemonReport, UpsertDaemon,
};
use executors::{
    ExecutionContext, ExecutionOutcome, ExecutionResult, ExecutorError, LogEntry, LogKind,
    LogStream, TaskExecutor,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tower::ServiceExt;

#[derive(Default)]
struct StreamingCancelableExecutor {
    cancelled: Arc<Mutex<HashSet<String>>>,
}

#[async_trait]
impl TaskExecutor for StreamingCancelableExecutor {
    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        let mut content = String::new();

        if let Some(sender) = ctx.log_sender.as_ref() {
            content.push_str("partial ");
            let _ = sender.send(LogEntry {
                sequence: 1,
                timestamp: db::now_rfc3339(),
                kind: LogKind::AssistantDelta,
                stream: LogStream::Main,
                payload: json!({ "delta": "partial " }),
                schema_version: 1,
                execution_id: ctx.execution_id.clone(),
                truncated: false,
            });
        }

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if self
                .cancelled
                .lock()
                .await
                .contains(ctx.execution_id.as_str())
            {
                return Ok(ExecutionResult {
                    status: ExecutionOutcome::Cancelled,
                    after_sha: None,
                    agent_session_id: Some("session-cancelled".to_owned()),
                    summary: Some(content),
                    error: None,
                    usage: None,
                });
            }
        }

        if let Some(sender) = ctx.log_sender.as_ref() {
            content.push_str("complete");
            let _ = sender.send(LogEntry {
                sequence: 2,
                timestamp: db::now_rfc3339(),
                kind: LogKind::AssistantDelta,
                stream: LogStream::Main,
                payload: json!({ "delta": "complete" }),
                schema_version: 1,
                execution_id: ctx.execution_id.clone(),
                truncated: false,
            });
        }

        Ok(ExecutionResult {
            status: ExecutionOutcome::Completed,
            after_sha: None,
            agent_session_id: Some("session-complete".to_owned()),
            summary: Some(content),
            error: None,
            usage: None,
        })
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        self.cancelled.lock().await.insert(execution_id.to_owned());
        Ok(())
    }
}

#[tokio::test]
async fn conversation_crud_and_message_endpoints_work() {
    let app = test_app(Arc::new(StreamingCancelableExecutor::default())).await;

    let project: ProjectResponse = json_request(
        &app,
        Method::POST,
        "/api/v1/projects",
        json!({ "name": "Chat API" }),
        StatusCode::OK,
    )
    .await;

    let agent: AgentResponse = json_request(
        &app,
        Method::POST,
        "/api/v1/agents",
        json!({ "name": "pm-agent", "executor_type": "codex", "model": "gpt-5", "reasoning_effort": "medium" }),
        StatusCode::OK,
    )
    .await;

    let created: ConversationResponse = json_request(
        &app,
        Method::POST,
        &format!("/api/v1/projects/{}/conversations", project.id),
        json!({ "agent_id": agent.id, "title": "Planning" }),
        StatusCode::OK,
    )
    .await;

    let listed: PaginatedResponse<ConversationResponse> = empty_request(
        &app,
        Method::GET,
        &format!("/api/v1/projects/{}/conversations", project.id),
        StatusCode::OK,
    )
    .await;
    assert!(listed
        .items
        .iter()
        .any(|conversation| conversation.id == created.id));

    let updated: ConversationResponse = json_request(
        &app,
        Method::PATCH,
        &format!("/api/v1/conversations/{}", created.id),
        json!({ "version": created.version, "title": "Roadmap" }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated.title, "Roadmap");

    let _send: SendMessageResponse = json_request(
        &app,
        Method::POST,
        &format!("/api/v1/conversations/{}/messages", created.id),
        json!({ "content": "What should we do next?" }),
        StatusCode::OK,
    )
    .await;

    let messages = wait_for_messages(&app, &created.id, |items| {
        items
            .iter()
            .any(|message| message.role == api_types::ConversationMessageRole::Assistant)
            && items
                .iter()
                .any(|message| message.role == api_types::ConversationMessageRole::User)
    })
    .await;
    assert!(messages.len() >= 2);

    let delete_response = raw_empty_request(
        &app,
        Method::DELETE,
        &format!("/api/v1/conversations/{}", created.id),
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    let active: PaginatedResponse<ConversationResponse> = empty_request(
        &app,
        Method::GET,
        &format!(
            "/api/v1/projects/{}/conversations?status=active",
            project.id
        ),
        StatusCode::OK,
    )
    .await;
    assert!(!active
        .items
        .iter()
        .any(|conversation| conversation.id == created.id));

    let archived: PaginatedResponse<ConversationResponse> = empty_request(
        &app,
        Method::GET,
        &format!(
            "/api/v1/projects/{}/conversations?status=archived",
            project.id
        ),
        StatusCode::OK,
    )
    .await;
    assert!(archived
        .items
        .iter()
        .any(|conversation| conversation.id == created.id));
}

#[tokio::test]
async fn cancel_endpoint_marks_streaming_message_cancelled_and_preserves_partial_content() {
    let app = test_app(Arc::new(StreamingCancelableExecutor::default())).await;

    let project: ProjectResponse = json_request(
        &app,
        Method::POST,
        "/api/v1/projects",
        json!({ "name": "Chat Cancel" }),
        StatusCode::OK,
    )
    .await;

    let agent: AgentResponse = json_request(
        &app,
        Method::POST,
        "/api/v1/agents",
        json!({ "name": "pm-agent", "executor_type": "codex", "model": "gpt-5", "reasoning_effort": "medium" }),
        StatusCode::OK,
    )
    .await;

    let created: ConversationResponse = json_request(
        &app,
        Method::POST,
        &format!("/api/v1/projects/{}/conversations", project.id),
        json!({ "agent_id": agent.id, "title": "Cancel Test" }),
        StatusCode::OK,
    )
    .await;

    let _send: SendMessageResponse = json_request(
        &app,
        Method::POST,
        &format!("/api/v1/conversations/{}/messages", created.id),
        json!({ "content": "stream something" }),
        StatusCode::OK,
    )
    .await;

    let pre_cancel_messages = wait_for_messages(&app, &created.id, |items| {
        items.iter().any(|message| {
            message.role == api_types::ConversationMessageRole::Assistant
                && message.status != api_types::ConversationMessageStatus::Streaming
        }) || items.iter().any(|message| {
            message.role == api_types::ConversationMessageRole::Assistant
                && message.status == api_types::ConversationMessageStatus::Streaming
        })
    })
    .await;

    let assistant_before_cancel = pre_cancel_messages
        .iter()
        .find(|message| message.role == api_types::ConversationMessageRole::Assistant)
        .expect("assistant message exists");

    let mut cancel = raw_empty_request(
        &app,
        Method::POST,
        &format!("/api/v1/conversations/{}/cancel", created.id),
    )
    .await;

    if assistant_before_cancel.status == api_types::ConversationMessageStatus::Streaming {
        for _ in 0..10 {
            if cancel.status() == StatusCode::NO_CONTENT {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel = raw_empty_request(
                &app,
                Method::POST,
                &format!("/api/v1/conversations/{}/cancel", created.id),
            )
            .await;
        }
        assert_eq!(cancel.status(), StatusCode::NO_CONTENT);

        let messages = wait_for_messages(&app, &created.id, |items| {
            items.iter().any(|message| {
                message.role == api_types::ConversationMessageRole::Assistant
                    && message.status == api_types::ConversationMessageStatus::Cancelled
            })
        })
        .await;

        let cancelled = messages
            .iter()
            .find(|message| {
                message.role == api_types::ConversationMessageRole::Assistant
                    && message.status == api_types::ConversationMessageStatus::Cancelled
            })
            .expect("cancelled assistant exists");
        if !assistant_before_cancel.content.is_empty() {
            assert!(
                !cancelled.content.is_empty(),
                "partial content should be preserved when deltas were emitted"
            );
        }
    } else {
        assert_eq!(cancel.status(), StatusCode::CONFLICT);
    }

    let second_cancel = raw_empty_request(
        &app,
        Method::POST,
        &format!("/api/v1/conversations/{}/cancel", created.id),
    )
    .await;
    assert_eq!(second_cancel.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = parse_response(second_cancel, StatusCode::CONFLICT).await;
    assert_eq!(error.code, "no_active_response");
}

async fn test_app(executor: Arc<dyn TaskExecutor>) -> Router {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations(&pool).await.expect("migrations run");
    let db_instance = db::SqliteDb::new(pool);

    let now = db::now_rfc3339();
    let daemon_id = db::new_uuid_v4();
    DaemonRepo::upsert_by_machine_id(
        &db_instance,
        UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: services::embedded_daemon::embedded_machine_id(),
            hostname: "test-host".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            status: DaemonStatus::Online,
            agent_version: None,
            labels_json: "{}".to_owned(),
            registration_token_hash: None,
            owner_id: None,
            visibility: "global".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("daemon seeds");
    DaemonRepo::update_report(
        &db_instance,
        UpdateDaemonReport {
            id: daemon_id.clone(),
            last_report_at: now.clone(),
            status: DaemonStatus::Online,
            detected_clis_json: serde_json::json!([
                {"kind": "shell", "availability": "authenticated", "path": "/bin/sh"}
            ])
            .to_string(),
            labels_json: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("daemon report seeds");
    RuntimeRepo::create(
        &db_instance,
        db::CreateRuntime {
            id: db::new_uuid_v4(),
            daemon_id,
            kind: "local_process".to_owned(),
            workspace_root: "/tmp/forge-test".to_owned(),
            status: RuntimeStatus::Ready,
            labels_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("runtime seeds");

    let db = Arc::new(db_instance);
    let event_bus = Arc::new(events::EventBus::new(32));
    let mut state = AppState::with_adapter_registry(
        db,
        event_bus,
        true,
        Arc::new(cli_adapters::default_registry()),
    );
    state.task_executor = executor;

    let web_dist_dir =
        std::env::temp_dir().join(format!("forge-api-chat-tests-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&web_dist_dir).expect("create web dist dir");
    std::fs::write(web_dist_dir.join("index.html"), "<html></html>").expect("write index");

    build_router(state, web_dist_dir)
}

async fn wait_for_messages(
    app: &Router,
    conversation_id: &str,
    predicate: impl Fn(&[ConversationMessageResponse]) -> bool,
) -> Vec<ConversationMessageResponse> {
    for _ in 0..40 {
        let response: PaginatedResponse<ConversationMessageResponse> = empty_request(
            app,
            Method::GET,
            &format!("/api/v1/conversations/{conversation_id}/messages?limit=200"),
            StatusCode::OK,
        )
        .await;
        if predicate(&response.items) {
            return response.items;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let response: PaginatedResponse<ConversationMessageResponse> = empty_request(
        app,
        Method::GET,
        &format!("/api/v1/conversations/{conversation_id}/messages?limit=200"),
        StatusCode::OK,
    )
    .await;
    response.items
}

async fn json_request<T>(
    app: &Router,
    method: Method,
    uri: &str,
    body: Value,
    expected_status: StatusCode,
) -> T
where
    T: DeserializeOwned,
{
    parse_response(
        raw_json_request(app, method, uri, body).await,
        expected_status,
    )
    .await
}

async fn empty_request<T>(app: &Router, method: Method, uri: &str, expected_status: StatusCode) -> T
where
    T: DeserializeOwned,
{
    parse_response(raw_empty_request(app, method, uri).await, expected_status).await
}

async fn parse_response<T>(response: axum::response::Response, expected_status: StatusCode) -> T
where
    T: DeserializeOwned,
{
    assert_eq!(response.status(), expected_status);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body bytes");
    serde_json::from_slice(&body).expect("json response")
}

fn test_jwt() -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "test-user-id",
        "email": "test@example.com",
        "iat": now,
        "exp": now + 900,
    });
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(b"test-jwt-secret-for-development"),
    )
    .expect("encode test jwt")
}

async fn raw_json_request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {}", test_jwt()))
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("request succeeds")
}

async fn raw_empty_request(app: &Router, method: Method, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {}", test_jwt()))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("request succeeds")
}
