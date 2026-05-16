use api_types::{
    CreateConversationRequest, PaginatedResponse, SendMessageRequest, SendMessageResponse,
    UpdateConversationRequest,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use db::{ConversationStatus, PageRequest, SortBy, SortOrder};
use executors::ExecutionOverrides;
use serde::Deserialize;
use services::ServiceError;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    routes::{conversation_message_response, conversation_response, paginated},
    state::AppState,
};

#[derive(Debug, Deserialize)]
pub struct ConversationListParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub include_total: Option<bool>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MessageListParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub include_total: Option<bool>,
    pub before_sequence: Option<i64>,
}

pub async fn create_conversation(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(project_id): Path<String>,
    Json(request): Json<CreateConversationRequest>,
) -> ApiResult<Json<api_types::ConversationResponse>> {
    {
        let usable_agents = state
            .db
            .list_agents_usable_in_project(&project_id, &user.user_id)
            .await
            .map_err(ApiError::from)?;
        let is_usable = usable_agents
            .into_iter()
            .any(|agent| agent.id == request.agent_id);
        if !is_usable {
            return Err(ApiError::bad_request("agent not usable in this project"));
        }
    }
    let conversation = state
        .conversation_service
        .create_conversation(
            project_id,
            request.agent_id,
            request.title,
            request.system_prompt,
        )
        .await
        .map_err(map_create_error)?;
    Ok(Json(conversation_response(conversation)))
}

pub async fn list_conversations(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(params): Query<ConversationListParams>,
) -> ApiResult<Json<PaginatedResponse<api_types::ConversationResponse>>> {
    let status = match params.status.as_deref() {
        Some("active") | None => Some(ConversationStatus::Active),
        Some("archived") => Some(ConversationStatus::Archived),
        Some(other) => {
            return Err(ApiError::bad_request_with_code(
                "invalid_status",
                format!("invalid status: {other}"),
            ));
        }
    };
    let page = PageRequest {
        cursor: params.cursor,
        limit: params.limit.unwrap_or(20).clamp(1, 100),
        include_total: params.include_total.unwrap_or(false),
        sort_by: SortBy::CreatedAt,
        sort_order: SortOrder::Desc,
    };
    let conversations = state
        .conversation_service
        .list_conversations(project_id, status, page)
        .await?;
    Ok(Json(paginated(conversations, conversation_response)))
}

pub async fn get_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<api_types::ConversationResponse>> {
    let conversation = state.conversation_service.get_conversation(id).await?;
    Ok(Json(conversation_response(conversation)))
}

pub async fn update_conversation(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Json(request): Json<UpdateConversationRequest>,
) -> ApiResult<Json<api_types::ConversationResponse>> {
    if let Some(agent_id) = request.agent_id.as_ref() {
        let existing = state
            .conversation_service
            .get_conversation(id.clone())
            .await?;
        let usable_agents = state
            .db
            .list_agents_usable_in_project(&existing.project_id, &user.user_id)
            .await
            .map_err(ApiError::from)?;
        let is_usable = usable_agents.into_iter().any(|agent| agent.id == *agent_id);
        if !is_usable {
            return Err(ApiError::bad_request("agent not usable in this project"));
        }
    }
    let conversation = state
        .conversation_service
        .update_conversation(
            id,
            request.version,
            request.title,
            request.agent_id,
            request.system_prompt,
            request.status.map(|status| match status {
                api_types::ConversationStatus::Active => ConversationStatus::Active,
                api_types::ConversationStatus::Archived => ConversationStatus::Archived,
            }),
        )
        .await?;
    Ok(Json(conversation_response(conversation)))
}

pub async fn archive_conversation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state.conversation_service.archive_conversation(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SendMessageRequest>,
) -> ApiResult<Json<SendMessageResponse>> {
    let overrides = request.overrides.map(|overrides| ExecutionOverrides {
        model_id: overrides.model_id,
        reasoning_effort: overrides.reasoning_effort,
        permission_policy: None,
    });
    let (user_message, assistant_message) = state
        .conversation_service
        .send_message(id, request.content, overrides, state.task_executor.clone())
        .await
        .map_err(map_send_error)?;

    Ok(Json(SendMessageResponse {
        user_message: conversation_message_response(user_message),
        assistant_message: conversation_message_response(assistant_message),
    }))
}

pub async fn list_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<MessageListParams>,
) -> ApiResult<Json<PaginatedResponse<api_types::ConversationMessageResponse>>> {
    let page = PageRequest {
        cursor: params.cursor,
        limit: params.limit.unwrap_or(50).clamp(1, 200),
        include_total: params.include_total.unwrap_or(false),
        sort_by: SortBy::CreatedAt,
        sort_order: SortOrder::Desc,
    };
    let messages = state
        .conversation_service
        .list_messages(id, params.before_sequence, page)
        .await?;
    Ok(Json(paginated(messages, conversation_message_response)))
}

pub async fn get_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let entries = state.conversation_service.list_log_entries(id).await?;
    Ok(Json(serde_json::json!({
        "items": entries,
        "has_more": false,
    })))
}

pub async fn cancel_response(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state
        .conversation_service
        .cancel_response(id, state.task_executor.clone())
        .await
        .map_err(map_cancel_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn map_create_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::NotFound {
            entity: "agent",
            id,
        } => ApiError::not_found_with_code("NOT_FOUND", "agent", id),
        other => ApiError::from(other),
    }
}

fn map_send_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::Conflict(message) if message.contains("archived") => {
            ApiError::conflict_with_code("CONVERSATION_ARCHIVED", message)
        }
        ServiceError::InvalidOperation { message } if message.contains("empty") => {
            ApiError::bad_request_with_code("validation_error", message)
        }
        other => ApiError::from(other),
    }
}

fn map_cancel_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::Conflict(message) if message.contains("no active response") => {
            ApiError::conflict_with_code("no_active_response", message)
        }
        other => ApiError::from(other),
    }
}
