use api_types::{
    ProjectCopilotMessagesResponse, ProjectCopilotResponse, ProjectCopilotRun,
    ProjectCopilotRunDetailResponse, SendProjectCopilotMessageRequest,
    SendProjectCopilotMessageResponse, UpdateProjectCopilotRequest,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use sqlx::Row;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};

#[derive(Deserialize)]
pub struct MessagesQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

pub async fn get(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<ProjectCopilotResponse>> {
    super::projects::require_project_visible(&state, &project_id, &user.user_id).await?;
    Ok(Json(state.project_copilot_service.get(&project_id).await?))
}

pub async fn update(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<UpdateProjectCopilotRequest>,
) -> ApiResult<Json<ProjectCopilotResponse>> {
    super::projects::require_project_admin(&state, &project_id, &user.user_id).await?;
    Ok(Json(
        state
            .project_copilot_service
            .update_binding(
                &project_id,
                &user.user_id,
                body.expected_version,
                body.agent_id.as_deref(),
            )
            .await?,
    ))
}

pub async fn messages(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<MessagesQuery>,
) -> ApiResult<Json<ProjectCopilotMessagesResponse>> {
    super::projects::require_project_visible(&state, &project_id, &user.user_id).await?;
    Ok(Json(
        state
            .project_copilot_service
            .list_messages(
                &project_id,
                query.cursor.as_deref(),
                query.limit.unwrap_or(50).clamp(1, 100),
            )
            .await?,
    ))
}

pub async fn send(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<SendProjectCopilotMessageRequest>,
) -> ApiResult<(StatusCode, Json<SendProjectCopilotMessageResponse>)> {
    super::projects::require_project_mutable(&state, &project_id, &user.user_id).await?;
    if !state.mcp_enabled {
        return Err(ApiError::bad_request(
            "Project Copilot requires the Forge MCP server",
        ));
    }
    // The startup configuration contains the actual bound listener. Neither a
    // request header, a public reverse-proxy origin nor a mutable discovery
    // file may choose where this credential is sent.
    let mut address: std::net::SocketAddr = state
        .effective_config
        .server
        .bind
        .parse()
        .map_err(|_| ApiError::bad_request("Project Copilot requires a bound Forge server"))?;
    if address.port() == 0 {
        return Err(ApiError::bad_request(
            "Project Copilot requires a bound Forge server",
        ));
    }
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    let mcp_url = format!("http://{address}/mcp");
    let response = state
        .project_copilot_service
        .send_message(
            &project_id,
            &user.user_id,
            &body.content,
            &body.idempotency_key,
            body.task_id.as_deref(),
            &mcp_url,
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

pub async fn run(
    State(state): State<AppState>,
    Path((project_id, run_id)): Path<(String, String)>,
    user: AuthenticatedUser,
) -> ApiResult<Json<ProjectCopilotRunDetailResponse>> {
    super::projects::require_project_visible(&state, &project_id, &user.user_id).await?;
    Ok(Json(
        state
            .project_copilot_service
            .get_run(&project_id, &run_id)
            .await?,
    ))
}

pub async fn cancel(
    State(state): State<AppState>,
    Path((project_id, run_id)): Path<(String, String)>,
    user: AuthenticatedUser,
) -> ApiResult<Json<ProjectCopilotRun>> {
    super::projects::require_project_mutable(&state, &project_id, &user.user_id).await?;
    Ok(Json(
        state
            .project_copilot_service
            .cancel_run(&project_id, &run_id, &user.user_id)
            .await?,
    ))
}

pub async fn activity(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<MessagesQuery>,
) -> ApiResult<Json<api_types::ProjectCopilotActivityResponse>> {
    super::projects::require_project_visible(&state, &project_id, &user.user_id).await?;
    let before = match query.cursor {
        None => i64::MAX,
        Some(cursor) => {
            let bytes = hex::decode(cursor)
                .map_err(|_| ApiError::bad_request("Invalid activity cursor"))?;
            i64::from_be_bytes(
                bytes
                    .try_into()
                    .map_err(|_| ApiError::bad_request("Invalid activity cursor"))?,
            )
        }
    };
    let limit = query.limit.unwrap_or(30).clamp(1, 100);
    let mut rows = sqlx::query("SELECT e.sequence,e.id,e.correlation_id,e.event_type,e.entity_type,e.entity_id,e.scope_type,e.scope_id,e.payload_json,e.created_at,r.agent_id,r.user_id FROM domain_event e JOIN project_copilot_run r ON r.id=e.correlation_id WHERE r.project_id=? AND e.actor_type='agent' AND e.actor_id=r.agent_id AND e.sequence<? ORDER BY e.sequence DESC LIMIT ?")
        .bind(&project_id).bind(before).bind(limit + 1).fetch_all(state.db.pool()).await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = if has_more {
        rows.last()
            .map(|row| hex::encode(row.get::<i64, _>("sequence").to_be_bytes()))
    } else {
        None
    };
    let items = rows
        .into_iter()
        .map(|row| {
            let payload: serde_json::Value =
                serde_json::from_str(row.get::<&str, _>("payload_json")).unwrap_or_default();
            api_types::ProjectCopilotActivity {
                id: row.get("id"),
                run_id: row.get("correlation_id"),
                actor_id: row.get("agent_id"),
                requested_by_user_id: row.get("user_id"),
                event_type: row.get("event_type"),
                entity_type: row.get("entity_type"),
                entity_id: row.get("entity_id"),
                created_at: row.get("created_at"),
                task_id: (row.get::<&str, _>("scope_type") == "task").then(|| row.get("scope_id")),
                tool: payload
                    .get("tool")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                outcome: payload
                    .get("outcome")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            }
        })
        .collect();
    Ok(Json(api_types::ProjectCopilotActivityResponse {
        items,
        next_cursor,
    }))
}
