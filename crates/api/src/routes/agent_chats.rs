//! REST resources for the singular Main/Project Agent Chat surface.
//!
//! The route layer is deliberately authorization-first: chat and Project IDs
//! are looked up only after deriving the owning account from the authenticated
//! user.  The repository layer owns persistence and optimistic concurrency;
//! this module only translates the public API shapes.

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};
use api_types::{
    AgentBindingState, AgentChatDetailResponse, AgentChatKind, AgentChatMessageAuthorType,
    AgentChatMessageListResponse, AgentChatMessageResponse, AgentChatMessageStatus,
    AgentChatMessagesQuery, AgentChatResponse, AgentChatStatus, AgentChatSwitcherItem,
    AgentChatSwitcherResponse, AgentChatTurnJobResponse, AgentChatTurnStatus, AgentHandoffResponse,
    AgentHandoffStatus, CancelAgentChatTurnRequest, CreateAgentHandoffRequest,
    MainAgentBindingResponse, ProjectAgentBindingResponse, SendAgentChatMessageRequest,
    SendAgentChatMessageResponse, SetMainAgentBindingRequest, SetProjectAgentBindingRequest,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use db::{
    AccountMainAgentBinding, AccountMainAgentBindingRepo, AgentChat, AgentChatMessage,
    AgentChatMessageAuthorType as DbMessageAuthorType, AgentChatMessageListQuery,
    AgentChatMessageRepo, AgentChatMessageStatus as DbMessageStatus, AgentChatRepo,
    AgentChatTurnJob, AgentChatTurnJobRepo, AgentChatTurnState, AgentHandoff, AgentHandoffRepo,
    AgentRepo, PageRequest, ProjectAgentBinding, ProjectAgentBindingRepo, ProjectMemberRepo,
    ProjectRepo, SortBy, SortOrder,
};
use serde_json::{json, Value};

pub async fn get_main_agent(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> ApiResult<Json<MainAgentBindingResponse>> {
    let binding = AccountMainAgentBindingRepo::get_active_main_binding(&*state.db, &user.user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("main_agent_binding", user.user_id.clone()))?;
    Ok(Json(main_binding_response(&state, binding).await?))
}

pub async fn set_main_agent(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Json(_request): Json<SetMainAgentBindingRequest>,
) -> ApiResult<Json<MainAgentBindingResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "Main Agent bindings were retired in Plan PR11",
    ))
}

pub async fn get_project_agent(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(project_id): Path<String>,
) -> ApiResult<Json<ProjectAgentBindingResponse>> {
    require_project_member(&state, &project_id, &user.user_id).await?;
    let binding = current_project_binding(&state, &project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project_agent_binding", project_id.clone()))?;
    Ok(Json(project_binding_response(&state, binding).await?))
}

pub async fn set_project_agent(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_project_id): Path<String>,
    Json(_request): Json<SetProjectAgentBindingRequest>,
) -> ApiResult<Json<ProjectAgentBindingResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "Project Agent bindings were retired in Plan PR11; use explicit TaskRole membership",
    ))
}

pub async fn list_agent_chats(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Query(_query): Query<api_types::AgentChatListQuery>,
) -> ApiResult<Json<AgentChatSwitcherResponse>> {
    let chats = state
        .agent_chat_history
        .list_authorized_chats(&user.user_id)
        .await?;
    let mut items = Vec::with_capacity(chats.len());
    for chat in chats {
        items.push(switcher_item(&state, chat).await?);
    }
    items.sort_by_key(|item| match item.kind {
        AgentChatKind::Main => 0,
        AgentChatKind::Project => 1,
    });
    Ok(Json(AgentChatSwitcherResponse { items }))
}

pub async fn get_agent_chat(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(chat_id): Path<String>,
) -> ApiResult<Json<AgentChatDetailResponse>> {
    let chat = state
        .agent_chat_history
        .get_authorized_chat(&user.user_id, &chat_id)
        .await?;
    let pending_turn_count = pending_turn_count(&state, &chat.id).await?;
    let main_binding = if chat.kind == "account_main" {
        match AccountMainAgentBindingRepo::get_active_main_binding(&*state.db, &user.user_id)
            .await?
        {
            Some(binding) => Some(main_binding_response(&state, binding).await?),
            None => None,
        }
    } else {
        None
    };
    let project_binding = if chat.kind == "project" {
        match current_project_binding(&state, chat.project_id.as_deref().unwrap_or_default())
            .await?
        {
            Some(binding) => Some(project_binding_response(&state, binding).await?),
            None => None,
        }
    } else {
        None
    };
    Ok(Json(AgentChatDetailResponse {
        chat: chat_response(chat, pending_turn_count, &user.user_id),
        main_binding,
        project_binding,
    }))
}

pub async fn list_agent_chat_messages(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(chat_id): Path<String>,
    Query(query): Query<AgentChatMessagesQuery>,
) -> ApiResult<Json<AgentChatMessageListResponse>> {
    state
        .agent_chat_history
        .get_authorized_chat(&user.user_id, &chat_id)
        .await?;
    let page = AgentChatMessageRepo::list_agent_chat_messages(
        &*state.db,
        AgentChatMessageListQuery {
            chat_id,
            before_sequence: query.before_sequence,
            page: PageRequest {
                cursor: query.cursor,
                limit: query.limit.unwrap_or(50).clamp(1, 100),
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Asc,
            },
        },
    )
    .await?;
    Ok(Json(AgentChatMessageListResponse {
        items: page.items.into_iter().map(message_response).collect(),
        next_cursor: page.next_cursor.clone(),
        has_more: page.next_cursor.is_some(),
    }))
}

pub async fn send_agent_chat_message(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_chat_id): Path<String>,
    Json(_request): Json<SendAgentChatMessageRequest>,
) -> ApiResult<(StatusCode, Json<SendAgentChatMessageResponse>)> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "Agent Chat cognition was retired in Plan PR11; use Task Message and Handoff",
    ))
}

pub async fn list_agent_chat_turns(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(chat_id): Path<String>,
) -> ApiResult<Json<Vec<AgentChatTurnJobResponse>>> {
    state
        .agent_chat_history
        .get_authorized_chat(&user.user_id, &chat_id)
        .await?;
    let jobs = AgentChatTurnJobRepo::list_agent_chat_turn_jobs(&*state.db, &chat_id).await?;
    Ok(Json(jobs.into_iter().map(turn_response).collect()))
}

pub async fn cancel_agent_chat_turn(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path((_chat_id, _turn_id)): Path<(String, String)>,
    Json(_request): Json<CancelAgentChatTurnRequest>,
) -> ApiResult<Json<AgentChatTurnJobResponse>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "Agent Chat turns cannot be resumed or cancelled after the PR11 cutover",
    ))
}

pub async fn list_agent_handoffs(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(project_id): Path<String>,
) -> ApiResult<Json<Vec<AgentHandoffResponse>>> {
    require_project_member(&state, &project_id, &user.user_id).await?;
    let chat = AgentChatRepo::get_project_chat(&*state.db, &project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent_chat", project_id.clone()))?;
    let handoffs = AgentHandoffRepo::list_agent_handoffs(&*state.db, &chat.id).await?;
    let mut response = Vec::with_capacity(handoffs.len());
    for handoff in handoffs {
        response.push(handoff_response(&state, handoff).await?);
    }
    Ok(Json(response))
}

pub async fn get_agent_handoff(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((_project_id, handoff_id)): Path<(String, String)>,
) -> ApiResult<Json<AgentHandoffResponse>> {
    let handoff = AgentHandoffRepo::get_agent_handoff(&*state.db, &handoff_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent_handoff", handoff_id.clone()))?;
    let target = state
        .agent_chat_history
        .get_authorized_chat(&user.user_id, &handoff.target_chat_id)
        .await?;
    if target.project_id.as_deref() != Some(_project_id.as_str()) {
        return Err(ApiError::not_found("agent_handoff", handoff_id));
    }
    Ok(Json(handoff_response(&state, handoff).await?))
}

pub async fn create_agent_handoff(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_project_id): Path<String>,
    Json(_request): Json<CreateAgentHandoffRequest>,
) -> ApiResult<(StatusCode, Json<AgentHandoffResponse>)> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "Main-to-Project Agent handoffs were retired; use Task-scoped Handoff",
    ))
}

async fn require_project_member(
    state: &AppState,
    project_id: &str,
    user_id: &str,
) -> ApiResult<()> {
    ProjectMemberRepo::get_member(&*state.db, project_id, user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", project_id.to_owned()))?;
    Ok(())
}

async fn current_project_binding(
    state: &AppState,
    project_id: &str,
) -> Result<Option<ProjectAgentBinding>, ApiError> {
    if let Some(binding) =
        ProjectAgentBindingRepo::get_active_project_binding(&*state.db, project_id).await?
    {
        return Ok(Some(binding));
    }
    let mut history =
        ProjectAgentBindingRepo::list_project_binding_history(&*state.db, project_id).await?;
    Ok(history.pop())
}

async fn switcher_item(state: &AppState, chat: AgentChat) -> ApiResult<AgentChatSwitcherItem> {
    let kind = if chat.kind == "project" {
        AgentChatKind::Project
    } else {
        AgentChatKind::Main
    };
    let (project_name, binding_state, identity_id) = match kind {
        AgentChatKind::Main => {
            let binding = AccountMainAgentBindingRepo::get_active_main_binding(
                &*state.db,
                chat.account_id.as_deref().unwrap_or_default(),
            )
            .await?;
            (
                None,
                binding
                    .as_ref()
                    .map(|value| binding_state(&value.state))
                    .unwrap_or(AgentBindingState::SetupRequired),
                binding.map(|value| value.identity_id),
            )
        }
        AgentChatKind::Project => {
            let project_id = chat
                .project_id
                .as_deref()
                .ok_or_else(|| ApiError::not_found("project", chat.id.clone()))?;
            let project = ProjectRepo::get_by_id(&*state.db, project_id)
                .await?
                .ok_or_else(|| ApiError::not_found("project", project_id.to_owned()))?;
            let binding = current_project_binding(state, project_id).await?;
            (
                Some(project.name),
                binding
                    .as_ref()
                    .map(|value| binding_state(&value.state))
                    .unwrap_or(AgentBindingState::SetupRequired),
                binding.and_then(|value| value.identity_id),
            )
        }
    };
    let identity_name = match identity_id.as_deref() {
        Some(identity_id) => AgentRepo::get_by_id(&*state.db, identity_id)
            .await?
            .map(|identity| identity.name),
        None => None,
    };
    let pending_turn_count = AgentChatTurnJobRepo::list_agent_chat_turn_jobs(&*state.db, &chat.id)
        .await?
        .into_iter()
        .filter(|job| {
            matches!(
                job.status,
                AgentChatTurnState::Queued
                    | AgentChatTurnState::Leased
                    | AgentChatTurnState::RetryWait
            )
        })
        .count() as i64;
    Ok(AgentChatSwitcherItem {
        chat_id: chat.id,
        kind,
        project_id: chat.project_id,
        project_name,
        identity_id,
        identity_name,
        binding_state,
        chat_status: chat_status(&chat.status),
        unread_count: 0,
        pending_turn_count,
        last_message_at: chat.last_message_at,
    })
}

async fn main_binding_response(
    state: &AppState,
    binding: AccountMainAgentBinding,
) -> ApiResult<MainAgentBindingResponse> {
    let chat_id = AgentChatRepo::get_main_chat(&*state.db, &binding.account_id)
        .await?
        .map(|chat| chat.id)
        .ok_or_else(|| ApiError::not_found("agent_chat", binding.account_id.clone()))?;
    Ok(MainAgentBindingResponse {
        id: binding.id,
        account_id: binding.account_id,
        identity_id: binding.identity_id,
        profile_id: binding.profile_id,
        chat_id,
        state: binding_state(&binding.state),
        autonomy_policy: parse_json(&binding.autonomy_policy_json),
        tool_policy_revision: Some(binding.tool_policy_revision),
        version: binding.version,
        created_at: binding.created_at,
        updated_at: binding.updated_at,
    })
}

async fn project_binding_response(
    state: &AppState,
    binding: ProjectAgentBinding,
) -> ApiResult<ProjectAgentBindingResponse> {
    let chat_id = AgentChatRepo::get_project_chat(&*state.db, &binding.project_id)
        .await?
        .map(|chat| chat.id)
        .ok_or_else(|| ApiError::not_found("agent_chat", binding.project_id.clone()))?;
    Ok(ProjectAgentBindingResponse {
        id: binding.id,
        project_id: binding.project_id,
        identity_id: binding.identity_id,
        profile_id: binding.profile_id,
        chat_id,
        state: binding_state(&binding.state),
        permission_ceiling: parse_json(&binding.permission_ceiling_json),
        autonomy_policy: parse_json(&binding.autonomy_policy_json),
        subscriptions: serde_json::from_str(&binding.subscriptions_json).unwrap_or_default(),
        wake_budget: binding.wake_budget,
        version: binding.version,
        created_at: binding.created_at,
        updated_at: binding.updated_at,
    })
}

async fn pending_turn_count(state: &AppState, chat_id: &str) -> ApiResult<i64> {
    Ok(
        AgentChatTurnJobRepo::list_agent_chat_turn_jobs(&*state.db, chat_id)
            .await?
            .into_iter()
            .filter(|job| {
                matches!(
                    job.status,
                    AgentChatTurnState::Queued
                        | AgentChatTurnState::Leased
                        | AgentChatTurnState::RetryWait
                )
            })
            .count() as i64,
    )
}

fn chat_response(
    chat: AgentChat,
    pending_turn_count: i64,
    fallback_account_id: &str,
) -> AgentChatResponse {
    AgentChatResponse {
        id: chat.id,
        kind: if chat.kind == "project" {
            AgentChatKind::Project
        } else {
            AgentChatKind::Main
        },
        account_id: chat
            .account_id
            .unwrap_or_else(|| fallback_account_id.to_owned()),
        project_id: chat.project_id,
        title: if chat.kind == "project" {
            "Project Agent".to_owned()
        } else {
            "Main Agent".to_owned()
        },
        status: chat_status(&chat.status),
        message_count: chat.message_count,
        pending_turn_count,
        last_message_at: chat.last_message_at,
        version: chat.version,
        created_at: chat.created_at,
        updated_at: chat.updated_at,
    }
}

fn message_response(message: AgentChatMessage) -> AgentChatMessageResponse {
    let source_chat_id = serde_json::from_str::<Value>(&message.source_metadata_json)
        .ok()
        .and_then(|metadata| {
            metadata
                .get("source_chat_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    AgentChatMessageResponse {
        id: message.id,
        chat_id: message.chat_id,
        author_type: match message.author_type {
            DbMessageAuthorType::User => AgentChatMessageAuthorType::User,
            DbMessageAuthorType::Agent => AgentChatMessageAuthorType::Agent,
            DbMessageAuthorType::Handoff => AgentChatMessageAuthorType::Handoff,
            DbMessageAuthorType::System => AgentChatMessageAuthorType::System,
        },
        author_id: message.author_id,
        content: message.content,
        content_guard: parse_json(&message.content_guard_json),
        sensitivity: message.sensitivity,
        status: match message.status {
            DbMessageStatus::Complete => AgentChatMessageStatus::Complete,
            DbMessageStatus::Failed => AgentChatMessageStatus::Failed,
            DbMessageStatus::Cancelled => AgentChatMessageStatus::Cancelled,
        },
        outcome: message.outcome,
        model: message.model,
        profile_id: message.profile_id,
        session_id: message.session_id,
        context_manifest_id: message.context_manifest_id,
        token_usage_json: message.token_usage_json.map(|value| parse_json(&value)),
        duration_ms: message.duration_ms,
        error: message.error,
        correlation_id: message.correlation_id,
        causation_id: message.causation_id,
        handoff_id: message.handoff_id,
        source_chat_id,
        source_message_id: message.source_message_id,
        sequence: message.sequence,
        created_at: message.created_at,
    }
}

fn turn_response(job: AgentChatTurnJob) -> AgentChatTurnJobResponse {
    AgentChatTurnJobResponse {
        id: job.id,
        chat_id: job.chat_id,
        input_message_id: job.triggering_message_id,
        responder_identity_id: job.responder_identity_id,
        responder_profile_id: job.profile_id,
        status: match job.status {
            AgentChatTurnState::Queued => AgentChatTurnStatus::Queued,
            AgentChatTurnState::Leased => AgentChatTurnStatus::Leased,
            AgentChatTurnState::RetryWait => AgentChatTurnStatus::RetryWait,
            AgentChatTurnState::Succeeded => AgentChatTurnStatus::Succeeded,
            AgentChatTurnState::Failed => AgentChatTurnStatus::Failed,
            AgentChatTurnState::Cancelled => AgentChatTurnStatus::Cancelled,
        },
        attempt_count: job.attempt_count,
        max_attempts: job.max_attempts,
        lease_expires_at: job.leased_until,
        next_attempt_at: job.next_attempt_at,
        response_message_id: job.response_message_id,
        error: job.error_message.or(job.error_code),
        correlation_id: job.correlation_id,
        version: job.version,
        created_at: job.created_at,
        updated_at: job.updated_at,
    }
}

async fn handoff_response(
    state: &AppState,
    handoff: AgentHandoff,
) -> ApiResult<AgentHandoffResponse> {
    let target_project_id = AgentChatRepo::get_agent_chat(&*state.db, &handoff.target_chat_id)
        .await?
        .and_then(|chat| chat.project_id)
        .ok_or_else(|| ApiError::not_found("project", handoff.target_chat_id.clone()))?;
    let updated_at = handoff.updated_at.clone();
    Ok(AgentHandoffResponse {
        id: handoff.id,
        source_chat_id: handoff.source_chat_id,
        source_message_id: handoff.source_message_id,
        source_turn_job_id: handoff.source_turn_job_id,
        target_project_id,
        target_chat_id: handoff.target_chat_id,
        author_identity_id: handoff.author_identity_id,
        content: handoff.content,
        content_guard: parse_json(&handoff.content_guard_json),
        sensitivity: "internal".to_owned(),
        status: match handoff.status {
            db::AgentHandoffStatus::Pending => AgentHandoffStatus::Pending,
            db::AgentHandoffStatus::Delivered => AgentHandoffStatus::Delivered,
            db::AgentHandoffStatus::Failed => AgentHandoffStatus::Failed,
            db::AgentHandoffStatus::Cancelled => AgentHandoffStatus::Cancelled,
        },
        target_message_id: handoff.target_message_id,
        target_turn_job_id: handoff.target_turn_job_id,
        dedupe_key: handoff.dedupe_key,
        correlation_id: handoff.correlation_id,
        causation_id: handoff.causation_id,
        error: handoff.error_code,
        created_at: handoff.created_at,
        delivered_at: match handoff.status {
            db::AgentHandoffStatus::Delivered => Some(updated_at.clone()),
            _ => None,
        },
        updated_at,
    })
}

fn binding_state(value: &str) -> AgentBindingState {
    match value {
        "setup_required" | "agent_setup_required" => AgentBindingState::SetupRequired,
        "paused" | "suspended" => AgentBindingState::Paused,
        "revoked" => AgentBindingState::Revoked,
        _ => AgentBindingState::Active,
    }
}

fn chat_status(value: &str) -> AgentChatStatus {
    match value {
        "agent_setup_required" | "setup_required" => AgentChatStatus::SetupRequired,
        "archived" => AgentChatStatus::Archived,
        _ => AgentChatStatus::Ready,
    }
}

fn parse_json(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| json!({}))
}
