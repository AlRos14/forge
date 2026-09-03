use api_types::{
    AgentAvailabilityResponse, AgentResponse, CreateAgentRequest, DiscoveredDaemonResponse,
    DiscoveredOptionsResponse, DuplicateAgentRequest, PaginatedResponse, UpdateAgentRequest,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use db::{
    new_uuid_v4, now_rfc3339, Agent, AgentListQuery, AgentRepo, AgentTaskListQuery, DaemonRepo,
    ExecutionRepo, TaskRepo, UpdateAgent,
};
use events::{event_timestamp, EventContext, ForgeEvent};
use executors::{DiscoverContext, ExecutorKind};
use serde_json::Value;
use services::agent_service::{compute_effective_status, resolve_daemon_for_agent};
use sqlx::Row;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    routes::{
        agent_response, page_request, parse_csv, serialize_json, task_page_request,
        task_response_light, ListParams,
    },
    state::AppState,
};

pub async fn register_agent(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(request): Json<CreateAgentRequest>,
) -> ApiResult<Json<AgentResponse>> {
    if !user.is_admin && request.daemon_id.is_some() {
        return Err(ApiError::forbidden_with_code(
            "admin_required",
            "Admin access required to pin an agent to a daemon",
        ));
    }

    // A harness agent may reference one of the caller's provider entries; the
    // capability matrix decides whether that entry can drive this executor.
    let credential_ref = if let Some(credential_id) = request.credential_id.as_deref() {
        let entry = state
            .embedded_agent_service
            .require_owned_entry(&user.user_id, credential_id)
            .await?;
        services::provider_authorization::runtime_supported(
            &entry.provider,
            &entry.credential_method,
            &request.executor_type,
        )
        .map_err(ApiError::bad_request)?;
        Some(entry.id)
    } else {
        None
    };

    let agent = state
        .agent_service
        .register(
            request.name,
            request.description,
            request.executor_type,
            request.model,
            request.reasoning_effort,
            request.permission_policy,
            request.prompt_template,
            serialize_json(request.capabilities)?.unwrap_or_else(|| "[]".to_owned()),
            serialize_json(request.config_json)?.unwrap_or_else(|| "{}".to_owned()),
            credential_ref,
            request.daemon_id,
            request.max_concurrent_tasks,
            request.heartbeat_interval_seconds,
            request.max_missed_heartbeats,
            request.is_default.unwrap_or(false),
            Some(user.user_id.clone()),
            Some("account".to_owned()),
        )
        .await?;
    let response = build_agent_response_for_user(&state, agent, Some(0), &user).await?;
    Ok(Json(response))
}

pub async fn list_agents(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<PaginatedResponse<AgentResponse>>> {
    let statuses = parse_csv::<db::AgentStatus>(params.status.as_ref(), "status")?;
    let status = statuses.into_iter().next();
    let capabilities = params
        .capabilities
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|item| !item.trim().is_empty())
        .map(|item| item.trim().to_owned())
        .collect();
    let page = AgentRepo::list(
        &*state.db,
        AgentListQuery {
            status,
            executor_type: params.executor_type.clone(),
            capabilities,
            page: page_request(&params)?,
        },
    )
    .await?;
    let has_more = page.next_cursor.is_some();
    let mut items = Vec::with_capacity(page.items.len());
    for agent in page.items {
        if !can_view_agent(&agent, &user) {
            continue;
        }
        let active_task_count = AgentRepo::count_active_tasks(&*state.db, &agent.id).await?;
        items.push(
            build_agent_response_for_user(&state, agent, Some(active_task_count), &user).await?,
        );
    }
    Ok(Json(PaginatedResponse {
        items,
        next_cursor: page.next_cursor,
        has_more,
        total_count: page.total_count.and_then(|count| u64::try_from(count).ok()),
    }))
}

pub async fn get_agent(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<AgentResponse>> {
    let agent = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    require_agent_visible(&agent, &user, &id)?;
    let active_task_count = AgentRepo::count_active_tasks(&*state.db, &agent.id).await?;
    Ok(Json(
        build_agent_response_for_user(&state, agent, Some(active_task_count), &user).await?,
    ))
}

pub async fn get_agent_usage(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<api_types::AgentUsageResponse>> {
    agent_usage_response(&state, &user, &id).await.map(Json)
}

pub async fn refresh_agent_usage(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<api_types::AgentUsageResponse>> {
    let agent = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    require_agent_visible(&agent, &user, &id)?;
    let usage = match agent.executor_type.as_str() {
        "codex" => Some(services::account_usage::refresh_codex_usage(&agent.config_json).await?),
        "cursor" => Some(services::account_usage::refresh_cursor_usage().await?),
        _ => None,
    };
    if let Some(usage) = usage {
        let account_key = usage_account_key(&agent);
        let captured_at = now_rfc3339();
        let stale_after = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        sqlx::query(
            "INSERT INTO account_usage_snapshot
             (id, account_key, executor_type, daemon_id, source, usage_json, captured_at, stale_after)
             VALUES (?, ?, ?, ?, 'manual_refresh', ?, ?, ?)",
        ).bind(new_uuid_v4()).bind(account_key).bind(&agent.executor_type)
            .bind(agent.daemon_id.as_deref())
            .bind(usage.to_string()).bind(captured_at).bind(stale_after)
            .execute(state.db.pool()).await?;
    }
    agent_usage_response(&state, &user, &id).await.map(Json)
}

async fn agent_usage_response(
    state: &AppState,
    user: &AuthenticatedUser,
    id: &str,
) -> ApiResult<api_types::AgentUsageResponse> {
    let agent = AgentRepo::get_by_id(&*state.db, id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.to_owned()))?;
    require_agent_visible(&agent, user, id)?;
    let account_key = usage_account_key(&agent);
    let row = sqlx::query(
        "SELECT source, usage_json, captured_at, stale_after FROM account_usage_snapshot
         WHERE account_key = ? ORDER BY captured_at DESC LIMIT 1",
    )
    .bind(&account_key)
    .fetch_optional(state.db.pool())
    .await?;
    let Some(row) = row else {
        return Ok(api_types::AgentUsageResponse {
            available: false,
            executor_type: agent.executor_type,
            account_key,
            shared_account: agent.daemon_id.is_none(),
            source: None,
            usage: None,
            captured_at: None,
            stale: true,
            message: Some("This harness has not reported account quota yet".to_owned()),
        });
    };
    let stale_after: String = row.get("stale_after");
    Ok(api_types::AgentUsageResponse {
        available: true,
        executor_type: agent.executor_type,
        account_key,
        shared_account: agent.daemon_id.is_none(),
        source: Some(row.get("source")),
        usage: serde_json::from_str::<Value>(&row.get::<String, _>("usage_json")).ok(),
        captured_at: Some(row.get("captured_at")),
        stale: stale_after < now_rfc3339(),
        message: None,
    })
}

fn usage_account_key(agent: &Agent) -> String {
    let config: Value = serde_json::from_str(&agent.config_json).unwrap_or(Value::Null);
    let kind = agent.executor_type.parse::<ExecutorKind>();
    let mut key = kind
        .map(|kind| executors::account_key(&kind, &config))
        .unwrap_or_else(|_| agent.executor_type.clone());
    if let Some(daemon_id) = agent.daemon_id.as_deref() {
        key.push('@');
        key.push_str(daemon_id);
    }
    key
}

pub async fn list_agent_tasks(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<PaginatedResponse<api_types::TaskResponse>>> {
    let agent = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    require_agent_visible(&agent, &user, &id)?;
    let page = TaskRepo::list_by_executing_agent(
        &*state.db,
        AgentTaskListQuery {
            agent_id: id,
            include_archived: params.include_archived.unwrap_or(false),
            include_cancelled: params.include_cancelled.unwrap_or(false),
            include_deleted: false,
            page: task_page_request(&params)?,
        },
    )
    .await?;
    let has_more = page.next_cursor.is_some();
    let mut items = Vec::with_capacity(page.items.len());
    for task in page.items {
        items.push(task_response_light(&state.db, task).await?);
    }
    Ok(Json(PaginatedResponse {
        items,
        next_cursor: page.next_cursor,
        has_more,
        total_count: page.total_count.and_then(|count| u64::try_from(count).ok()),
    }))
}

pub async fn update_agent(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Json(request): Json<UpdateAgentRequest>,
) -> ApiResult<Json<AgentResponse>> {
    let existing = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    require_agent_manageable(&existing, &user, &id)?;
    if !user.is_admin
        && request
            .daemon_id
            .as_ref()
            .and_then(|id| id.as_ref())
            .is_some()
    {
        return Err(ApiError::forbidden_with_code(
            "admin_required",
            "Admin access required to pin an agent to a daemon",
        ));
    }

    let agent = AgentRepo::update(
        &*state.db,
        UpdateAgent {
            id,
            expected_version: request.version,
            name: request.name,
            description: request.description,
            model: request.model,
            reasoning_effort: request.reasoning_effort,
            permission_policy: request.permission_policy,
            capabilities_json: request
                .capabilities
                .map(|capabilities| serialize_json(Some(capabilities)))
                .transpose()?
                .flatten(),
            config_json: serialize_json(request.config_json)?,
            daemon_id: request.daemon_id,
            max_concurrent_tasks: request.max_concurrent_tasks,
            heartbeat_interval_seconds: None,
            max_missed_heartbeats: None,
            status: None,
            last_heartbeat_at: None,
            is_default: request.is_default,
            paused: request.paused,
            prompt_template: request.prompt_template,
            updated_at: now_rfc3339(),
        },
    )
    .await?;
    let active_task_count = AgentRepo::count_active_tasks(&*state.db, &agent.id).await?;
    Ok(Json(
        build_agent_response_for_user(&state, agent, Some(active_task_count), &user).await?,
    ))
}

pub async fn archive_agent(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let agent = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    require_agent_manageable(&agent, &user, &id)?;
    state.agent_service.archive(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn pause_agent(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<AgentResponse>> {
    let agent = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    require_agent_manageable(&agent, &user, &id)?;
    if agent.paused {
        let response = build_agent_response_for_user(
            &state,
            agent,
            Some(AgentRepo::count_active_tasks(&*state.db, &id).await?),
            &user,
        )
        .await?;
        return Ok(Json(response));
    }

    AgentRepo::set_paused(&*state.db, &id, true).await?;
    let agent = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    tracing::info!(agent_id = %agent.id, agent_name = %agent.name, "agent paused");
    state.event_bus.publish(ForgeEvent {
        event_type: "agent.paused".to_owned(),
        entity_id: agent.id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::AgentPaused {},
    });
    let active_task_count = AgentRepo::count_active_tasks(&*state.db, &agent.id).await?;
    let response =
        build_agent_response_for_user(&state, agent, Some(active_task_count), &user).await?;
    Ok(Json(response))
}

pub async fn resume_agent(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<AgentResponse>> {
    let agent = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    require_agent_manageable(&agent, &user, &id)?;
    if !agent.paused {
        let response = build_agent_response_for_user(
            &state,
            agent,
            Some(AgentRepo::count_active_tasks(&*state.db, &id).await?),
            &user,
        )
        .await?;
        return Ok(Json(response));
    }

    AgentRepo::set_paused(&*state.db, &id, false).await?;
    let agent = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    tracing::info!(agent_id = %agent.id, agent_name = %agent.name, "agent resumed");
    state.event_bus.publish(ForgeEvent {
        event_type: "agent.resumed".to_owned(),
        entity_id: agent.id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::AgentResumed {},
    });
    let active_task_count = AgentRepo::count_active_tasks(&*state.db, &agent.id).await?;
    let response =
        build_agent_response_for_user(&state, agent, Some(active_task_count), &user).await?;
    Ok(Json(response))
}

pub async fn duplicate_agent(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Json(request): Json<DuplicateAgentRequest>,
) -> ApiResult<Json<AgentResponse>> {
    let existing = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    require_agent_manageable(&existing, &user, &id)?;
    let agent =
        AgentRepo::duplicate_agent(&*state.db, &id, new_uuid_v4(), request.name, now_rfc3339())
            .await?;
    let active_task_count = AgentRepo::count_active_tasks(&*state.db, &agent.id).await?;
    Ok(Json(
        build_agent_response_for_user(&state, agent, Some(active_task_count), &user).await?,
    ))
}

pub async fn agent_availability(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<AgentAvailabilityResponse>> {
    let agent = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    require_agent_visible(&agent, &user, &id)?;
    let active_task_count = AgentRepo::count_active_tasks(&*state.db, &agent.id).await?;
    let effective_status = compute_effective_status(&state.db, &agent).await?;
    let resolved_daemon = resolve_daemon_for_agent(&state.db, &agent).await.ok();
    let available = effective_status.as_str() == "active" || effective_status.as_str() == "busy";
    let reason = if available {
        None
    } else {
        Some(match effective_status.as_str() {
            "daemon_unavailable" => format!(
                "No daemon with authenticated {} executor found",
                agent.executor_type
            ),
            "daemon_offline" => "Pinned daemon is offline".to_owned(),
            "deactivated" => "Pinned daemon does not have this executor authenticated".to_owned(),
            "connection_degraded" => "Embedded provider connection is degraded".to_owned(),
            "connection_unavailable" => "Embedded provider connection is unavailable".to_owned(),
            status => format!("Agent is not available: {status}"),
        })
    };
    Ok(Json(AgentAvailabilityResponse {
        available,
        effective_status: effective_status.as_str().to_owned(),
        resolved_daemon_id: if user.is_admin {
            resolved_daemon.map(|daemon| daemon.id)
        } else {
            None
        },
        active_task_count,
        max_concurrent_tasks: agent.max_concurrent_tasks,
        reason,
    }))
}

pub async fn agent_discovered_options(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Query(params): Query<DiscoveryParams>,
) -> ApiResult<Json<DiscoveredOptionsResponse>> {
    let agent = AgentRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", id.clone()))?;
    require_agent_visible(&agent, &user, &id)?;
    let kind = parse_executor_kind(&agent.executor_type)?;
    let adapter = state.adapter_registry.get(&kind).ok_or_else(|| {
        ApiError::bad_request(format!(
            "No adapter registered for executor type: {}",
            agent.executor_type
        ))
    })?;
    let daemons = if user.is_admin {
        DaemonRepo::list_available_for_executor(&*state.db, &agent.executor_type).await?
    } else {
        Vec::new()
    };
    let discovered = if daemons.is_empty() {
        Default::default()
    } else {
        adapter
            .discover_options(DiscoverContext {
                project_path: params.project_id,
            })
            .await
            .map_err(|error| ApiError::bad_request(error.to_string()))?
    };
    Ok(Json(DiscoveredOptionsResponse {
        models: discovered.models,
        permission_policies: discovered.permission_policies,
        cli_specific: discovered.cli_specific,
        available_daemons: daemons
            .into_iter()
            .map(|daemon| DiscoveredDaemonResponse {
                id: daemon.id,
                name: daemon.hostname,
                status: daemon.status.to_string(),
            })
            .collect(),
        warning: None,
    }))
}

async fn build_agent_response(
    state: &AppState,
    agent: Agent,
    active_task_count: Option<i64>,
) -> ApiResult<AgentResponse> {
    let effective_status = compute_effective_status(&state.db, &agent)
        .await?
        .as_str()
        .to_owned();
    let stats = ExecutionRepo::stats_by_agent(&*state.db, &agent.id).await?;
    Ok(agent_response(
        agent,
        active_task_count,
        Some(effective_status),
        stats,
    ))
}

async fn build_agent_response_for_user(
    state: &AppState,
    agent: Agent,
    active_task_count: Option<i64>,
    user: &AuthenticatedUser,
) -> ApiResult<AgentResponse> {
    let mut response = build_agent_response(state, agent, active_task_count).await?;
    if !user.is_admin {
        response.daemon_id = None;
    }
    Ok(response)
}

#[derive(Debug, serde::Deserialize)]
pub struct DiscoveryParams {
    pub project_id: Option<String>,
}

fn parse_executor_kind(value: &str) -> ApiResult<ExecutorKind> {
    value
        .parse()
        .map_err(|_| ApiError::bad_request(format!("invalid executor_type: {value}")))
}

fn can_view_agent(agent: &Agent, user: &AuthenticatedUser) -> bool {
    agent.visibility == "global" || agent.owner_id.as_deref() == Some(&user.user_id)
}

fn can_manage_agent(agent: &Agent, user: &AuthenticatedUser) -> bool {
    agent.owner_id.as_deref() == Some(&user.user_id)
}

fn require_agent_visible(agent: &Agent, user: &AuthenticatedUser, id: &str) -> ApiResult<()> {
    if can_view_agent(agent, user) {
        Ok(())
    } else {
        Err(ApiError::not_found("agent", id.to_owned()))
    }
}

fn require_agent_manageable(agent: &Agent, user: &AuthenticatedUser, id: &str) -> ApiResult<()> {
    if can_manage_agent(agent, user) {
        Ok(())
    } else {
        Err(ApiError::not_found("agent", id.to_owned()))
    }
}
