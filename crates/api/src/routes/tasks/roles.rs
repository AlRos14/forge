use super::*;
use crate::routes::auth::AuthenticatedUser;

pub async fn list_task_role_model(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<TaskRoleResponse>>> {
    require_task_visible(&state, &id, &user).await?;
    Ok(Json(
        crate::routes::task_roles_response(&state.db, &id, true).await?,
    ))
}

pub async fn create_task_role_model(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Json(request): Json<CreateTaskRoleRequest>,
) -> ApiResult<Json<TaskRoleResponse>> {
    ensure_task_role_access(&state.db, &id, &user.user_id).await?;
    let policy_json =
        serde_json::to_string(&request.policy.unwrap_or_else(|| serde_json::json!({})))?;
    let mode = to_db_coordination_mode(request.coordination_mode);
    state
        .task_service
        .create_task_role(&id, &request.role, mode, policy_json)
        .await
        .map_err(ApiError::from)?;
    let mut roles = crate::routes::task_roles_response(&state.db, &id, true).await?;
    let role = roles
        .drain(..)
        .find(|role| role.role == db::canonical_task_role_name(&request.role).unwrap_or_default())
        .ok_or_else(|| ApiError::internal("created TaskRole could not be loaded"))?;
    Ok(Json(role))
}

pub async fn update_task_role_model(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((id, role)): Path<(String, String)>,
    Json(request): Json<UpdateTaskRoleRequest>,
) -> ApiResult<Json<TaskRoleResponse>> {
    ensure_task_role_access(&state.db, &id, &user.user_id).await?;
    let mode = request.coordination_mode.map(to_db_coordination_mode);
    let policy_json = request
        .policy
        .map(|policy| serde_json::to_string(&policy))
        .transpose()?;
    state
        .task_service
        .update_task_role(&id, &role, request.expected_version, mode, policy_json)
        .await
        .map_err(ApiError::from)?;
    let canonical = db::canonical_task_role_name(&role).unwrap_or_default();
    let role = crate::routes::task_roles_response(&state.db, &id, true)
        .await?
        .into_iter()
        .find(|role_response| role_response.role == canonical)
        .ok_or_else(|| ApiError::internal("updated TaskRole could not be loaded"))?;
    Ok(Json(role))
}

pub async fn add_task_role_member(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((id, role)): Path<(String, String)>,
    Json(request): Json<AddRoleMembershipRequest>,
) -> ApiResult<Json<RoleMembershipResponse>> {
    let project_id = ensure_task_role_access(&state.db, &id, &user.user_id).await?;
    validate_actor_request(&state.db, &project_id, &user.user_id, &request.actor_ref).await?;
    let membership = state
        .task_service
        .add_task_role_member(&id, &role, request.actor_ref)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(crate::routes::role_membership_response(membership)))
}

pub async fn update_task_role_member(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((id, membership_id)): Path<(String, String)>,
    Json(request): Json<UpdateRoleMembershipRequest>,
) -> ApiResult<Json<RoleMembershipResponse>> {
    ensure_task_role_access(&state.db, &id, &user.user_id).await?;
    let status = match request.status {
        api_types::RoleMembershipStatus::Active => db::RoleMembershipStatus::Active,
        api_types::RoleMembershipStatus::Suspended => db::RoleMembershipStatus::Suspended,
        api_types::RoleMembershipStatus::Ended => db::RoleMembershipStatus::Ended,
    };
    let membership = state
        .task_service
        .update_task_role_member(&id, &membership_id, request.expected_version, status)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(crate::routes::role_membership_response(membership)))
}

async fn ensure_task_role_access(
    db: &db::SqliteDb,
    task_id: &str,
    user_id: &str,
) -> ApiResult<String> {
    let task = TaskRepo::get_by_id(db, task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.to_owned()))?;
    let project = ProjectRepo::get_by_id(db, &task.project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", task.project_id.clone()))?;
    if project.owner_id.is_none() || project.owner_id.as_deref() == Some(user_id) {
        return Ok(project.id);
    }
    if db::ProjectMemberRepo::get_member(db, &project.id, user_id)
        .await?
        .is_none()
    {
        return Err(ApiError::not_found("task", task_id.to_owned()));
    }
    Ok(project.id)
}

async fn validate_actor_request(
    db: &db::SqliteDb,
    project_id: &str,
    _requester_id: &str,
    actor_ref: &api_types::ActorRef,
) -> ApiResult<()> {
    match actor_ref {
        api_types::ActorRef::Agent(agent_id) => {
            let usable = db.list_agents_eligible_for_project(project_id).await?;
            if usable.into_iter().all(|agent| agent.id != *agent_id) {
                return Err(ApiError::not_found("agent", agent_id.clone()));
            }
        }
        api_types::ActorRef::Human(user_id) => {
            let project = ProjectRepo::get_by_id(db, project_id)
                .await?
                .ok_or_else(|| ApiError::not_found("project", project_id.to_owned()))?;
            if project.owner_id.as_deref() != Some(user_id)
                && db::ProjectMemberRepo::get_member(db, project_id, user_id)
                    .await?
                    .is_none()
            {
                return Err(ApiError::bad_request(
                    "human actor must be a project member",
                ));
            }
        }
    }
    Ok(())
}

fn to_db_coordination_mode(mode: api_types::CoordinationMode) -> db::CoordinationMode {
    match mode {
        api_types::CoordinationMode::Partitioned => db::CoordinationMode::Partitioned,
        api_types::CoordinationMode::Collaborative => db::CoordinationMode::Collaborative,
        api_types::CoordinationMode::Independent => db::CoordinationMode::Independent,
    }
}
