//! Transitional routes for historical embedded Agent records.
//!
//! Runtime operations fail closed until the broad public-surface retirement
//! in PR12. Profile and session history remain readable for audit.

use api_types::{
    AgentProfileResponse, AgentSessionResponse, ConnectEmbeddedProfileRequest,
    CreateAgentSessionRequest, CreateEmbeddedAgentRequest, SessionVersionRequest,
};
use axum::{
    extract::{Path, State},
    Json,
};
use db::{
    Agent, AgentProfile, AgentProfileRepo, AgentRepo, AgentSession, AgentSessionRepo,
    ExecutionRepo, SelectAgentProfile,
};
use services::agent_service::compute_effective_status;

use crate::{
    errors::{ApiError, ApiResult},
    routes::{agent_response, auth::AuthenticatedUser, redact_sensitive_config},
    state::AppState,
};

pub async fn create_embedded_agent(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Json(_request): Json<CreateEmbeddedAgentRequest>,
) -> ApiResult<Json<api_types::ConnectedEmbeddedAgentResponse>> {
    Err(retired_runtime_error())
}

pub async fn connect_embedded_profile(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_identity_id): Path<String>,
    Json(_request): Json<ConnectEmbeddedProfileRequest>,
) -> ApiResult<Json<api_types::ConnectedEmbeddedProfileResponse>> {
    Err(retired_runtime_error())
}

pub async fn list_profiles(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(identity_id): Path<String>,
) -> ApiResult<Json<Vec<AgentProfileResponse>>> {
    require_owned_identity(&state, &identity_id, &user.user_id).await?;
    let profiles = AgentProfileRepo::list_profiles(&*state.db, &identity_id).await?;
    Ok(Json(profiles.into_iter().map(profile_response).collect()))
}

pub async fn select_profile(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((identity_id, profile_id)): Path<(String, String)>,
    Json(request): Json<SessionVersionRequest>,
) -> ApiResult<Json<api_types::AgentResponse>> {
    require_owned_identity(&state, &identity_id, &user.user_id).await?;
    let profile = AgentProfileRepo::get_profile(&*state.db, &profile_id)
        .await?
        .filter(|profile| profile.identity_id == identity_id)
        .ok_or_else(|| ApiError::not_found("agent_profile", profile_id.clone()))?;
    if is_retired_profile(&profile) {
        return Err(retired_runtime_error());
    }
    let agent = AgentProfileRepo::select_profile(
        &*state.db,
        SelectAgentProfile {
            identity_id,
            profile_id,
            expected_version: request.version,
            updated_at: db::now_rfc3339(),
        },
    )
    .await?;
    Ok(Json(response_for_agent(&state, agent).await?))
}

pub async fn create_session(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_identity_id): Path<String>,
    Json(_request): Json<CreateAgentSessionRequest>,
) -> ApiResult<Json<AgentSessionResponse>> {
    Err(retired_runtime_error())
}

pub async fn list_sessions(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(identity_id): Path<String>,
) -> ApiResult<Json<Vec<AgentSessionResponse>>> {
    require_owned_identity(&state, &identity_id, &user.user_id).await?;
    let sessions = AgentSessionRepo::list_agent_sessions(&*state.db, &identity_id).await?;
    Ok(Json(sessions.into_iter().map(session_response).collect()))
}

pub async fn list_session_interactions(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_session_id): Path<String>,
) -> ApiResult<Json<Vec<api_types::ProtectedInteractionSummaryResponse>>> {
    Err(retired_runtime_error())
}

pub async fn answer_session_interaction(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path((_session_id, _interaction_id)): Path<(String, String)>,
    Json(_request): Json<api_types::ProtectedInteractionAnswerRequest>,
) -> ApiResult<Json<api_types::ProtectedInteractionSummaryResponse>> {
    Err(retired_runtime_error())
}

pub async fn cancel_session_interaction(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path((_session_id, _interaction_id)): Path<(String, String)>,
    Json(_request): Json<api_types::ProtectedInteractionCancelRequest>,
) -> ApiResult<Json<api_types::ProtectedInteractionSummaryResponse>> {
    Err(retired_runtime_error())
}

pub async fn rotate_session(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_session_id): Path<String>,
    Json(_request): Json<SessionVersionRequest>,
) -> ApiResult<Json<AgentSessionResponse>> {
    Err(retired_runtime_error())
}

pub async fn suspend_session(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_session_id): Path<String>,
    Json(_request): Json<SessionVersionRequest>,
) -> ApiResult<Json<AgentSessionResponse>> {
    Err(retired_runtime_error())
}

pub async fn resume_session(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_session_id): Path<String>,
    Json(_request): Json<SessionVersionRequest>,
) -> ApiResult<Json<AgentSessionResponse>> {
    Err(retired_runtime_error())
}

pub async fn cancel_session_turn(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_session_id): Path<String>,
) -> ApiResult<axum::http::StatusCode> {
    Err(retired_runtime_error())
}

pub async fn steer_session_turn(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_session_id): Path<String>,
    Json(_request): Json<api_types::SteerAgentSessionRequest>,
) -> ApiResult<axum::http::StatusCode> {
    Err(retired_runtime_error())
}

pub async fn effective_permissions(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_identity_id): Path<String>,
    Json(_scope): Json<api_types::CanonicalScopeRequest>,
) -> ApiResult<Json<api_types::EffectivePermissionsResponse>> {
    Err(retired_runtime_error())
}

async fn require_owned_identity(
    state: &AppState,
    identity_id: &str,
    user_id: &str,
) -> ApiResult<Agent> {
    AgentRepo::get_by_id(&*state.db, identity_id)
        .await?
        .filter(|agent| agent.owner_id.as_deref() == Some(user_id))
        .ok_or_else(|| ApiError::not_found("agent", identity_id.to_owned()))
}

fn is_retired_profile(profile: &AgentProfile) -> bool {
    profile.backend_kind == "native" || profile.executor_type == "embedded"
}

fn retired_runtime_error() -> ApiError {
    ApiError::bad_request_with_code(
        "agent_runtime.retired",
        "embedded Agent execution is retired; bind a new Agent to an available HarnessAdapter",
    )
}

async fn response_for_agent(state: &AppState, agent: Agent) -> ApiResult<api_types::AgentResponse> {
    let stats = ExecutionRepo::stats_by_agent(&*state.db, &agent.id).await?;
    let active_task_count = AgentRepo::count_active_tasks(&*state.db, &agent.id).await?;
    let effective_status = compute_effective_status(&state.db, &agent)
        .await?
        .as_str()
        .to_owned();
    Ok(agent_response(
        agent,
        Some(active_task_count),
        Some(effective_status),
        stats,
    ))
}

fn profile_response(profile: AgentProfile) -> AgentProfileResponse {
    AgentProfileResponse {
        id: profile.id,
        identity_id: profile.identity_id,
        backend_kind: profile.backend_kind,
        executor_type: profile.executor_type,
        provider: profile.provider,
        model: profile.model,
        reasoning_effort: profile.reasoning_effort,
        permission_policy: redact_profile_text(profile.permission_policy),
        system_prompt: redact_profile_text(profile.prompt_template),
        capabilities: redact_profile_value(parse_json(&profile.capabilities_json)),
        tool_policy: redact_profile_value(parse_json(&profile.tool_policy_json)),
        config: redact_profile_value(redact_sensitive_config(parse_json(&profile.config_json))),
        credential_handle_id: profile.credential_ref,
        version: profile.version,
        created_at: profile.created_at,
    }
}

fn redact_profile_text(value: Option<String>) -> Option<String> {
    value.map(|value| {
        if contains_protected_runtime_marker(&value) {
            "[redacted]".to_owned()
        } else {
            value
        }
    })
}

fn redact_profile_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(value) if contains_protected_runtime_marker(&value) => {
            serde_json::Value::String("[redacted]".to_owned())
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(redact_profile_value).collect())
        }
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let redacted = if is_sensitive_profile_key(&key) {
                        serde_json::Value::String("[redacted]".to_owned())
                    } else {
                        redact_profile_value(value)
                    };
                    (key, redacted)
                })
                .collect(),
        ),
        value => value,
    }
}

fn is_sensitive_profile_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace('-', "_");
    [
        "api_key",
        "token",
        "secret",
        "password",
        "authorization",
        "credential",
        "private_key",
    ]
    .iter()
    .any(|candidate| normalized == *candidate || normalized.contains(candidate))
}

fn contains_protected_runtime_marker(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let compact: String = lower
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    let has_bearer_marker = lower
        .split(|character: char| !character.is_ascii_alphabetic())
        .any(|word| word == "bearer");
    let has_github_token_marker = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"]
        .iter()
        .any(|marker| lower.contains(marker));
    let has_pem_marker = lower.contains("-----begin")
        && (lower.contains("private key") || lower.contains("openssh"));
    has_bearer_marker
        || compact.contains("bearer")
        || compact.contains("apikey")
        || lower.contains("sk-")
        || has_github_token_marker
        || has_pem_marker
}

fn session_response(session: AgentSession) -> AgentSessionResponse {
    AgentSessionResponse {
        id: session.id,
        identity_id: session.identity_id,
        profile_id: session.profile_id,
        context_scope_id: session.context_scope_id,
        backend_kind: session.backend_kind,
        status: session.status,
        capabilities: parse_json(&session.capabilities_json),
        connection_status: session.connection_status,
        predecessor_session_id: session.predecessor_session_id,
        replaced_by_session_id: session.replaced_by_session_id,
        last_activity_at: session.last_activity_at,
        version: session.version,
        created_at: session.created_at,
        updated_at: session.updated_at,
    }
}

fn parse_json(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or(serde_json::Value::Null)
}
