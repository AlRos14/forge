//! Provider entries: the account's configured provider connections, plus the
//! CLI-managed runtimes discovered on connected daemons. Entries own the
//! credentials; agents reference entries and are managed separately.

use std::collections::HashMap;

use api_types::{
    AgentProviderCapabilitiesResponse, CliRuntimeEntryResponse, CreateProviderEntryRequest,
    DetectedCli, DisconnectCredentialResponse, ProviderEntriesResponse, ProviderEntryAgentRef,
    ProviderEntryResponse, ProviderEntryTestResponse, ProviderRevocationStatus,
    ProviderUsageResponse, ProviderUsageWindow, RenameProviderEntryRequest, SessionVersionRequest,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::{
    now_rfc3339, AgentConnectionHealthRepo, AgentListQuery, AgentRepo, CredentialHandle,
    CredentialHandleRepo, CredentialUsage, Daemon, DaemonRepo, DbError, PageRequest, SortBy,
    SortOrder, UpsertAgentConnectionHealth,
};
use forge_agent_host::{AgentHostError, CredentialRevocationOutcome, Secret};
use serde_json::Value;
use services::embedded_agent_service::ConnectApiKeyCredential;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};

pub async fn provider_catalog(
    State(state): State<AppState>,
    _user: AuthenticatedUser,
) -> Json<AgentProviderCapabilitiesResponse> {
    Json(state.provider_authorization_service.capabilities())
}

pub async fn list_providers(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> ApiResult<Json<ProviderEntriesResponse>> {
    let handles = CredentialHandleRepo::list_credential_handles(&*state.db, &user.user_id).await?;
    let usage = CredentialHandleRepo::list_credential_usage(&*state.db, &user.user_id).await?;
    let mut usage_by_credential: HashMap<String, Vec<CredentialUsage>> = HashMap::new();
    for row in usage {
        usage_by_credential
            .entry(row.credential_id.clone())
            .or_default()
            .push(row);
    }
    let items = handles
        .into_iter()
        .map(|handle| {
            let usage = usage_by_credential.remove(&handle.id).unwrap_or_default();
            entry_response(handle, usage)
        })
        .collect();
    let cli_runtimes = cli_runtime_entries(&state).await?;
    Ok(Json(ProviderEntriesResponse {
        items,
        cli_runtimes,
    }))
}

pub async fn create_provider_entry(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(request): Json<CreateProviderEntryRequest>,
) -> ApiResult<Json<ProviderEntryResponse>> {
    let provider = provider_name(request.provider);
    let handle = state
        .embedded_agent_service
        .connect_api_key_credential(ConnectApiKeyCredential {
            owner_user_id: user.user_id,
            provider: provider.to_owned(),
            label: request.label,
            credential: Secret::new(request.credential),
            base_url: request.base_url,
        })
        .await?;
    Ok(Json(entry_response(handle, Vec::new())))
}

pub async fn test_provider_entry(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<ProviderEntryTestResponse>> {
    let outcome = state
        .embedded_agent_service
        .test_provider_entry(&user.user_id, &id)
        .await?;
    Ok(Json(ProviderEntryTestResponse {
        status: if outcome.ok { "ok" } else { "failed" }.to_owned(),
        latency_ms: outcome.latency_ms,
        message: outcome.message,
        checked_at: outcome.checked_at,
    }))
}

pub async fn usage_provider_entry(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<ProviderUsageResponse>> {
    let outcome = state
        .embedded_agent_service
        .usage_provider_entry(&user.user_id, &id)
        .await?;
    Ok(Json(ProviderUsageResponse {
        id,
        provider: outcome.provider,
        source: if outcome.probed { "probe" } else { "unknown" }.to_owned(),
        plan_type: outcome.plan_type,
        windows: outcome
            .windows
            .into_iter()
            .map(|window| ProviderUsageWindow {
                id: window.id,
                used_percent: window.used_percent,
                window_minutes: window.window_minutes,
                resets_at: window.resets_at,
            })
            .collect(),
        fetched_at: outcome.fetched_at,
        detail: outcome.detail,
    }))
}

pub async fn rename_provider_entry(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Json(request): Json<RenameProviderEntryRequest>,
) -> ApiResult<Json<ProviderEntryResponse>> {
    if request.label.trim().is_empty() {
        return Err(ApiError::bad_request("label is required"));
    }
    let handle = CredentialHandleRepo::rename_credential_handle(
        &*state.db,
        &id,
        &user.user_id,
        request.label.trim(),
        request.version,
        &now_rfc3339(),
    )
    .await
    .map_err(|error| match error {
        DbError::VersionConflict => ApiError::conflict_with_code(
            "provider_entry.version_conflict",
            "provider entry changed before it could be renamed",
        ),
        _ => ApiError::not_found("provider_entry", id.clone()),
    })?;
    let usage = CredentialHandleRepo::list_credential_usage(&*state.db, &user.user_id)
        .await?
        .into_iter()
        .filter(|row| row.credential_id == handle.id)
        .collect();
    Ok(Json(entry_response(handle, usage)))
}

pub async fn delete_provider_entry(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(handle_id): Path<String>,
    Query(request): Query<SessionVersionRequest>,
) -> ApiResult<Json<DisconnectCredentialResponse>> {
    let affected: Vec<CredentialUsage> =
        CredentialHandleRepo::list_credential_usage(&*state.db, &user.user_id)
            .await?
            .into_iter()
            .filter(|row| row.credential_id == handle_id)
            .collect();
    let outcome = state
        .embedded_agent_service
        .protected_store()
        .revoke_credential_at_version(&handle_id, &user.user_id, request.version, &now_rfc3339())
        .await
        .map_err(|error| match error {
            AgentHostError::VersionConflict => ApiError::conflict_with_code(
                "provider_entry.version_conflict",
                "provider entry changed before it could be disconnected",
            ),
            _ => ApiError::not_found("provider_entry", handle_id.clone()),
        })?;
    // Make dependents visibly unhealthy right away instead of waiting for the
    // next probe. Bindings are never silently reassigned.
    let now = now_rfc3339();
    for row in &affected {
        if let Ok(Some(agent)) = AgentRepo::get_by_id(&*state.db, &row.agent_id).await {
            let _ = AgentConnectionHealthRepo::upsert_connection_health(
                &*state.db,
                UpsertAgentConnectionHealth {
                    profile_id: agent.profile_id,
                    status: "unavailable".to_owned(),
                    capability_status_json: agent.capabilities_json,
                    checked_at: Some(now.clone()),
                    error_code: Some("credential_revoked".to_owned()),
                    updated_at: now.clone(),
                },
            )
            .await;
        }
    }
    Ok(Json(DisconnectCredentialResponse {
        id: handle_id,
        status: "revoked".to_owned(),
        provider_revocation: match outcome {
            CredentialRevocationOutcome::NotSupported => ProviderRevocationStatus::NotSupported,
            CredentialRevocationOutcome::Succeeded => ProviderRevocationStatus::Succeeded,
            CredentialRevocationOutcome::Failed => ProviderRevocationStatus::Failed,
        },
        affected_agents: affected.into_iter().map(usage_ref).collect(),
    }))
}

fn entry_response(handle: CredentialHandle, usage: Vec<CredentialUsage>) -> ProviderEntryResponse {
    let metadata = serde_json::from_str::<Value>(&handle.metadata_json).unwrap_or(Value::Null);
    let base_url = metadata
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let provider_account_id = metadata
        .get("provider_account_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let last_used_at = usage
        .iter()
        .filter_map(|row| row.last_used_at.as_deref())
        .max()
        .map(str::to_owned);
    ProviderEntryResponse {
        id: handle.id,
        provider: handle.provider,
        label: handle.label,
        credential_method: handle.credential_method,
        status: handle.status,
        base_url,
        provider_account_id,
        used_by: usage.into_iter().map(usage_ref).collect(),
        last_used_at,
        version: handle.version,
        created_at: handle.created_at,
        updated_at: handle.updated_at,
    }
}

fn usage_ref(row: CredentialUsage) -> ProviderEntryAgentRef {
    ProviderEntryAgentRef {
        agent_id: row.agent_id,
        agent_name: row.agent_name,
        runtime: if row.runtime == "embedded" {
            "direct".to_owned()
        } else {
            row.runtime
        },
    }
}

fn provider_name(provider: api_types::AgentProviderId) -> &'static str {
    match provider {
        api_types::AgentProviderId::OpenAi => "openai",
        api_types::AgentProviderId::XAi => "xai",
        api_types::AgentProviderId::Gemini => "gemini",
        api_types::AgentProviderId::OpenRouter => "openrouter",
        api_types::AgentProviderId::OpenAiCompatible => "openai_compatible",
    }
}

fn cli_login_hint(kind: &str) -> Option<&'static str> {
    match kind {
        "claude_code" => Some("Run `claude` on the host and complete its login"),
        "codex" => Some("Run `codex login` on the host"),
        "cursor" => Some("Run `cursor-agent login` on the host"),
        "gemini" => Some("Run `gemini` on the host and complete its login"),
        "opencode" => Some("Run `opencode auth login` on the host"),
        _ => None,
    }
}

async fn cli_runtime_entries(state: &AppState) -> ApiResult<Vec<CliRuntimeEntryResponse>> {
    let daemons = all_daemons(state).await?;
    let mut used_by: HashMap<String, Vec<ProviderEntryAgentRef>> = HashMap::new();
    for agent in all_agents(state).await? {
        if agent.backend_kind != "cli" || agent.credential_ref.is_some() {
            continue;
        }
        used_by
            .entry(agent.executor_type.clone())
            .or_default()
            .push(ProviderEntryAgentRef {
                agent_id: agent.id,
                agent_name: agent.name,
                runtime: agent.executor_type,
            });
    }
    let mut items = Vec::new();
    for daemon in daemons {
        let detected: Vec<DetectedCli> =
            serde_json::from_str(&daemon.detected_clis_json).unwrap_or_default();
        for cli in detected {
            items.push(CliRuntimeEntryResponse {
                daemon_id: daemon.id.clone(),
                daemon_hostname: Some(daemon.hostname.clone()),
                daemon_status: daemon.status.to_string(),
                availability: cli.availability,
                version: cli.version,
                login_hint: cli_login_hint(&cli.kind).map(str::to_owned),
                used_by: used_by.get(&cli.kind).cloned().unwrap_or_default(),
                kind: cli.kind,
            });
        }
    }
    Ok(items)
}

async fn all_daemons(state: &AppState) -> ApiResult<Vec<Daemon>> {
    let mut daemons = Vec::new();
    let mut cursor = None;
    loop {
        let page = DaemonRepo::list(
            &*state.db,
            PageRequest {
                cursor,
                limit: 500,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Asc,
            },
        )
        .await?;
        daemons.extend(page.items);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(daemons)
}

async fn all_agents(state: &AppState) -> ApiResult<Vec<db::Agent>> {
    let mut agents = Vec::new();
    let mut cursor = None;
    loop {
        let page = AgentRepo::list(
            &*state.db,
            AgentListQuery {
                status: None,
                executor_type: None,
                capabilities: Vec::new(),
                page: PageRequest {
                    cursor,
                    limit: 500,
                    include_total: false,
                    sort_by: SortBy::CreatedAt,
                    sort_order: SortOrder::Asc,
                },
            },
        )
        .await?;
        agents.extend(page.items);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(agents)
}
