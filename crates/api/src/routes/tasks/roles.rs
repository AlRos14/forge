use super::*;
use crate::routes::auth::AuthenticatedUser;

pub async fn list_task_role_model(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<TaskRoleResponse>>> {
    require_task_visible(&state, &id, &user).await?;
    Ok(Json(crate::routes::task_roles_response(&state.db, &id, true).await?))
}

pub async fn create_task_role_model(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Json(request): Json<CreateTaskRoleRequest>,
) -> ApiResult<Json<TaskRoleResponse>> {
    ensure_task_role_access(&state.db, &id, &user.user_id).await?;
    let policy_json = serde_json::to_string(&request.policy.unwrap_or_else(|| serde_json::json!({})))?;
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
        .update_task_role(
            &id,
            &role,
            request.expected_version,
            mode,
            policy_json,
        )
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

pub async fn assign_task_role(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((id, role_name)): Path<(String, String)>,
    Json(request): Json<AssignRoleRequest>,
) -> ApiResult<Json<TaskRoleAssignmentResponse>> {
    let project_id = validate_role_name(&state.db, &id, &role_name).await?;
    let assignee_id = required_body_field(request.assignee_id.clone(), "assignee_id")?;
    match request.assignee_type.as_str() {
        "agent" => {
            let usable_agents = state
                .db
                .list_agents_usable_in_project(&project_id, &user.user_id)
                .await
                .map_err(ApiError::from)?;
            let is_usable = usable_agents
                .into_iter()
                .any(|agent| agent.id == assignee_id);
            if !is_usable {
                return Err(ApiError::not_found("agent", assignee_id));
            }
        }
        "user" => {
            let member =
                db::ProjectMemberRepo::get_member(&*state.db, &project_id, &assignee_id).await?;
            if member.is_none() {
                return Err(ApiError::bad_request("assignee must be a project member"));
            }
        }
        _ => {}
    }
    let reset_workspace = request.reset_workspace.unwrap_or(false);
    let reset_worktree = request.reset_worktree.unwrap_or(false);
    let assignment = assign_role_input(id, role_name, request)?;
    let assignment = state
        .task_service
        .reassign_role(assignment, reset_workspace, reset_worktree)
        .await
        .map_err(role_reassignment_error)?;
    Ok(Json(task_role_assignment_response(assignment)))
}

pub async fn list_task_roles(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskRoleAssignmentListResponse>> {
    let items = TaskRoleAssignmentRepo::list_by_task(&*state.db, &id)
        .await?
        .into_iter()
        .map(task_role_assignment_response)
        .collect();
    Ok(Json(TaskRoleAssignmentListResponse { items }))
}

pub async fn remove_task_role(
    State(state): State<AppState>,
    Path((id, role_name)): Path<(String, String)>,
    body: Option<Json<RoleResetRequest>>,
) -> ApiResult<StatusCode> {
    validate_role_name(&state.db, &id, &role_name).await?;
    let reset = body.map(|Json(request)| request).unwrap_or_default();
    state
        .task_service
        .remove_role(
            &id,
            &role_name,
            reset.reset_workspace.unwrap_or(false),
            reset.reset_worktree.unwrap_or(false),
        )
        .await
        .map_err(role_reassignment_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) fn role_reassignment_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::InvalidOperation { message } => ApiError::invalid_operation_conflict(message),
        other => other.into(),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct RoleResetRequest {
    reset_workspace: Option<bool>,
    reset_worktree: Option<bool>,
}

#[derive(Serialize)]
pub struct TaskRoleAssignmentListResponse {
    pub items: Vec<TaskRoleAssignmentResponse>,
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
            let usable = db
                .list_agents_usable_in_project(project_id, _requester_id)
                .await?;
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
                return Err(ApiError::bad_request("human actor must be a project member"));
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

async fn validate_role_name(
    db: &db::SqliteDb,
    task_id: &str,
    role_name: &str,
) -> ApiResult<String> {
    let task = TaskRepo::get_by_id(db, task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.to_owned()))?;
    let project = ProjectRepo::get_by_id(db, &task.project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", task.project_id.clone()))?;
    let workflow = WorkflowEngine::resolve_workflow(&project.workflow_definition);
    if !workflow.roles.iter().any(|role| role.name == role_name) {
        return Err(ApiError::bad_request(format!(
            "role '{role_name}' is not defined in workflow"
        )));
    }
    Ok(task.project_id)
}

fn assign_role_input(
    task_id: String,
    role_name: String,
    request: AssignRoleRequest,
) -> ApiResult<CreateTaskRoleAssignment> {
    let assignee_type = match request.assignee_type.as_str() {
        "agent" => {
            let assignee_id = required_body_field(request.assignee_id, "assignee_id")?;
            (db::AssigneeKind::Agent, assignee_id)
        }
        "user" => {
            let assignee_id = required_body_field(request.assignee_id, "assignee_id")?;
            (db::AssigneeKind::User, assignee_id)
        }
        _ => {
            return Err(ApiError::bad_request(
                "assignee_type must be 'agent' or 'user'",
            ));
        }
    };
    let now = now_rfc3339();
    Ok(CreateTaskRoleAssignment {
        id: db::new_uuid_v4(),
        task_id,
        role_name,
        assignee_type: Some(assignee_type.0),
        assignee_id: Some(assignee_type.1),
        created_at: now.clone(),
        updated_at: now,
    })
}

fn required_body_field(value: Option<String>, field: &'static str) -> ApiResult<String> {
    let Some(value) = value else {
        return Err(ApiError::bad_request(format!("{field} is required")));
    };
    if value.trim().is_empty() {
        return Err(ApiError::bad_request(format!("{field} must not be empty")));
    }
    Ok(value)
}
