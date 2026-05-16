use api_types::AgentResponse;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use db::{
    new_uuid_v4, now_rfc3339, AgentRepo, CreateProjectAgentLink, ExecutionRepo,
    ProjectAgentLinkRepo, ProjectMember, ProjectMemberRepo,
};
use serde::{Deserialize, Serialize};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{agent_response, auth::AuthenticatedUser},
    state::AppState,
};

#[derive(Debug, Clone, Serialize)]
pub struct ProjectAgentLinkResponse {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub linked_by_user_id: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateProjectAgentLinkRequest {
    pub agent_id: String,
}

pub async fn list_project_agents(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(project_id): Path<String>,
) -> ApiResult<Json<Vec<AgentResponse>>> {
    require_project_member(&state, &project_id, &user.user_id).await?;
    let agents = state
        .db
        .list_agents_usable_in_project(&project_id, &user.user_id)
        .await
        .map_err(ApiError::from)?;

    let mut responses = Vec::with_capacity(agents.len());
    for agent in agents {
        let active_task_count = AgentRepo::count_active_tasks(&*state.db, &agent.id).await?;
        let stats = ExecutionRepo::stats_by_agent(&*state.db, &agent.id).await?;
        responses.push(agent_response(agent, Some(active_task_count), None, stats));
    }

    Ok(Json(responses))
}

pub async fn list_project_agent_links(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(project_id): Path<String>,
) -> ApiResult<Json<Vec<ProjectAgentLinkResponse>>> {
    require_project_member(&state, &project_id, &user.user_id).await?;
    let links = ProjectAgentLinkRepo::list_by_project(&*state.db, &project_id).await?;
    Ok(Json(
        links
            .into_iter()
            .map(map_project_agent_link_response)
            .collect(),
    ))
}

pub async fn create_project_agent_link(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(project_id): Path<String>,
    Json(body): Json<CreateProjectAgentLinkRequest>,
) -> ApiResult<(StatusCode, Json<ProjectAgentLinkResponse>)> {
    require_project_admin(&state, &project_id, &user.user_id).await?;

    let agent = AgentRepo::get_by_id(&*state.db, &body.agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent", body.agent_id.clone()))?;
    if agent.visibility != "global" && agent.owner_id.as_deref() != Some(&user.user_id) {
        return Err(ApiError::not_found("agent", body.agent_id));
    }

    let now = now_rfc3339();
    let link = ProjectAgentLinkRepo::create(
        &*state.db,
        CreateProjectAgentLink {
            id: new_uuid_v4(),
            project_id,
            agent_id: agent.id,
            linked_by_user_id: user.user_id,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .map_err(|e| match e {
        db::DbError::Check(msg) if msg.contains("already linked") => ApiError::conflict(
            "agent_already_linked",
            "Agent is already linked to this project",
        ),
        other => ApiError::from(other),
    })?;

    Ok((
        StatusCode::CREATED,
        Json(map_project_agent_link_response(link)),
    ))
}

pub async fn delete_project_agent_link(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((project_id, agent_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    require_project_admin(&state, &project_id, &user.user_id).await?;
    ProjectAgentLinkRepo::delete_by_project_and_agent(&*state.db, &project_id, &agent_id)
        .await
        .map_err(|e| match e {
            db::DbError::NotFound => ApiError::not_found("project_agent_link", agent_id),
            other => ApiError::from(other),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn require_project_member(
    state: &AppState,
    project_id: &str,
    user_id: &str,
) -> ApiResult<ProjectMember> {
    ProjectMemberRepo::get_member(&*state.db, project_id, user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", project_id.to_owned()))
}

pub async fn require_project_admin(
    state: &AppState,
    project_id: &str,
    user_id: &str,
) -> ApiResult<ProjectMember> {
    let member = require_project_member(state, project_id, user_id).await?;
    if member.role != "owner" && member.role != "admin" {
        return Err(ApiError::forbidden_with_code(
            "insufficient_role",
            "project owner or admin role is required",
        ));
    }
    Ok(member)
}

fn map_project_agent_link_response(link: db::ProjectAgentLink) -> ProjectAgentLinkResponse {
    ProjectAgentLinkResponse {
        id: link.id,
        project_id: link.project_id,
        agent_id: link.agent_id,
        linked_by_user_id: link.linked_by_user_id,
        created_at: link.created_at,
        updated_at: link.updated_at,
    }
}
