use api_types::{AgentProfileResponse, SelectAgentProfileRequest};
use axum::{
    extract::{Path, State},
    Json,
};
use db::{Agent, AgentProfile, AgentProfileRepo, AgentRepo, ExecutionRepo, SelectAgentProfile};
use services::agent_service::compute_effective_status;

use crate::{
    errors::{ApiError, ApiResult},
    routes::{agent_response, auth::AuthenticatedUser, redact_sensitive_config},
    state::AppState,
};

pub async fn list_profiles(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(identity_id): Path<String>,
) -> ApiResult<Json<Vec<AgentProfileResponse>>> {
    require_owned_identity(&state, &identity_id, &user.user_id).await?;
    let profiles = AgentProfileRepo::list_profiles(&*state.db, &identity_id).await?;
    Ok(Json(
        profiles
            .into_iter()
            .filter(|profile| !is_retired_profile(profile))
            .map(profile_response)
            .collect(),
    ))
}

pub async fn select_profile(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((identity_id, profile_id)): Path<(String, String)>,
    Json(request): Json<SelectAgentProfileRequest>,
) -> ApiResult<Json<api_types::AgentResponse>> {
    require_owned_identity(&state, &identity_id, &user.user_id).await?;
    let profile = AgentProfileRepo::get_profile(&*state.db, &profile_id)
        .await?
        .filter(|profile| profile.identity_id == identity_id)
        .ok_or_else(|| ApiError::not_found("agent_profile", profile_id.clone()))?;
    if is_retired_profile(&profile) {
        return Err(ApiError::bad_request_with_code(
            "agent_profile.runtime_retired",
            "native runtime profiles cannot be selected for current Agent work",
        ));
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

async fn require_owned_identity(
    state: &AppState,
    identity_id: &str,
    user_id: &str,
) -> ApiResult<Agent> {
    AgentRepo::get_by_id(&*state.db, identity_id)
        .await?
        .filter(|agent| {
            agent.backend_kind != "native"
                && agent.executor_type != "embedded"
                && agent.owner_id.as_deref() == Some(user_id)
        })
        .ok_or_else(|| ApiError::not_found("agent", identity_id.to_owned()))
}

fn is_retired_profile(profile: &AgentProfile) -> bool {
    profile.backend_kind == "native" || profile.executor_type == "embedded"
}

async fn response_for_agent(state: &AppState, agent: Agent) -> ApiResult<api_types::AgentResponse> {
    let stats = ExecutionRepo::stats_by_agent(&*state.db, &agent.id).await?;
    let active_execution_count = AgentRepo::count_running_executions(&*state.db, &agent.id).await?;
    let effective_status = compute_effective_status(&state.db, &agent)
        .await?
        .as_str()
        .to_owned();
    Ok(agent_response(
        agent,
        Some(active_execution_count),
        Some(effective_status),
        stats,
    ))
}

fn profile_response(profile: AgentProfile) -> AgentProfileResponse {
    AgentProfileResponse {
        id: profile.id,
        identity_id: profile.identity_id,
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

fn parse_json(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or(serde_json::Value::Null)
}
