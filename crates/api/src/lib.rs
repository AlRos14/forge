#![forbid(unsafe_code)]

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::middleware::{from_fn, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, patch, post, put};
use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;

pub mod errors;
pub mod middleware;
mod path_input;
pub mod routes;
pub mod state;

pub use state::AppState;

pub fn build_router(state: AppState, web_dist_dir: impl Into<PathBuf>) -> Router {
    let web_dist_dir = web_dist_dir.into();
    let index_file = web_dist_dir.join("index.html");
    let cors_origins = state.effective_config.server.cors_origins.clone();

    let static_service = ServeDir::new(web_dist_dir).fallback(ServeFile::new(index_file));

    Router::new()
        .route("/healthz", get(healthz))
        // Keep unknown API paths inside the API surface.  Otherwise the
        // outer SPA fallback turns an unknown API mutation into a static-file
        // method response (typically 405), which makes a missing API route
        // look like a valid browser route.
        .route("/api/v1", any(api_not_found))
        .route("/api/v1/{*path}", any(api_not_found))
        // OAuth 2.1 endpoints for MCP client authentication.
        .route(
            "/.well-known/oauth-protected-resource",
            get(routes::oauth::protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(routes::oauth::protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(routes::oauth::authorization_server_metadata),
        )
        .route(
            "/.well-known/{*path}",
            get(|| async { StatusCode::NOT_FOUND }),
        )
        .route(
            "/oauth/register",
            post(routes::oauth::register_public_client),
        )
        .route("/oauth/authorize", get(routes::oauth::authorize))
        .route("/oauth/token", post(routes::oauth::token))
        .route(
            "/api/v1/provider-authorizations/{provider}/callback",
            get(routes::provider_authorizations::provider_authorization_callback),
        )
        .with_state(state.clone())
        .merge(api_router(state))
        .fallback_service(static_service)
        .layer(CompressionLayer::new())
        .layer(middleware::cors_middleware(&cors_origins))
        .layer(from_fn(cache_control_middleware))
        .layer(from_fn(middleware::request_id_middleware))
}

pub fn api_router(state: AppState) -> Router {
    let mcp_enabled = state.mcp_enabled;
    let mcp_state = mcp_enabled.then(|| {
        mcp_server::AppState::with_task_service(
            Arc::clone(&state.db),
            Arc::clone(&state.event_bus),
            Arc::clone(&state.task_service),
            Arc::clone(&state.agent_service),
        )
    });

    let router = Router::new()
        // Auth routes (register/login/refresh/logout exempt from auth middleware)
        .route("/api/v1/auth/register", post(routes::auth::register))
        .route("/api/v1/auth/login", post(routes::auth::login))
        .route("/api/v1/auth/refresh", post(routes::auth::refresh))
        .route("/api/v1/auth/logout", post(routes::auth::logout))
        .route(
            "/api/v1/oauth/authorize/context",
            get(routes::oauth::authorize_context),
        )
        .route(
            "/api/v1/oauth/authorize/approve",
            post(routes::oauth::authorize_approve),
        )
        .route(
            "/api/v1/auth/me",
            get(routes::auth::me).patch(routes::auth::update_me),
        )
        .route(
            "/api/v1/auth/tokens",
            post(routes::auth::create_pat).get(routes::auth::list_pats),
        )
        .route("/api/v1/auth/tokens/{id}", delete(routes::auth::delete_pat))
        .route("/api/v1/admin/users", get(routes::admin::list_users))
        .route(
            "/api/v1/admin/users/{id}",
            patch(routes::admin::update_user_admin).delete(routes::admin::delete_user),
        )
        .route("/api/v1/admin/settings", get(routes::admin::list_settings))
        .route(
            "/api/v1/admin/settings/{key}",
            put(routes::admin::upsert_setting).delete(routes::admin::delete_setting),
        )
        .route(
            "/api/v1/projects",
            post(routes::projects::create_project).get(routes::projects::list_projects),
        )
        .route(
            "/api/v1/projects/{id}",
            get(routes::projects::get_project)
                .patch(routes::projects::update_project)
                .delete(routes::projects::delete_project),
        )
        .route(
            "/api/v1/projects/{id}/pause",
            post(routes::projects::pause_project),
        )
        .route(
            "/api/v1/projects/{id}/resume",
            post(routes::projects::resume_project),
        )
        .route(
            "/api/v1/projects/{id}/media",
            get(routes::project_media::list_media).post(routes::project_media::upload_media),
        )
        .route(
            "/api/v1/projects/{id}/media/{asset_id}",
            get(routes::project_media::serve_media),
        )
        .route(
            "/api/v1/projects/{id}/media/{asset_id}/redact",
            post(routes::project_media::redact_media),
        )
        .route(
            "/api/v1/projects/{id}/media/{asset_id}/purge",
            post(routes::project_media::purge_media),
        )
        .route(
            "/api/v1/projects/{id}/releases/{release_id}",
            get(routes::project_releases::get_release),
        )
        .route(
            "/api/v1/projects/{id}/milestones/{milestone_id}/releases",
            get(routes::project_releases::list_releases),
        )
        .route(
            "/api/v1/projects/{id}/project_hook_runs",
            get(routes::projects::list_project_hook_runs),
        )
        .route("/api/v1/users/search", get(routes::members::search_users))
        .route(
            "/api/v1/projects/{id}/members",
            get(routes::members::list_members).post(routes::members::add_member),
        )
        .route(
            "/api/v1/projects/{id}/members/{user_id}",
            patch(routes::members::update_member_role).delete(routes::members::remove_member),
        )
        .route(
            "/api/v1/projects/{id}/integration",
            post(routes::integrations::create_integration)
                .get(routes::integrations::get_integration)
                .patch(routes::integrations::update_integration)
                .delete(routes::integrations::delete_integration),
        )
        .route(
            "/api/v1/projects/{id}/integration/sync",
            post(routes::integrations::trigger_sync),
        )
        .route(
            "/api/v1/projects/{id}/repos",
            post(routes::repos::create_repo).get(routes::repos::list_repos),
        )
        .route(
            "/api/v1/repos/{id}",
            get(routes::repos::get_repo)
                .patch(routes::repos::update_repo)
                .delete(routes::repos::delete_repo),
        )
        .route("/api/v1/repos/{id}/sync", post(routes::repos::sync_repo))
        .route("/api/v1/fs/list", get(routes::fs::list_entries))
        .route("/api/v1/fs/branches", get(routes::fs::list_branches))
        .route(
            "/api/v1/projects/{project_id}/tasks",
            post(routes::tasks::create_task).get(routes::tasks::list_tasks),
        )
        .route(
            "/api/v1/tasks/{task_id}/work-units",
            post(routes::work_units::create_work_unit).get(routes::work_units::list_work_units),
        )
        .route(
            "/api/v1/work-units/{id}",
            get(routes::work_units::get_work_unit).patch(routes::work_units::update_work_unit),
        )
        .route(
            "/api/v1/work-units/{id}/allocation",
            post(routes::work_units::allocate_work_unit),
        )
        .route(
            "/api/v1/work-units/{id}/status",
            post(routes::work_units::transition_work_unit),
        )
        .route(
            "/api/v1/work-units/{id}/dependencies",
            get(routes::work_units::list_dependencies),
        )
        .route(
            "/api/v1/work-units/{id}/dependencies/{prerequisite_id}",
            post(routes::work_units::add_dependency).delete(routes::work_units::remove_dependency),
        )
        .route(
            "/api/v1/work-units/{id}/readiness",
            get(routes::work_units::readiness),
        )
        .route(
            "/api/v1/work-units/{id}/integrations",
            post(routes::work_units::integrate),
        )
        .route(
            "/api/v1/tasks/{task_id}/artifacts",
            post(routes::collaboration::create_artifact).get(routes::collaboration::list_artifacts),
        )
        .route(
            "/api/v1/artifacts/{id}",
            get(routes::collaboration::get_artifact),
        )
        .route(
            "/api/v1/tasks/{task_id}/messages",
            post(routes::collaboration::create_message).get(routes::collaboration::list_messages),
        )
        .route(
            "/api/v1/messages/{id}",
            get(routes::collaboration::get_message),
        )
        .route(
            "/api/v1/tasks/{task_id}/handoffs",
            post(routes::collaboration::create_handoff).get(routes::collaboration::list_handoffs),
        )
        .route(
            "/api/v1/handoffs/{id}",
            get(routes::collaboration::get_handoff),
        )
        .route(
            "/api/v1/handoffs/{id}/status",
            post(routes::collaboration::transition_handoff),
        )
        .route(
            "/api/v1/tasks/{task_id}/proposals",
            post(routes::collaboration::create_proposal).get(routes::collaboration::list_proposals),
        )
        .route(
            "/api/v1/proposals/{id}",
            get(routes::collaboration::get_proposal),
        )
        .route(
            "/api/v1/proposals/{id}/withdraw",
            post(routes::collaboration::withdraw_proposal),
        )
        .route(
            "/api/v1/tasks/{task_id}/collaboration/decisions",
            post(routes::collaboration::create_decision).get(routes::collaboration::list_decisions),
        )
        .route(
            "/api/v1/decisions/{id}",
            get(routes::collaboration::get_decision),
        )
        .route(
            "/api/v1/tasks/{id}",
            get(routes::tasks::get_task)
                .patch(routes::tasks::update_task)
                .delete(routes::tasks::delete_task),
        )
        .route(
            "/api/v1/tasks/{id}/subtasks/reorder",
            post(routes::tasks::reorder_subtasks),
        )
        .route(
            "/api/v1/tasks/{id}/workspace",
            get(routes::tasks::get_task_workspace),
        )
        .route(
            "/api/v1/tasks/{id}/workspace/reset",
            post(routes::tasks::reset_task_workspace),
        )
        .route("/api/v1/tasks/{id}/diff", get(routes::tasks::get_task_diff))
        .route(
            "/api/v1/tasks/{id}/dependencies",
            post(routes::tasks::add_dependency).get(routes::tasks::list_dependencies),
        )
        .route(
            "/api/v1/tasks/{id}/dependencies/{dep_id}",
            delete(routes::tasks::remove_dependency),
        )
        .route(
            "/api/v1/tasks/{id}/dependents",
            get(routes::tasks::list_dependents),
        )
        .route(
            "/api/v1/tasks/{id}/gates",
            get(routes::tasks::list_task_gates).post(routes::tasks::create_task_gate),
        )
        .route("/api/v1/gates/{id}", get(routes::tasks::get_gate))
        .route(
            "/api/v1/gates/{id}/policy",
            put(routes::tasks::revise_gate_policy),
        )
        .route(
            "/api/v1/gates/{id}/evaluate",
            post(routes::tasks::evaluate_gate),
        )
        .route(
            "/api/v1/gate-evaluations/{id}",
            get(routes::tasks::get_gate_evaluation),
        )
        .route(
            "/api/v1/tasks/{id}/merge",
            post(routes::tasks::merge_after_gate),
        )
        .route(
            "/api/v1/tasks/{id}/lifecycle",
            get(routes::tasks::get_task_lifecycle).post(routes::tasks::transition_task_lifecycle),
        )
        .route(
            "/api/v1/tasks/{id}/lifecycle/transitions",
            get(routes::tasks::list_task_lifecycle_transitions),
        )
        .route(
            "/api/v1/tasks/{id}/task-roles",
            get(routes::tasks::list_task_role_model).post(routes::tasks::create_task_role_model),
        )
        .route(
            "/api/v1/tasks/{id}/task-roles/{role}",
            patch(routes::tasks::update_task_role_model),
        )
        .route(
            "/api/v1/tasks/{id}/task-roles/{role}/members",
            post(routes::tasks::add_task_role_member),
        )
        .route(
            "/api/v1/tasks/{id}/task-roles/{role}/members/{membership_id}",
            patch(routes::tasks::update_task_role_member),
        )
        .route(
            "/api/v1/tasks/{id}/review-executions",
            post(routes::tasks::trigger_review).get(routes::tasks::list_reviews),
        )
        .route(
            "/api/v1/tasks/{id}/terminals",
            post(routes::terminals::create_terminal_session)
                .get(routes::terminals::list_terminal_sessions),
        )
        .route(
            "/api/v1/tasks/{id}/terminals/availability",
            get(routes::terminals::terminal_availability),
        )
        .route(
            "/api/v1/terminals/{id}",
            get(routes::terminals::get_terminal_session),
        )
        .route(
            "/api/v1/terminals/{id}/attach-token",
            post(routes::terminals::issue_terminal_attach_token),
        )
        .route(
            "/api/v1/terminals/{id}/resize",
            post(routes::terminals::resize_terminal_session),
        )
        .route(
            "/api/v1/terminals/{id}/terminate",
            post(routes::terminals::terminate_terminal_session),
        )
        .route(
            "/api/v1/terminals/{id}/ws",
            get(routes::terminals::terminal_ws),
        )
        .route(
            "/api/v1/tasks/{id}/external-links",
            get(routes::external_links::list_external_links)
                .post(routes::external_links::create_external_link),
        )
        .route(
            "/api/v1/tasks/{id}/external-links/{link_id}",
            delete(routes::external_links::delete_external_link),
        )
        .route(
            "/api/v1/review-executions/{id}",
            get(routes::reviews::get_review).post(routes::reviews::submit_review_report),
        )
        .route(
            "/api/v1/tasks/{id}/validation-runs",
            get(routes::reviews::list_validation_runs),
        )
        .route(
            "/api/v1/validation-runs/{id}",
            get(routes::reviews::get_validation_run),
        )
        .route("/api/v1/evidence/{id}", get(routes::reviews::get_evidence))
        .route(
            "/api/v1/notifications",
            get(routes::notifications::list_notifications),
        )
        .route(
            "/api/v1/notifications/unread-count",
            get(routes::notifications::get_unread_count),
        )
        .route(
            "/api/v1/notifications/mark-all-read",
            post(routes::notifications::mark_all_read),
        )
        .route(
            "/api/v1/notifications/{id}/read",
            patch(routes::notifications::mark_read),
        )
        .route(
            "/api/v1/notifications/{id}",
            delete(routes::notifications::delete_notification),
        )
        .route(
            "/api/v1/operations/status",
            get(routes::operations::get_operations_status),
        )
        .route(
            "/api/v1/operations/refresh",
            post(routes::operations::refresh_operations),
        )
        .route(
            "/api/v1/settings",
            get(routes::settings::get_settings).put(routes::settings::update_settings),
        )
        .route(
            "/api/v1/agents",
            post(routes::agents::register_agent).get(routes::agents::list_agents),
        )
        .route(
            "/api/v1/agents/{id}",
            get(routes::agents::get_agent)
                .patch(routes::agents::update_agent)
                .delete(routes::agents::archive_agent),
        )
        .route(
            "/api/v1/agents/{id}/pause",
            post(routes::agents::pause_agent),
        )
        .route(
            "/api/v1/agents/{id}/resume",
            post(routes::agents::resume_agent),
        )
        .route(
            "/api/v1/agents/{id}/availability",
            get(routes::agents::agent_availability),
        )
        .route(
            "/api/v1/agents/{id}/usage",
            get(routes::agents::get_agent_usage),
        )
        .route(
            "/api/v1/agents/{id}/discovered-options",
            get(routes::agents::agent_discovered_options),
        )
        .route(
            "/api/v1/providers/catalog",
            get(routes::providers::provider_catalog),
        )
        .route(
            "/api/v1/providers",
            get(routes::providers::list_providers).post(routes::providers::create_provider_entry),
        )
        .route(
            "/api/v1/providers/{id}",
            patch(routes::providers::rename_provider_entry)
                .delete(routes::providers::delete_provider_entry),
        )
        .route(
            "/api/v1/providers/{id}/test",
            post(routes::providers::test_provider_entry),
        )
        .route(
            "/api/v1/providers/{id}/usage",
            get(routes::providers::usage_provider_entry),
        )
        .route(
            "/api/v1/provider-authorizations",
            post(routes::provider_authorizations::start_provider_authorization),
        )
        .route(
            "/api/v1/provider-authorizations/{id}",
            get(routes::provider_authorizations::get_provider_authorization),
        )
        .route(
            "/api/v1/provider-authorizations/{id}/cancel",
            post(routes::provider_authorizations::cancel_provider_authorization),
        )
        .route(
            "/api/v1/agents/{id}/profiles",
            get(routes::agent_profiles::list_profiles),
        )
        .route(
            "/api/v1/agents/{id}/profiles/{profile_id}/select",
            post(routes::agent_profiles::select_profile),
        )
        .route(
            "/api/v1/executor-types",
            get(routes::executor_types::list_executor_types),
        )
        .route(
            "/api/v1/executor-types/{type_name}/discovered-options",
            get(routes::executor_types::executor_type_discovered_options),
        )
        .route("/api/v1/clis", get(routes::clis::list_clis))
        .route("/api/v1/daemons", get(routes::daemons::list_daemons))
        .route(
            "/api/v1/daemons/register",
            post(routes::daemons::register_daemon),
        )
        .route("/api/v1/daemons/{id}", get(routes::daemons::get_daemon))
        .route(
            "/api/v1/daemons/{id}/connect",
            get(routes::daemons::connect_daemon),
        )
        .route(
            "/api/v1/daemons/{id}/report",
            post(routes::daemons::report_daemon),
        )
        .route(
            "/api/v1/tasks/{id}/executions",
            get(routes::executions::list_executions).post(routes::executions::start_task_execution),
        )
        .route(
            "/api/v1/executions/{id}",
            get(routes::executions::get_execution),
        )
        .route(
            "/api/v1/executions/{id}/logs",
            get(routes::executions::get_logs),
        )
        .route(
            "/api/v1/executions/{id}/hook-logs",
            get(routes::executions::get_hook_logs),
        )
        .route(
            "/api/v1/executions/{id}/follow-up",
            post(routes::executions::follow_up_execution),
        )
        .route(
            "/api/v1/executions/{id}/cancel",
            post(routes::executions::cancel_execution),
        )
        .route(
            "/api/v1/executions/{id}/usage",
            get(routes::executions::get_execution_usage),
        )
        .route(
            "/api/v1/tasks/{id}/usage",
            get(routes::executions::get_task_usage),
        )
        .route(
            "/api/v1/workspaces/{id}/diff",
            get(routes::workspaces::get_workspace_diff),
        )
        .route("/api/v1/events", get(routes::events::stream_events))
        .route(
            "/api/v1/config/mcp",
            get(routes::mcp_config::get_mcp_config).post(routes::mcp_config::update_mcp_config),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth_middleware,
        ))
        .with_state(state.clone());

    let router = if let Some(mcp_state) = mcp_state {
        let mcp_router = mcp_server::mcp_router(mcp_state).layer(
            axum::middleware::from_fn_with_state(state, middleware::mcp_auth_middleware),
        );
        router.merge(mcp_router)
    } else {
        router
    };

    router.layer(
        TraceLayer::new_for_http()
            .make_span_with(|request: &Request| {
                let request_id = request
                    .extensions()
                    .get::<middleware::RequestId>()
                    .map(|request_id| request_id.as_str().to_owned())
                    .or_else(|| {
                        request
                            .headers()
                            .get(&middleware::REQUEST_ID_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "unknown".to_owned());

                tracing::info_span!(
                    "http.trace",
                    request_id = %request_id,
                    method = %request.method(),
                    path = %middleware::request_log_path(request.uri()),
                )
            })
            .on_response(DefaultOnResponse::new().level(Level::INFO)),
    )
}

#[cfg(test)]

pub async fn serve(
    addr: SocketAddr,
    state: AppState,
    web_dist_dir: impl Into<PathBuf>,
) -> Result<(), std::io::Error> {
    serve_with_shutdown(addr, state, web_dist_dir, std::future::pending::<()>()).await
}

pub async fn serve_with_shutdown<F>(
    addr: SocketAddr,
    state: AppState,
    web_dist_dir: impl Into<PathBuf>,
    shutdown_signal: F,
) -> Result<(), std::io::Error>
where
    F: Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_with_listener(listener, state, web_dist_dir, shutdown_signal).await
}

pub async fn serve_with_listener<F>(
    listener: tokio::net::TcpListener,
    state: AppState,
    web_dist_dir: impl Into<PathBuf>,
    shutdown_signal: F,
) -> Result<(), std::io::Error>
where
    F: Future<Output = ()> + Send + 'static,
{
    axum::serve(listener, build_router(state, web_dist_dir))
        .with_graceful_shutdown(shutdown_signal)
        .await
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn api_not_found(uri: Uri) -> impl IntoResponse {
    errors::ApiError::not_found("route", uri.path())
}

async fn cache_control_middleware(req: Request, next: Next) -> Response {
    let uri = req.uri().clone();
    let mut response = next.run(req).await;

    if is_asset_path(&uri) {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else if is_spa_path(&uri) {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, max-age=0"),
        );
    }

    response
}

fn is_asset_path(uri: &Uri) -> bool {
    let path = uri.path();
    path.starts_with("/assets/") || path.ends_with(".js") || path.ends_with(".css")
}

fn is_spa_path(uri: &Uri) -> bool {
    let path = uri.path();
    !path.starts_with("/api") && !path.starts_with("/assets/") && !path.contains('.')
}

#[tokio::test]
async fn router_serves_spa_fallback() {
    use tower::util::ServiceExt;

    let web_dist_dir =
        std::env::temp_dir().join(format!("forge-api-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&web_dist_dir).expect("create web dist dir");
    std::fs::write(web_dist_dir.join("index.html"), "<html></html>").expect("write index");

    let router = build_router(test_state().await, web_dist_dir);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/projects/default/board")
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router response");

    assert_ne!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_api_paths_do_not_fall_through_to_spa() {
    use tower::util::ServiceExt;

    let router = build_router(test_state().await, temp_web_dist());
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ready")
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/projects/default/board")
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router response");

    assert_ne!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn router_compresses_static_assets() {
    use tower::util::ServiceExt;

    let web_dist_dir = std::env::temp_dir().join(format!(
        "forge-api-compression-test-{}",
        uuid::Uuid::new_v4()
    ));
    let assets_dir = web_dist_dir.join("assets");
    std::fs::create_dir_all(&assets_dir).expect("create asset dir");
    std::fs::write(
        assets_dir.join("app.css"),
        ".board{display:grid;}".repeat(100),
    )
    .expect("write asset");

    let router = build_router(test_state().await, web_dist_dir);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/assets/app.css")
                .header(header::ACCEPT_ENCODING, "gzip")
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_ENCODING),
        Some(&HeaderValue::from_static("gzip"))
    );
}

#[tokio::test]
async fn api_errors_include_inbound_request_id() {
    use api_types::ErrorResponse;
    use axum::body::to_bytes;
    use tower::util::ServiceExt;

    let state = test_state().await;
    let token = test_jwt(&state);
    let router = build_router(state, temp_web_dist());
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/missing")
                .header("x-request-id", "test-request-id")
                .header("authorization", format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get(&middleware::REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("test-request-id")
    );

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body collects");
    let error: ErrorResponse = serde_json::from_slice(&body).expect("error response parses");

    assert_eq!(error.request_id, "test-request-id");
}

#[tokio::test]
async fn event_stream_finishes_when_shutdown_is_requested() {
    use axum::body::to_bytes;
    use tokio::time::{timeout, Duration};
    use tower::util::ServiceExt;

    let state = test_state().await;
    let token = test_jwt(&state);
    let shutdown_signal = state.shutdown_signal.clone();
    let _event_bus = Arc::clone(&state.event_bus);
    let app = build_router(state, temp_web_dist());

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/events?token={token}"))
                .body(axum::body::Body::empty())
                .expect("build request"),
        )
        .await
        .expect("event stream response");

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body();
    let mut collect_body = tokio::spawn(async move { to_bytes(body, usize::MAX).await });

    assert!(
        timeout(Duration::from_millis(50), &mut collect_body)
            .await
            .is_err(),
        "event stream should stay open before shutdown"
    );

    shutdown_signal.request();
    timeout(Duration::from_secs(1), collect_body)
        .await
        .expect("event stream ends after shutdown")
        .expect("body task joins")
        .expect("body collects");
}

#[cfg(test)]
async fn test_state() -> AppState {
    use std::sync::Arc;

    let pool = db::create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    db::run_migrations(&pool).await.expect("migrations run");
    let db = Arc::new(db::SqliteDb::new(pool));
    let event_bus = Arc::new(events::EventBus::new(16));
    AppState::new(db, event_bus, true)
}

#[cfg(test)]
fn test_jwt(state: &AppState) -> String {
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
    let secret = b"test-jwt-secret-for-development";
    let _ = state; // use same secret as test_state creates
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .expect("encode test jwt")
}

#[cfg(test)]
fn temp_web_dist() -> std::path::PathBuf {
    let web_dist_dir = std::env::temp_dir().join(format!("forge-api-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&web_dist_dir).expect("create web dist dir");
    std::fs::write(web_dist_dir.join("index.html"), "<html></html>").expect("write index");
    web_dist_dir
}

#[test]
fn backoff_cap_is_stable() {
    let mut backoff = std::time::Duration::from_secs(1);
    for _ in 0..10 {
        backoff = std::cmp::min(backoff * 2, std::time::Duration::from_secs(30));
    }
    assert_eq!(backoff, std::time::Duration::from_secs(30));
}

#[test]
fn request_trace_path_omits_sensitive_query_parameters() {
    let request = Request::builder()
        .uri("/api/v1/events?token=never-log-this")
        .body(axum::body::Body::empty())
        .expect("build request");

    assert_eq!(
        middleware::request_log_path(request.uri()),
        "/api/v1/events"
    );
}
