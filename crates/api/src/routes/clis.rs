use api_types::{CliProjectionAgent, CliProjectionItem, CliProjectionResponse, DetectedCli};
use axum::{
    extract::{Query, State},
    Json,
};
use db::{Agent, AgentListQuery, AgentRepo, Daemon, DaemonRepo, PageRequest, SortBy, SortOrder};
use serde::Deserialize;
use std::collections::HashMap;

use crate::{errors::ApiResult, routes::auth::RequireAdmin, state::AppState};

#[derive(Debug, Deserialize)]
pub struct CliProjectionParams {
    pub daemon_id: Option<String>,
}

pub async fn list_clis(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Query(params): Query<CliProjectionParams>,
) -> ApiResult<Json<CliProjectionResponse>> {
    let daemons = list_daemons(&state, params.daemon_id.as_deref()).await?;
    let agents_by_daemon_kind = agents_by_daemon_kind(&state).await?;
    let mut items = Vec::new();

    for daemon in daemons {
        let detected_clis: Vec<DetectedCli> =
            serde_json::from_str(&daemon.detected_clis_json).unwrap_or_default();
        for detected_cli in detected_clis {
            let agents = agents_by_daemon_kind
                .get(&(daemon.id.clone(), detected_cli.kind.clone()))
                .cloned()
                .unwrap_or_default();
            items.push(CliProjectionItem {
                daemon_id: daemon.id.clone(),
                daemon_hostname: daemon.hostname.clone(),
                daemon_status: daemon.status.to_string(),
                kind: detected_cli.kind,
                availability: detected_cli.availability,
                config_path: detected_cli.config_path,
                version: detected_cli.version,
                path: detected_cli.path,
                agents,
            });
        }
    }

    Ok(Json(CliProjectionResponse { items }))
}

async fn list_daemons(state: &AppState, daemon_id: Option<&str>) -> ApiResult<Vec<Daemon>> {
    if let Some(daemon_id) = daemon_id {
        let daemon = DaemonRepo::get_by_id(&*state.db, daemon_id).await?;
        return Ok(daemon.into_iter().collect());
    }

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

async fn agents_by_daemon_kind(
    state: &AppState,
) -> ApiResult<HashMap<(String, String), Vec<CliProjectionAgent>>> {
    let mut grouped: HashMap<(String, String), Vec<CliProjectionAgent>> = HashMap::new();
    for agent in list_agents(state).await? {
        let Some(daemon_id) = agent.daemon_id.clone() else {
            continue;
        };
        grouped
            .entry((daemon_id, agent.executor_type.clone()))
            .or_default()
            .push(CliProjectionAgent {
                id: agent.id,
                name: agent.name,
                executor_type: agent.executor_type,
                effective_status: None,
            });
    }
    Ok(grouped)
}

async fn list_agents(state: &AppState) -> ApiResult<Vec<Agent>> {
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
