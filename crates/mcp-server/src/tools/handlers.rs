use std::sync::Arc;

use api_types::{
    parse_project_hooks_json, Actor, CoordinationMode as PublicCoordinationMode,
    ExecutionPurpose as PublicExecutionPurpose, UserActionSource,
};
use db::{
    new_uuid_v4, now_rfc3339, AgentListQuery, AgentProfileRepo, AgentRepo, ArtifactKind,
    CollaborationRepo, CreateProject, ExecutionPurpose, ExecutionRepo, GateRepo, GateScopeKind,
    PageRequest, ProjectMemberRepo, ProjectRepo, RoleMembershipRepo, SortBy, SortOrder, Task,
    TaskDependencyRepo, TaskLifecycleRepo, TaskListQuery, TaskRepo, TaskRoleRepo, UpdateProject,
    UpdateTask, ValidationRunRepo,
};
use executors::ExecutionOverrides;
use serde_json::{json, Map, Value};
use services::{
    gate_engine::GateEngine,
    task_lifecycle::{LifecycleCause, TaskLifecycleService, TransitionLifecycleInput},
    DiffService,
};

use crate::{
    error::McpToolError,
    params::{
        page_request, parse_params, task_page_request, AddTaskDependencyParams,
        AddTaskRoleMemberParams, CollaborationListParams, CreateProjectParams,
        CreateSubTasksParams, CreateTaskParams, CreateTaskRoleParams, EvidenceParams,
        GateEvaluationParams, GateIdParams, GateTaskParams, GetProjectParams, GetTaskParams,
        ListAgentProfilesParams, ListAgentsParams, ListExecutionsParams, ListProjectsParams,
        ListTaskDependenciesParams, ListTasksParams, RegisterAgentParams,
        RemoveTaskDependencyParams, ReviewExecutionParams, ReviewTaskParams,
        ReviseGatePolicyParams, StartExecutionParams, TransitionTaskLifecycleParams,
        UpdateProjectHooksParams, UpdateProjectParams, UpdateTaskParams, ValidationRunParams,
        ValidationTaskParams,
    },
    protocol::McpContext,
    state::AppState,
    values::{
        agent_page_value, agent_profile_value, agent_value, execution_page_value, execution_value,
        project_page_value, project_value,
    },
};

pub(super) async fn forge_create_task(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    validate_create_task_arguments(&params)?;
    let params: CreateTaskParams = parse_params(params)?;
    if params.project_id.trim().is_empty() {
        return Err(invalid_field_error(
            "project_id",
            "must be a non-empty string",
            Some(json!({
                "type": "string",
                "non_empty": true
            })),
        ));
    }
    if params.title.trim().is_empty() {
        return Err(invalid_field_error(
            "title",
            "must be a non-empty string",
            Some(json!({
                "type": "string",
                "non_empty": true
            })),
        ));
    }
    if let Some(parent_task_id) = params.parent_task_id.as_deref() {
        if parent_task_id.trim().is_empty() {
            return Err(invalid_field_error(
                "parent_task_id",
                "must be a non-empty string when provided",
                Some(json!({
                    "type": "string",
                    "non_empty": true
                })),
            ));
        }
    }
    if ProjectRepo::get_by_id(&*state.db, &params.project_id)
        .await?
        .is_none()
    {
        return Err(invalid_field_error(
            "project_id",
            "must reference an existing project",
            Some(json!({
                "type": "string",
                "constraint": "existing project id"
            })),
        ));
    }
    let task = state
        .task_service
        .create_task(
            params.project_id,
            params.title,
            params.description,
            params.parent_task_id,
            params.priority,
            params.task_type,
            None,
            None,
            None,
        )
        .await
        .map_err(|error| match error {
            services::ServiceError::NotFound { entity: "task", id } => invalid_field_error(
                "parent_task_id",
                format!("parent task not found: {id}"),
                Some(json!({
                    "type": "string",
                    "constraint": "existing root task id"
                })),
            ),
            other => other.into(),
        })?;
    task_public_value(state, task).await
}

pub(super) async fn forge_create_sub_tasks(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: CreateSubTasksParams = parse_params(params)?;
    let inputs = params
        .subtasks
        .into_iter()
        .map(|s| services::NewSubtaskInput {
            title: s.title,
            description: s.description,
            assignee_id: None,
        })
        .collect::<Vec<_>>();
    let tasks = state
        .task_service
        .create_subtasks(params.parent_task_id, inputs)
        .await?;
    let mut task_values = Vec::with_capacity(tasks.len());
    for task in tasks {
        task_values.push(task_public_value(state, task).await?);
    }
    Ok(serde_json::json!({ "subtasks": task_values }))
}

fn invalid_field_error(
    field: &'static str,
    message: impl Into<String>,
    accepted: Option<Value>,
) -> McpToolError {
    let mut data = json!({
        "field": field,
        "details": message.into(),
    });
    if let Some(accepted) = accepted {
        if let Some(object) = data.as_object_mut() {
            object.insert("accepted".to_owned(), accepted);
        }
    }
    McpToolError::new(-32602, "invalid params").with_data(data)
}

fn validate_create_task_arguments(params: &Value) -> Result<(), McpToolError> {
    let Some(object) = params.as_object() else {
        return Err(
            McpToolError::new(-32602, "invalid params").with_data(json!({
                "details": "tool arguments must be an object"
            })),
        );
    };

    if !object.contains_key("project_id") {
        return Err(invalid_field_error(
            "project_id",
            "is required",
            Some(json!({
                "type": "string",
                "non_empty": true
            })),
        ));
    }
    if !object.contains_key("title") {
        return Err(invalid_field_error(
            "title",
            "is required",
            Some(json!({
                "type": "string",
                "non_empty": true
            })),
        ));
    }

    if let Some(value) = object.get("project_id") {
        if !value.is_string() {
            return Err(invalid_field_error(
                "project_id",
                "must be a string",
                Some(json!({ "type": "string" })),
            ));
        }
    }
    if let Some(value) = object.get("title") {
        if !value.is_string() {
            return Err(invalid_field_error(
                "title",
                "must be a string",
                Some(json!({ "type": "string" })),
            ));
        }
    }
    if let Some(value) = object.get("parent_task_id") {
        if !value.is_string() {
            return Err(invalid_field_error(
                "parent_task_id",
                "must be a string",
                Some(json!({ "type": "string" })),
            ));
        }
    }
    if let Some(value) = object.get("priority") {
        if !value.is_i64() {
            return Err(invalid_field_error(
                "priority",
                "must be an integer in the accepted i64 range",
                Some(json!({
                    "type": "integer",
                    "min": i64::MIN,
                    "max": i64::MAX
                })),
            ));
        }
    }
    if let Some(value) = object.get("type") {
        let Some(task_type) = value.as_str() else {
            return Err(invalid_field_error(
                "type",
                "must be one of the accepted values",
                Some(json!({
                    "type": "string",
                    "enum": ["implementation", "planning", "discovery", "review", "validation"]
                })),
            ));
        };
        if task_type != "implementation"
            && task_type != "planning"
            && task_type != "discovery"
            && task_type != "review"
            && task_type != "validation"
        {
            return Err(invalid_field_error(
                "type",
                format!("unsupported value `{task_type}`"),
                Some(json!({
                    "type": "string",
                    "enum": ["implementation", "planning", "discovery", "review", "validation"]
                })),
            ));
        }
    }

    Ok(())
}

pub(super) async fn forge_list_tasks(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: ListTasksParams = parse_params(params)?;
    let page = TaskRepo::list(
        &*state.db,
        TaskListQuery {
            project_id: params.project_id,
            q: None,
            lifecycle_states: params.lifecycle_state.into_vec(),
            statuses: Vec::new(),
            agent_ids: Vec::new(),
            assignee_types: Vec::new(),
            assignee_ids: Vec::new(),
            priority: None,
            include_archived: false,
            include_cancelled: true,
            include_deleted: false,
            page: task_page_request(params.cursor, params.limit, params.sort_by)?,
        },
    )
    .await?;
    let mut task_values = Vec::with_capacity(page.items.len());
    for task in page.items {
        task_values.push(task_public_value(state, task).await?);
    }
    let has_more = page.next_cursor.is_some();
    Ok(json!({
        "items": task_values,
        "next_cursor": page.next_cursor,
        "has_more": has_more,
        "total_count": page.total_count,
    }))
}

pub(super) async fn forge_get_task(state: &AppState, params: Value) -> Result<Value, McpToolError> {
    let params: GetTaskParams = parse_params(params)?;
    let task = TaskRepo::get_by_id(&*state.db, &params.task_id, false)
        .await?
        .ok_or_else(|| McpToolError::not_found("task", params.task_id))?;
    task_public_value(state, task).await
}

/// Recovery hints are persisted diagnostics, not session authority. Filter
/// `ResumeSession` only in this response projection using the shared
/// Execution/HarnessSession authority; never rewrite the stored Task.
pub(super) async fn forge_get_task_diff(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: GetTaskParams = parse_params(params)?;
    let diff = DiffService::new(std::sync::Arc::clone(&state.db))
        .task_diff(&params.task_id)
        .await?;
    serde_json::to_value(diff)
        .map_err(|error| McpToolError::new(-32603, format!("failed to serialize diff: {error}")))
}

pub(super) async fn forge_list_executions(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: ListExecutionsParams = parse_params(params)?;
    let page = ExecutionRepo::list_by_task(
        &*state.db,
        &params.task_id,
        page_request(params.cursor, params.limit, None)?,
    )
    .await?;
    Ok(execution_page_value(page))
}

pub(super) async fn forge_start_execution(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: StartExecutionParams = parse_params(params)?;
    let user_id = authenticated_user(context)?;
    let task = TaskRepo::get_by_id(&*state.db, &params.task_id, false)
        .await?
        .ok_or_else(|| McpToolError::not_found("task", params.task_id.clone()))?;
    let project = ProjectRepo::get_by_id(&*state.db, &task.project_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("project", task.project_id.clone()))?;
    if project
        .owner_id
        .as_deref()
        .is_some_and(|owner_id| owner_id != user_id)
        && db::ProjectMemberRepo::get_member(&*state.db, &project.id, user_id)
            .await?
            .is_none()
    {
        return Err(McpToolError::not_found("task", params.task_id));
    }
    if params.role.trim().is_empty() || params.prompt.trim().is_empty() {
        return Err(McpToolError::new(
            -32602,
            "role and prompt must be non-empty for an Execution",
        ));
    }
    let purpose = match params.purpose {
        PublicExecutionPurpose::Plan => ExecutionPurpose::Plan,
        PublicExecutionPurpose::Implement => ExecutionPurpose::Implement,
        PublicExecutionPurpose::Review => ExecutionPurpose::Review,
        PublicExecutionPurpose::Validate => ExecutionPurpose::Validate,
        PublicExecutionPurpose::Investigate => ExecutionPurpose::Investigate,
        PublicExecutionPurpose::Orchestrate => ExecutionPurpose::Orchestrate,
        PublicExecutionPurpose::General => ExecutionPurpose::General,
    };
    let execution = state
        .task_service
        .dispatch_initial_role_execution_with_artifacts(
            &params.task_id,
            &params.agent_id,
            &params.role,
            purpose,
            params.prompt,
            params.input_artifact_ids,
            None,
        )
        .await?;
    Ok(execution_value(execution))
}

pub(super) async fn forge_update_task(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: UpdateTaskParams = parse_params(params)?;
    let task = TaskRepo::update(
        &*state.db,
        UpdateTask {
            id: params.task_id,
            expected_version: params.version,
            title: params.title,
            description: params.description.map(Some),
            priority: params.priority,
            merge_config: None,
            error_annotation: None,
            blocked_json: None,
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await?;
    task_public_value(state, task).await
}

pub(super) async fn forge_register_agent(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: RegisterAgentParams = parse_params(params)?;
    let agent = state
        .agent_service
        .register(
            params.name,
            None,
            params.executor_type,
            None,
            None,
            None,
            None,
            "[]".to_owned(),
            "{}".to_owned(),
            None,
            params.daemon_id,
            None,
            None,
            None,
            false,
            None,
            None,
        )
        .await?;
    Ok(agent_value(agent))
}

pub(super) async fn forge_list_agents(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: ListAgentsParams = parse_params(params)?;
    let page = AgentRepo::list(
        &*state.db,
        AgentListQuery {
            status: params.status.map(Into::into),
            executor_type: None,
            capabilities: Vec::new(),
            harness_only: true,
            page: page_request(params.cursor, params.limit, None)?,
        },
    )
    .await?;
    Ok(agent_page_value(page))
}

pub(super) async fn forge_list_projects(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: ListProjectsParams = parse_params(params)?;
    let page =
        ProjectRepo::list(&*state.db, page_request(params.cursor, params.limit, None)?).await?;
    project_page_value(page)
}

pub(super) async fn forge_create_project(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: CreateProjectParams = parse_params(params)?;
    if params.name.trim().is_empty() {
        return Err(McpToolError::new(-32602, "name must not be empty"));
    }
    let now = now_rfc3339();
    let project = ProjectRepo::create(
        &*state.db,
        CreateProject {
            id: new_uuid_v4(),
            name: params.name,
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_string(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await?;
    project_value(project)
}

pub(super) async fn forge_get_project(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: GetProjectParams = parse_params(params)?;
    let project = ProjectRepo::get_by_id(&*state.db, &params.project_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("project", params.project_id))?;
    project_value(project)
}

pub(super) async fn forge_update_project(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: UpdateProjectParams = parse_params(params)?;
    if params
        .name
        .as_deref()
        .is_some_and(|name| name.trim().is_empty())
    {
        return Err(McpToolError::new(-32602, "name must not be empty"));
    }

    let project = ProjectRepo::update(
        &*state.db,
        UpdateProject {
            id: params.project_id,
            name: params.name,
            settings: None,
            primary_repo_id: None,
            paused_at: params.paused.map(|paused| paused.then(now_rfc3339)),
            updated_at: now_rfc3339(),
        },
    )
    .await?;
    project_value(project)
}

pub(super) async fn forge_update_project_hooks(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: UpdateProjectHooksParams = parse_params(params)?;
    let project_hooks = serde_json::to_string(&params.project_hooks)
        .map_err(|error| McpToolError::new(-32603, format!("serialize project hooks: {error}")))?;
    parse_project_hooks_json(&project_hooks).map_err(|error| McpToolError::new(-32602, error))?;
    let project = ProjectRepo::update_at_version(
        &*state.db,
        UpdateProject {
            id: params.project_id,
            name: None,
            settings: None,
            primary_repo_id: None,
            paused_at: None,
            updated_at: now_rfc3339(),
        },
        params.version,
        Some(project_hooks),
    )
    .await?;
    project_value(project)
}

pub(super) async fn forge_follow_up_execution(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params = params
        .as_object()
        .ok_or_else(|| McpToolError::new(-32602, "params must be an object"))?;
    let execution_id = required_string_param(params, "execution_id")?;
    let message = required_string_param(params, "message")?;
    let agent_id = required_string_param(params, "agent_id")?;
    let overrides = optional_overrides_param(params, "overrides")?;

    let launched = state
        .task_service
        .follow_up_execution(execution_id, message, Some(agent_id), overrides)
        .await?;
    let task = task_public_value(state, launched.task).await?;

    Ok(json!({
        "task": task,
        "execution": execution_value(launched.execution),
        "workspace": {
            "id": launched.workspace.id,
            "task_id": launched.workspace.task_id,
            "repo_id": launched.workspace.repo_id,
            "worktree_path": launched.workspace.worktree_path,
            "branch": launched.workspace.branch,
            "status": launched.workspace.status.to_string(),
            "before_sha": launched.workspace.before_sha,
            "error": launched.workspace.error,
            "created_at": launched.workspace.created_at,
            "updated_at": launched.workspace.updated_at,
        },
    }))
}

fn required_string_param(
    params: &Map<String, Value>,
    key: &'static str,
) -> Result<String, McpToolError> {
    match params.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(McpToolError::new(-32602, format!("{key} must be a string"))),
        None => Err(McpToolError::new(-32602, format!("{key} is required"))),
    }
}

fn optional_overrides_param(
    params: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<ExecutionOverrides>, McpToolError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let Value::Object(overrides) = value else {
        return Err(McpToolError::new(
            -32602,
            format!("{key} must be an object"),
        ));
    };
    Ok(Some(ExecutionOverrides {
        model_id: optional_overrides_field(overrides, "model_id")?,
        reasoning_effort: optional_overrides_field(overrides, "reasoning_effort")?,
        permission_policy: optional_overrides_field(overrides, "permission_policy")?,
    }))
}

fn optional_overrides_field(
    overrides: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<String>, McpToolError> {
    match overrides.get(key) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(McpToolError::new(
            -32602,
            format!("overrides.{key} must be a string"),
        )),
    }
}

pub(super) async fn forge_add_task_dependency(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: AddTaskDependencyParams = parse_params(params)?;
    TaskDependencyRepo::add_dependency(
        &*state.db,
        &params.task_id,
        &params.depends_on_id,
        &now_rfc3339(),
    )
    .await?;
    Ok(json!({ "task_id": params.task_id, "depends_on_id": params.depends_on_id }))
}

pub(super) async fn forge_remove_task_dependency(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: RemoveTaskDependencyParams = parse_params(params)?;
    TaskDependencyRepo::remove_dependency(&*state.db, &params.task_id, &params.depends_on_id)
        .await?;
    Ok(json!({ "task_id": params.task_id, "depends_on_id": params.depends_on_id }))
}

pub(super) async fn forge_list_task_dependencies(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: ListTaskDependenciesParams = parse_params(params)?;
    let deps = TaskDependencyRepo::list_dependencies(&*state.db, &params.task_id).await?;
    Ok(json!({ "task_id": params.task_id, "depends_on": deps }))
}

pub(super) async fn forge_list_agent_profiles(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: ListAgentProfilesParams = parse_params(params)?;
    require_owned_identity(state, context, &params.identity_id).await?;
    let profiles = AgentProfileRepo::list_profiles(&*state.db, &params.identity_id).await?;
    Ok(json!({
        "items": profiles.into_iter().map(agent_profile_value).collect::<Vec<_>>(),
    }))
}

async fn require_owned_identity(
    state: &AppState,
    context: &McpContext,
    identity_id: &str,
) -> Result<db::Agent, McpToolError> {
    let actor_user_id = authenticated_user(context)?;
    let identity = AgentRepo::get_by_id(&*state.db, identity_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("agent_identity", identity_id.to_owned()))?;
    if identity.owner_id.as_deref() != Some(actor_user_id) {
        // Do not reveal whether another account owns the identity.
        return Err(McpToolError::not_found(
            "agent_identity",
            identity_id.to_owned(),
        ));
    }
    Ok(identity)
}

fn authenticated_user(context: &McpContext) -> Result<&str, McpToolError> {
    context
        .user_id
        .as_deref()
        .ok_or_else(|| McpToolError::new(-32003, "authenticated server identity is required"))
}

async fn task_public_value(state: &AppState, task: Task) -> Result<Value, McpToolError> {
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*state.db, &task.id)
        .await?
        .ok_or_else(|| McpToolError::not_found("task_lifecycle", task.id.clone()))?;
    let mut roles = Vec::new();
    for role in TaskRoleRepo::list_by_task(&*state.db, &task.id).await? {
        let members = RoleMembershipRepo::list_by_role(&*state.db, &role.id, false).await?;
        roles.push(json!({
            "id": role.id,
            "task_id": role.task_id,
            "role": role.role,
            "coordination_mode": role.coordination_mode.map(|mode| mode.to_string()),
            "policy": serde_json::from_str::<Value>(&role.policy_json).unwrap_or(Value::Null),
            "version": role.version,
            "members": members.into_iter().map(|member| json!({
                "id": member.id,
                "task_role_id": member.task_role_id,
                "actor_ref": member.actor_ref(),
                "status": member.status.to_string(),
                "version": member.version,
                "created_at": member.created_at,
                "updated_at": member.updated_at,
                "ended_at": member.ended_at,
            })).collect::<Vec<_>>(),
            "created_at": role.created_at,
            "updated_at": role.updated_at,
        }));
    }
    Ok(json!({
        "id": task.id,
        "project_id": task.project_id,
        "repo_id": task.repo_id,
        "parent_task_id": task.parent_task_id,
        "title": task.title,
        "description": task.description,
        "task_type": task.task_type,
        "lifecycle": {
            "task_id": lifecycle.task_id,
            "state": lifecycle.state.to_string(),
            "version": lifecycle.version,
            "reason_kind": lifecycle.reason_kind,
            "reason_ref": lifecycle.reason_ref,
            "created_at": lifecycle.created_at,
            "updated_at": lifecycle.updated_at,
        },
        "priority": task.priority,
        "board_position": task.board_position,
        "subtask_order": task.subtask_order,
        "task_roles": roles,
        "version": task.version,
        "created_at": task.created_at,
        "updated_at": task.updated_at,
    }))
}

pub(super) async fn forge_get_task_lifecycle(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: GetTaskParams = parse_params(params)?;
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*state.db, &params.task_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("task_lifecycle", params.task_id))?;
    serde_json::to_value(lifecycle).map_err(|error| {
        McpToolError::new(-32603, format!("failed to serialize lifecycle: {error}"))
    })
}

pub(super) async fn forge_list_task_lifecycle_transitions(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: GetTaskParams = parse_params(params)?;
    let transitions =
        TaskLifecycleRepo::list_task_lifecycle_transitions(&*state.db, &params.task_id, 100)
            .await?;
    Ok(json!({
        "items": transitions.into_iter().map(transition_fact_value).collect::<Vec<_>>(),
    }))
}

fn transition_fact_value(transition: db::TaskLifecycleTransitionFact) -> Value {
    json!({
        "id": transition.id,
        "task_id": transition.task_id,
        "from_state": transition.from_state.to_string(),
        "to_state": transition.to_state.to_string(),
        "from_version": transition.from_version,
        "to_version": transition.to_version,
        "cause_kind": transition.cause_kind,
        "cause_ref": transition.cause_ref,
        "gate_evaluation_id": transition.gate_evaluation_id,
        "reason_kind": transition.reason_kind,
        "reason_ref": transition.reason_ref,
        "domain_event_id": transition.domain_event_id,
        "created_at": transition.created_at,
    })
}

pub(super) async fn forge_transition_task_lifecycle(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: TransitionTaskLifecycleParams = parse_params(params)?;
    let user_id = authenticated_user(context)?.to_owned();
    let cause = match params.gate_evaluation_id {
        Some(evaluation_id)
            if matches!(
                params.to_state,
                db::TaskLifecycleState::ReadyToMerge | db::TaskLifecycleState::Active
            ) =>
        {
            LifecycleCause::GateEvaluation(evaluation_id)
        }
        Some(_) => {
            return Err(McpToolError::new(
                -32602,
                "gate_evaluation_id is accepted only for exact merge-readiness lifecycle edges",
            ));
        }
        None if params.to_state == db::TaskLifecycleState::ReadyToMerge => {
            return Err(McpToolError::new(
                -32602,
                "ready_to_merge requires the exact GateEvaluation that admitted it",
            ));
        }
        None => LifecycleCause::Actor(Actor::User {
            user_id: Some(user_id),
            source: UserActionSource::Mcp,
        }),
    };
    let task = db::TaskRepo::get_by_id(&*state.db, &params.task_id, false)
        .await?
        .ok_or_else(|| McpToolError::new(-32004, "Task not found"))?;
    let result = TaskLifecycleService::new(Arc::clone(&state.db), Arc::clone(&state.event_bus))
        .transition_at_lifecycle_version(
            TransitionLifecycleInput {
                task_id: params.task_id,
                expected_task_version: task.version,
                to_state: params.to_state,
                cause,
                reason_kind: params.reason_kind,
                reason_ref: params.reason_ref,
                idempotency_key: params.idempotency_key,
            },
            params.expected_lifecycle_version,
        )
        .await?;
    let (transition_id, gate_evaluation_id, replayed) = result
        .transition
        .map(|receipt| {
            (
                Some(receipt.transition_id),
                receipt.gate_evaluation_id,
                receipt.replayed,
            )
        })
        .unwrap_or((None, None, false));
    Ok(json!({
        "task_id": result.lifecycle.task_id,
        "lifecycle": result.lifecycle,
        "transition_id": transition_id,
        "gate_evaluation_id": gate_evaluation_id,
        "replayed": replayed,
    }))
}

pub(super) async fn forge_list_task_roles(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: GetTaskParams = parse_params(params)?;
    let mut roles = Vec::new();
    for role in TaskRoleRepo::list_by_task(&*state.db, &params.task_id).await? {
        let members = RoleMembershipRepo::list_by_role(&*state.db, &role.id, true).await?;
        roles.push(json!({
            "id": role.id,
            "task_id": role.task_id,
            "role": role.role,
            "coordination_mode": role.coordination_mode.map(|mode| mode.to_string()),
            "policy": serde_json::from_str::<Value>(&role.policy_json).unwrap_or(Value::Null),
            "version": role.version,
            "members": members.into_iter().map(|member| json!({
                "id": member.id,
                "task_role_id": member.task_role_id,
                "actor_ref": member.actor_ref(),
                "status": member.status.to_string(),
                "version": member.version,
                "created_at": member.created_at,
                "updated_at": member.updated_at,
                "ended_at": member.ended_at,
            })).collect::<Vec<_>>(),
            "created_at": role.created_at,
            "updated_at": role.updated_at,
        }));
    }
    Ok(json!({"items": roles}))
}

pub(super) async fn forge_create_task_role(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let user_id = authenticated_user(context)?;
    let params: CreateTaskRoleParams = parse_params(params)?;
    require_task_access(state, &params.task_id, user_id).await?;
    let policy = params.policy.unwrap_or_else(|| json!({}));
    let coordination_mode = match params.coordination_mode {
        PublicCoordinationMode::Partitioned => db::CoordinationMode::Partitioned,
        PublicCoordinationMode::Collaborative => db::CoordinationMode::Collaborative,
        PublicCoordinationMode::Independent => db::CoordinationMode::Independent,
    };
    let role = state
        .task_service
        .create_task_role(
            &params.task_id,
            &params.role,
            coordination_mode,
            serde_json::to_string(&policy)
                .map_err(|error| McpToolError::new(-32602, error.to_string()))?,
        )
        .await?;
    let members = RoleMembershipRepo::list_by_role(&*state.db, &role.id, false).await?;
    task_role_value(role, members)
}

pub(super) async fn forge_add_task_role_member(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let user_id = authenticated_user(context)?;
    let params: AddTaskRoleMemberParams = parse_params(params)?;
    require_task_access(state, &params.task_id, user_id).await?;
    let member = state
        .task_service
        .add_task_role_member(&params.task_id, &params.role, params.actor_ref)
        .await?;
    Ok(json!({
        "id": member.id,
        "task_role_id": member.task_role_id,
        "actor_ref": member.actor_ref(),
        "status": member.status.to_string(),
        "version": member.version,
        "created_at": member.created_at,
        "updated_at": member.updated_at,
        "ended_at": member.ended_at,
    }))
}

fn task_role_value(
    role: db::TaskRole,
    members: Vec<db::RoleMembership>,
) -> Result<Value, McpToolError> {
    let policy = serde_json::from_str::<Value>(&role.policy_json).map_err(|error| {
        McpToolError::new(-32603, format!("invalid stored TaskRole policy: {error}"))
    })?;
    Ok(json!({
        "id": role.id,
        "task_id": role.task_id,
        "role": role.role,
        "coordination_mode": role.coordination_mode.map(|mode| mode.to_string()),
        "policy": policy,
        "version": role.version,
        "members": members.into_iter().map(|member| json!({
            "id": member.id,
            "task_role_id": member.task_role_id,
            "actor_ref": member.actor_ref(),
            "status": member.status.to_string(),
            "version": member.version,
            "created_at": member.created_at,
            "updated_at": member.updated_at,
            "ended_at": member.ended_at,
        })).collect::<Vec<_>>(),
        "created_at": role.created_at,
        "updated_at": role.updated_at,
    }))
}

async fn require_task_access(
    state: &AppState,
    task_id: &str,
    user_id: &str,
) -> Result<Task, McpToolError> {
    let task = TaskRepo::get_by_id(&*state.db, task_id, false)
        .await?
        .ok_or_else(|| McpToolError::not_found("task", task_id.to_owned()))?;
    let project = ProjectRepo::get_by_id(&*state.db, &task.project_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("project", task.project_id.clone()))?;
    if project
        .owner_id
        .as_deref()
        .is_some_and(|owner_id| owner_id != user_id)
        && ProjectMemberRepo::get_member(&*state.db, &project.id, user_id)
            .await?
            .is_none()
    {
        return Err(McpToolError::new(-32001, "project not accessible"));
    }
    Ok(task)
}

fn gate_value(
    gate: db::Gate,
    policy: Option<db::GatePolicyRevision>,
) -> Result<Value, McpToolError> {
    let active_policy = policy.map(|policy| -> Result<Value, McpToolError> {
        Ok(json!({
            "gate_id": policy.gate_id,
            "revision": policy.revision,
            "schema_version": policy.schema_version,
            "policy": serde_json::from_str::<Value>(&policy.policy_json).map_err(|error| McpToolError::new(-32603, format!("invalid stored Gate policy: {error}")))?,
            "policy_digest": policy.policy_digest,
            "created_at": policy.created_at,
        }))
    }).transpose()?;
    Ok(json!({
        "gate": {
            "id": gate.id,
            "task_id": gate.task_id,
            "gate_kind": gate.gate_kind,
            "scope_kind": gate.scope_kind.to_string(),
            "scope_id": gate.scope_id,
            "active_policy_revision": gate.active_policy_revision,
            "created_at": gate.created_at,
        },
        "active_policy": active_policy,
    }))
}

fn gate_evaluation_value(
    evaluation: db::GateEvaluation,
    inputs: Vec<db::GateEvaluationInput>,
) -> Result<Value, McpToolError> {
    let result: Value = serde_json::from_str(&evaluation.result_json).map_err(|error| {
        McpToolError::new(-32603, format!("invalid stored Gate result: {error}"))
    })?;
    let mut input_values = Vec::with_capacity(inputs.len());
    for input in inputs {
        input_values.push(json!({
            "ordinal": input.ordinal,
            "input_kind": input.input_kind,
            "input_id": input.input_id,
            "input_version": input.input_version,
            "input_digest": input.input_digest,
            "producer_ref": input.producer_ref,
            "subject": serde_json::from_str::<Value>(&input.subject_json).map_err(|error| McpToolError::new(-32603, format!("invalid stored Gate input: {error}")))?,
            "status": input.status,
        }));
    }
    Ok(json!({
        "id": evaluation.id,
        "gate_id": evaluation.gate_id,
        "task_id": evaluation.task_id,
        "policy_revision": evaluation.policy_revision,
        "outcome": evaluation.outcome.to_string(),
        "input_digest": evaluation.input_digest,
        "result": result,
        "evaluated_at": evaluation.evaluated_at,
        "inputs": input_values,
    }))
}

pub(super) async fn forge_create_task_gate(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: GateTaskParams = parse_params(params)?;
    let policy = serde_json::from_value(params.policy)
        .map_err(|error| McpToolError::new(-32602, format!("invalid Gate policy: {error}")))?;
    let (gate, revision) = GateEngine::new(Arc::clone(&state.db), Arc::clone(&state.event_bus))
        .create_gate_with_initial_policy(
            &params.task_id,
            &params.gate_kind,
            GateScopeKind::Task,
            &params.task_id,
            policy,
        )
        .await?;
    gate_value(gate, Some(revision))
}

pub(super) async fn forge_get_gate(state: &AppState, params: Value) -> Result<Value, McpToolError> {
    let params: GateIdParams = parse_params(params)?;
    let gate = GateRepo::get_gate(&*state.db, &params.gate_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("gate", params.gate_id))?;
    let policy = match gate.active_policy_revision {
        Some(revision) => Some(
            GateRepo::get_gate_policy_revision(&*state.db, &gate.id, revision)
                .await?
                .ok_or_else(|| {
                    McpToolError::not_found(
                        "gate_policy_revision",
                        format!("{}:{revision}", gate.id),
                    )
                })?,
        ),
        None => None,
    };
    gate_value(gate, policy)
}

pub(super) async fn forge_revise_gate_policy(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: ReviseGatePolicyParams = parse_params(params)?;
    let policy = serde_json::from_value(params.policy)
        .map_err(|error| McpToolError::new(-32602, format!("invalid Gate policy: {error}")))?;
    let revision = GateEngine::new(Arc::clone(&state.db), Arc::clone(&state.event_bus))
        .revise_policy(&params.gate_id, params.expected_active_revision, policy)
        .await?;
    Ok(json!({
        "gate_id": revision.gate_id,
        "revision": revision.revision,
        "schema_version": revision.schema_version,
        "policy": serde_json::from_str::<Value>(&revision.policy_json).map_err(|error| McpToolError::new(-32603, format!("invalid stored Gate policy: {error}")))?,
        "policy_digest": revision.policy_digest,
        "created_at": revision.created_at,
    }))
}

pub(super) async fn forge_evaluate_gate(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: GateIdParams = parse_params(params)?;
    let write = GateEngine::new(Arc::clone(&state.db), Arc::clone(&state.event_bus))
        .evaluate_active(&params.gate_id)
        .await?;
    gate_evaluation_value(write.evaluation, write.inputs)
}

pub(super) async fn forge_get_gate_evaluation(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: GateEvaluationParams = parse_params(params)?;
    let evaluation = GateRepo::get_gate_evaluation(&*state.db, &params.evaluation_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("gate_evaluation", params.evaluation_id))?;
    let inputs = GateRepo::list_gate_evaluation_inputs(&*state.db, &evaluation.id).await?;
    gate_evaluation_value(evaluation, inputs)
}

pub(super) async fn forge_list_task_review_executions(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: ReviewTaskParams = parse_params(params)?;
    let page = ExecutionRepo::list_by_task(
        &*state.db,
        &params.task_id,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    let mut reviews = Vec::new();
    for execution in page.items.into_iter().filter(|execution| {
        execution.role == "reviewer" && execution.purpose == Some(ExecutionPurpose::Review)
    }) {
        reviews.push(review_execution_value(state, execution).await?);
    }
    Ok(json!({"items": reviews}))
}

pub(super) async fn forge_get_review_execution(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: ReviewExecutionParams = parse_params(params)?;
    let execution = ExecutionRepo::get_by_id(&*state.db, &params.execution_id)
        .await?
        .filter(|execution| {
            execution.role == "reviewer" && execution.purpose == Some(ExecutionPurpose::Review)
        })
        .ok_or_else(|| McpToolError::not_found("review_execution", params.execution_id))?;
    review_execution_value(state, execution).await
}

async fn review_execution_value(
    state: &AppState,
    execution: db::Execution,
) -> Result<Value, McpToolError> {
    let report = CollaborationRepo::get_execution_artifact_output(
        &*state.db,
        &execution.id,
        ArtifactKind::ReviewReport,
    )
    .await?;
    let report = report.map(artifact_value).transpose()?;
    Ok(json!({"execution": execution_value(execution), "report": report}))
}

pub(super) async fn forge_list_validation_runs(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: ValidationTaskParams = parse_params(params)?;
    let runs = ValidationRunRepo::list_validation_runs_by_task(&*state.db, &params.task_id).await?;
    let mut items = Vec::with_capacity(runs.len());
    for run in runs {
        items.push(validation_run_value(state, run).await?);
    }
    Ok(json!({"items": items}))
}

pub(super) async fn forge_get_validation_run(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: ValidationRunParams = parse_params(params)?;
    let run = ValidationRunRepo::get_validation_run(&*state.db, &params.validation_run_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("validation_run", params.validation_run_id))?;
    validation_run_value(state, run).await
}

async fn validation_run_value(
    state: &AppState,
    run: db::ValidationRun,
) -> Result<Value, McpToolError> {
    let evidence = ValidationRunRepo::list_evidence_for_validation_run(&*state.db, &run.id).await?;
    let artifact =
        ValidationRunRepo::get_validation_run_artifact_output(&*state.db, &run.id).await?;
    Ok(json!({
        "id": run.id,
        "task_id": run.task_id,
        "work_unit_id": run.work_unit_id,
        "caused_by_execution_id": run.caused_by_execution_id,
        "check_identity": run.check_identity,
        "command": run.command,
        "config_summary": serde_json::from_str::<Value>(&run.config_summary_json).map_err(|error| McpToolError::new(-32603, format!("invalid stored ValidationRun config: {error}")))?,
        "config_digest": run.config_digest,
        "workspace_id": run.workspace_id,
        "commit_sha": run.commit_sha,
        "workspace_snapshot_digest": run.workspace_snapshot_digest,
        "status": run.status.to_string(),
        "exit_code": run.exit_code,
        "started_at": run.started_at,
        "finished_at": run.finished_at,
        "logs_ref": run.logs_ref,
        "evidence_ids": evidence.into_iter().map(|item| item.id).collect::<Vec<_>>(),
        "validation_report_artifact_id": artifact.map(|item| item.id),
        "created_at": run.created_at,
        "updated_at": run.updated_at,
    }))
}

pub(super) async fn forge_get_evidence(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: EvidenceParams = parse_params(params)?;
    let evidence = ValidationRunRepo::get_evidence(&*state.db, &params.evidence_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("evidence", params.evidence_id))?;
    Ok(json!({
        "id": evidence.id,
        "task_id": evidence.task_id,
        "kind": evidence.kind,
        "content": serde_json::from_str::<Value>(&evidence.content_json).map_err(|error| McpToolError::new(-32603, format!("invalid stored Evidence content: {error}")))?,
        "digest": evidence.digest,
        "producer_validation_run_id": evidence.producer_validation_run_id,
        "evidence_key": evidence.evidence_key,
        "created_at": evidence.created_at,
    }))
}

fn artifact_value(artifact: db::Artifact) -> Result<Value, McpToolError> {
    Ok(json!({
        "id": artifact.id,
        "task_id": artifact.task_id,
        "kind": artifact.kind.to_string(),
        "storage_kind": artifact.storage_kind.to_string(),
        "content": artifact.content,
        "content_ref": artifact.content_ref,
        "metadata": serde_json::from_str::<Value>(&artifact.metadata_json).map_err(|error| McpToolError::new(-32603, format!("invalid stored Artifact metadata: {error}")))?,
        "digest": artifact.digest,
        "producer": artifact.producer,
        "created_at": artifact.created_at,
    }))
}

pub(super) async fn forge_list_task_artifacts(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: CollaborationListParams = parse_params(params)?;
    let page = CollaborationRepo::list_artifacts(
        &*state.db,
        &params.task_id,
        page_request(params.cursor, params.limit, None)?,
    )
    .await?;
    let mut items = Vec::with_capacity(page.items.len());
    for artifact in page.items {
        items.push(artifact_value(artifact)?);
    }
    let has_more = page.next_cursor.is_some();
    Ok(
        json!({"items": items, "next_cursor": page.next_cursor, "has_more": has_more, "total_count": page.total_count}),
    )
}

pub(super) async fn forge_list_task_messages(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: CollaborationListParams = parse_params(params)?;
    let page = CollaborationRepo::list_messages(
        &*state.db,
        &params.task_id,
        page_request(params.cursor, params.limit, None)?,
    )
    .await?;
    let items = page
        .items
        .into_iter()
        .map(message_value)
        .collect::<Vec<_>>();
    let has_more = page.next_cursor.is_some();
    Ok(
        json!({"items": items, "next_cursor": page.next_cursor, "has_more": has_more, "total_count": page.total_count}),
    )
}

pub(super) async fn forge_list_task_handoffs(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: CollaborationListParams = parse_params(params)?;
    let page = CollaborationRepo::list_handoffs(
        &*state.db,
        &params.task_id,
        page_request(params.cursor, params.limit, None)?,
    )
    .await?;
    let items = page
        .items
        .into_iter()
        .map(handoff_value)
        .collect::<Vec<_>>();
    let has_more = page.next_cursor.is_some();
    Ok(
        json!({"items": items, "next_cursor": page.next_cursor, "has_more": has_more, "total_count": page.total_count}),
    )
}

pub(super) async fn forge_list_task_proposals(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: CollaborationListParams = parse_params(params)?;
    let page = CollaborationRepo::list_proposals(
        &*state.db,
        &params.task_id,
        page_request(params.cursor, params.limit, None)?,
    )
    .await?;
    let items = page
        .items
        .into_iter()
        .map(proposal_value)
        .collect::<Vec<_>>();
    let has_more = page.next_cursor.is_some();
    Ok(
        json!({"items": items, "next_cursor": page.next_cursor, "has_more": has_more, "total_count": page.total_count}),
    )
}

pub(super) async fn forge_list_task_decisions(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: CollaborationListParams = parse_params(params)?;
    let page = CollaborationRepo::list_decisions(
        &*state.db,
        &params.task_id,
        page_request(params.cursor, params.limit, None)?,
    )
    .await?;
    let items = page
        .items
        .into_iter()
        .map(decision_value)
        .collect::<Vec<_>>();
    let has_more = page.next_cursor.is_some();
    Ok(
        json!({"items": items, "next_cursor": page.next_cursor, "has_more": has_more, "total_count": page.total_count}),
    )
}

fn message_value(message: db::Message) -> Value {
    json!({
        "id": message.id,
        "task_id": message.task_id,
        "sender": message.sender,
        "target": message.target,
        "work_unit_id": message.work_unit_id,
        "body": message.body,
        "artifact_ids": message.artifact_ids,
        "created_at": message.created_at,
    })
}

fn handoff_value(handoff: db::Handoff) -> Value {
    json!({
        "id": handoff.id,
        "task_id": handoff.task_id,
        "created_by": handoff.created_by,
        "source_role_id": handoff.source_role_id,
        "target": handoff.target,
        "work_unit_id": handoff.work_unit_id,
        "intent": handoff.intent.to_string(),
        "parent_execution_id": handoff.parent_execution_id,
        "expected_policy_ref": handoff.expected_policy_ref,
        "status": handoff.status.to_string(),
        "version": handoff.version,
        "artifact_ids": handoff.artifact_ids,
        "created_at": handoff.created_at,
        "updated_at": handoff.updated_at,
    })
}

fn proposal_value(proposal: db::Proposal) -> Value {
    json!({
        "id": proposal.id,
        "task_id": proposal.task_id,
        "proposer": proposal.proposer,
        "target": proposal.target,
        "action": proposal.action,
        "reason": proposal.reason,
        "target_version": proposal.target_version,
        "target_digest": proposal.target_digest,
        "required_policy_ref": proposal.required_policy_ref,
        "required_policy_version": proposal.required_policy_version,
        "required_policy_digest": proposal.required_policy_digest,
        "content_version": proposal.content_version,
        "status": proposal.status.to_string(),
        "supersedes_proposal_id": proposal.supersedes_proposal_id,
        "artifact_ids": proposal.artifact_ids,
        "created_at": proposal.created_at,
    })
}

fn decision_value(decision: db::Decision) -> Value {
    json!({
        "id": decision.id,
        "task_id": decision.task_id,
        "proposal_id": decision.proposal_id,
        "proposal_version": decision.proposal_version,
        "outcome": decision.outcome.to_string(),
        "rationale": decision.rationale,
        "policy_ref": decision.policy_ref,
        "policy_version": decision.policy_version,
        "policy_digest": decision.policy_digest,
        "actors": decision.actors,
        "created_at": decision.created_at,
    })
}
