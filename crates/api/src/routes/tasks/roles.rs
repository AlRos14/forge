use super::*;
use crate::routes::auth::AuthenticatedUser;

pub async fn task_role_capability_catalog() -> Json<api_types::TaskRoleCapabilityCatalog> {
    use api_types::{TaskRoleCapability as Capability, TaskRoleTemplate as Template};
    Json(api_types::TaskRoleCapabilityCatalog {
        schema_version: 1,
        capabilities: vec![
            Capability::FormalReview,
            Capability::Orchestration,
            Capability::RequestRework,
        ],
        templates: vec![
            Template {
                id: "planner".to_owned(),
                label: "Planificador".to_owned(),
                default_role: "planner".to_owned(),
                description: "Sugiere instrucciones y divide el trabajo en WorkUnits ordinarios."
                    .to_owned(),
                default_capabilities: vec![],
            },
            Template {
                id: "implementer".to_owned(),
                label: "Implementador".to_owned(),
                default_role: "implementer".to_owned(),
                description: "Realiza trabajo ordinario dentro del alcance de sus WorkUnits."
                    .to_owned(),
                default_capabilities: vec![],
            },
            Template {
                id: "reviewer".to_owned(),
                label: "Revisor".to_owned(),
                default_role: "reviewer".to_owned(),
                description: "Produce ReviewReports sobre sujetos exactos.".to_owned(),
                default_capabilities: vec![Capability::FormalReview],
            },
            Template {
                id: "orchestrator".to_owned(),
                label: "Orquestador".to_owned(),
                default_role: "orchestrator".to_owned(),
                description: "Participa en las operaciones tipadas de orquestación ya soportadas."
                    .to_owned(),
                default_capabilities: vec![Capability::Orchestration],
            },
            Template {
                id: "custom".to_owned(),
                label: "Personalizado".to_owned(),
                default_role: String::new(),
                description: "Empieza sin capacidades privilegiadas; añade solo las necesarias."
                    .to_owned(),
                default_capabilities: vec![],
            },
        ],
    })
}

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
    let task = super::require_task_mutable(&state, &id, &user).await?;
    if !request.configuration.capabilities.is_empty() || request.configuration.rework.is_some() {
        crate::routes::projects::require_project_admin(&state, &task.project_id, &user.user_id)
            .await?;
    }
    let policy_json =
        serde_json::to_string(&request.policy.unwrap_or_else(|| serde_json::json!({})))?;
    validate_rework_role_references(&state, &id, &request.configuration).await?;
    let configuration_json =
        services::task_role_configuration::store_task_role_configuration(&request.configuration)
            .map_err(ApiError::from)?;
    let mode = to_db_coordination_mode(request.coordination_mode);
    state
        .task_service
        .create_task_role_with_configuration(
            &id,
            &request.role,
            mode,
            policy_json,
            configuration_json,
        )
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
    let task = super::require_task_mutable(&state, &id, &user).await?;
    let mode = request.coordination_mode.map(to_db_coordination_mode);
    let canonical = db::canonical_task_role_name(&role)
        .ok_or_else(|| ApiError::bad_request("role is not a TaskRole"))?;
    let current = TaskRoleRepo::get_by_task_and_role(&*state.db, &id, &canonical)
        .await?
        .ok_or_else(|| ApiError::not_found("task_role", canonical.clone()))?;
    let current_configuration_record =
        db::TaskRoleCapabilityConfigurationRepo::get_by_task_role_id(&*state.db, &current.id)
            .await?
            .ok_or_else(|| ApiError::internal("TaskRole capability configuration is missing"))?;
    let current_configuration = services::task_role_configuration::parse_task_role_configuration(
        &current_configuration_record.configuration_json,
    )
    .map_err(ApiError::from)?;
    let proposed_configuration = request
        .configuration
        .as_ref()
        .unwrap_or(&current_configuration);
    let capability_change = request
        .configuration
        .as_ref()
        .is_some_and(|configuration| configuration != &current_configuration);
    let orchestration_policy_change = request.policy.is_some()
        && (current_configuration
            .capabilities
            .contains(&api_types::TaskRoleCapability::Orchestration)
            || proposed_configuration
                .capabilities
                .contains(&api_types::TaskRoleCapability::Orchestration));
    if capability_change || orchestration_policy_change {
        crate::routes::projects::require_project_admin(&state, &task.project_id, &user.user_id)
            .await?;
    }
    let policy_needs_write = request.policy.is_some() || request.configuration.is_some();
    let mut policy = serde_json::from_str::<serde_json::Value>(&current.policy_json)?;
    if let Some(replacement) = request.policy {
        policy = replacement;
    }
    let policy_json = policy_needs_write
        .then(|| serde_json::to_string(&policy))
        .transpose()?;
    let configuration_json = if request.configuration.is_some() {
        let configuration = request.configuration.unwrap_or(current_configuration);
        validate_rework_role_references(&state, &id, &configuration).await?;
        services::task_role_configuration::store_task_role_configuration(&configuration)
            .map_err(ApiError::from)?
    } else {
        current_configuration_record.configuration_json
    };
    state
        .task_service
        .update_task_role_with_configuration(
            &id,
            &role,
            request.expected_version,
            mode,
            policy_json,
            configuration_json,
        )
        .await
        .map_err(ApiError::from)?;
    let role = crate::routes::task_roles_response(&state.db, &id, true)
        .await?
        .into_iter()
        .find(|role_response| role_response.role == canonical)
        .ok_or_else(|| ApiError::internal("updated TaskRole could not be loaded"))?;
    Ok(Json(role))
}

async fn validate_rework_role_references(
    state: &AppState,
    task_id: &str,
    configuration: &api_types::TaskRoleConfiguration,
) -> ApiResult<()> {
    let Some(rework) = configuration.rework.as_ref() else {
        return Ok(());
    };
    let roles = TaskRoleRepo::list_by_task(&*state.db, task_id).await?;
    let by_id = roles
        .iter()
        .map(|role| (role.id.as_str(), role))
        .collect::<std::collections::HashMap<_, _>>();
    for role_id in rework
        .allowed_destination_role_ids
        .iter()
        .chain(rework.next_review_role_id.iter())
    {
        if !by_id.contains_key(role_id.as_str()) {
            return Err(ApiError::bad_request(format!(
                "rework route references a TaskRole outside this Task: {role_id}"
            )));
        }
    }
    if let Some(review_role_id) = rework.next_review_role_id.as_deref() {
        let review_role = by_id[review_role_id];
        let review_configuration = db::TaskRoleCapabilityConfigurationRepo::get_by_task_role_id(
            &*state.db,
            &review_role.id,
        )
        .await?
        .ok_or_else(|| {
            ApiError::internal("next review TaskRole capability configuration is missing")
        })?;
        let capabilities = services::task_role_configuration::parse_task_role_configuration(
            &review_configuration.configuration_json,
        )
        .map_err(ApiError::from)?
        .capabilities;
        if !capabilities.contains(&api_types::TaskRoleCapability::FormalReview) {
            return Err(ApiError::bad_request(
                "next review TaskRole must have formal_review capability",
            ));
        }
    }
    Ok(())
}

pub async fn add_task_role_member(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((id, role)): Path<(String, String)>,
    Json(request): Json<AddRoleMembershipRequest>,
) -> ApiResult<Json<RoleMembershipResponse>> {
    let task = super::require_task_mutable(&state, &id, &user).await?;
    validate_actor_request(
        &state.db,
        &task.project_id,
        &user.user_id,
        &request.actor_ref,
    )
    .await?;
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
    super::require_task_mutable(&state, &id, &user).await?;
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
            if project.owner_id.is_some()
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
