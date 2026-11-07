use std::sync::Arc;

use api_types::{
    parse_project_hooks_json, Actor, ActorRef, CoordinationMode as PublicCoordinationMode,
    UserActionSource,
};
use db::{
    new_uuid_v4, now_rfc3339, AgentListQuery, AgentProfileRepo, AgentRepo, ArtifactKind,
    CollaborationRepo, CreateProject, ExecutionRepo, GateRepo, GateScopeKind, PageRequest,
    ProjectRepo, RoleMembershipRepo, SortBy, SortOrder, Task, TaskDependencyRepo,
    TaskLifecycleRepo, TaskListQuery, TaskRepo, TaskRoleRepo, UpdateProject, UpdateTask,
    ValidationRunRepo, WorkUnitDependency, WorkUnitRepo,
};
use executors::ExecutionOverrides;
use serde_json::{json, Map, Value};
use services::{
    gate_engine::GateEngine,
    task_lifecycle::{LifecycleCause, TaskLifecycleService, TransitionLifecycleInput},
    CollaborationActorSource, DiffService,
};

use crate::{
    error::McpToolError,
    params::{
        page_request, parse_params, task_page_request, AddTaskDependencyParams,
        AddTaskRoleMemberParams, AllocateWorkUnitParams, CollaborationListParams,
        CreateProjectParams, CreateSubTasksParams, CreateTaskParams, CreateTaskRoleParams,
        CreateWorkUnitParams, EvidenceParams, GateEvaluationParams, GateIdParams, GateTaskParams,
        GetProjectParams, GetTaskParams, ListAgentProfilesParams, ListAgentsParams,
        ListExecutionsParams, ListProjectsParams, ListTaskDependenciesParams, ListTasksParams,
        ListWorkUnitsParams, RegisterAgentParams, RemoveTaskDependencyParams, RequestReworkParams,
        ReviewExecutionParams, ReviewTaskParams, ReviseGatePolicyParams,
        SubmitWorkUnitReviewParams, TaskDispatchActionParams, TransitionTaskLifecycleParams,
        UpdateProjectHooksParams, UpdateProjectParams, UpdateTaskParams, UpdateTaskRoleParams,
        UpdateWorkUnitParams, ValidationRunParams, ValidationTaskParams, WorkUnitDependencyParams,
        WorkUnitIdParams, WorkUnitTransitionParams,
    },
    protocol::McpContext,
    state::AppState,
    values::{
        agent_profile_value, agent_value, execution_page_value, execution_value,
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
            agent_ids: Vec::new(),
            assignee_types: Vec::new(),
            assignee_ids: Vec::new(),
            priority: None,
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

pub(super) async fn forge_get_task_dispatch(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: GetTaskParams = parse_params(params)?;
    let user_id = authenticated_user(context)?;
    require_task_access(state, &params.task_id, user_id).await?;
    let preflight = dispatch_service(state)
        .launch_preflight(&params.task_id)
        .await?;
    serde_json::to_value(preflight).map_err(|error| {
        McpToolError::new(-32603, format!("failed to serialize dispatch: {error}"))
    })
}

pub(super) async fn forge_launch_task(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: TaskDispatchActionParams = parse_params(params)?;
    let user_id = authenticated_user(context)?;
    let task = require_task_access(state, &params.task_id, user_id).await?;
    require_project_write_access(state, &task.project_id, user_id).await?;
    let write = dispatch_service(state)
        .launch_task(
            &params.task_id,
            params.expected_task_version,
            params.expected_lifecycle_version,
            &ActorRef::Human(user_id.to_owned()),
            &params.idempotency_key,
        )
        .await?;
    let outcome = services::DispatchOutcome::default();
    let preflight = dispatch_service(state)
        .launch_preflight(&params.task_id)
        .await?;
    Ok(json!({
        "admitted": true,
        "dispatch": dispatch_intent_value(&write.dispatch),
        "preflight": preflight,
        "admitted_execution_ids": outcome.admitted_execution_ids,
        "started_execution_ids": outcome.started_execution_ids,
    }))
}

pub(super) async fn forge_pause_task(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: TaskDispatchActionParams = parse_params(params)?;
    let user_id = authenticated_user(context)?;
    let task = require_task_access(state, &params.task_id, user_id).await?;
    require_project_write_access(state, &task.project_id, user_id).await?;
    let write = dispatch_service(state)
        .pause_task(
            &params.task_id,
            params.expected_task_version,
            params.expected_lifecycle_version,
            &ActorRef::Human(user_id.to_owned()),
            params.reason,
            &params.idempotency_key,
        )
        .await?;
    dispatch_action_value(state, &params.task_id, write).await
}

pub(super) async fn forge_resume_task(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: TaskDispatchActionParams = parse_params(params)?;
    let user_id = authenticated_user(context)?;
    let task = require_task_access(state, &params.task_id, user_id).await?;
    require_project_write_access(state, &task.project_id, user_id).await?;
    let write = dispatch_service(state)
        .resume_task(
            &params.task_id,
            params.expected_task_version,
            params.expected_lifecycle_version,
            &ActorRef::Human(user_id.to_owned()),
            &params.idempotency_key,
        )
        .await?;
    dispatch_action_value(state, &params.task_id, write).await
}

pub(super) async fn forge_cancel_task(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: TaskDispatchActionParams = parse_params(params)?;
    let user_id = authenticated_user(context)?;
    let task = require_task_access(state, &params.task_id, user_id).await?;
    require_project_write_access(state, &task.project_id, user_id).await?;
    if let Some(receipt) = TaskLifecycleRepo::get_task_dispatch_receipt(
        &*state.db,
        &params.task_id,
        &params.idempotency_key,
    )
    .await?
    {
        if receipt.operation != db::TaskDispatchOperation::Cancel {
            return Err(McpToolError::new(
                -32602,
                "Task dispatch idempotency key belongs to another operation",
            ));
        }
        let write = dispatch_service(state)
            .record_cancelled_dispatch_after_cancel(
                &params.task_id,
                params.expected_task_version,
                params.expected_lifecycle_version,
                &ActorRef::Human(user_id.to_owned()),
                params.reason,
                &params.idempotency_key,
            )
            .await?;
        return dispatch_action_value(state, &params.task_id, write).await;
    }
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*state.db, &params.task_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("task lifecycle", params.task_id.clone()))?;
    if task.version != params.expected_task_version {
        return Err(McpToolError::from(db::DbError::TaskVersionConflict {
            expected: params.expected_task_version,
            actual: task.version,
        }));
    }
    if lifecycle.version != params.expected_lifecycle_version {
        return Err(McpToolError::from(db::DbError::VersionConflict));
    }
    state
        .task_service
        .cancel_task_as(
            params.task_id.clone(),
            Actor::User {
                user_id: Some(user_id.to_owned()),
                source: UserActionSource::Mcp,
            },
        )
        .await?;
    let write = dispatch_service(state)
        .record_cancelled_dispatch_after_cancel(
            &params.task_id,
            task.version,
            lifecycle.version,
            &ActorRef::Human(user_id.to_owned()),
            params.reason,
            &params.idempotency_key,
        )
        .await?;
    dispatch_action_value(state, &params.task_id, write).await
}

pub(super) async fn forge_request_rework(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: RequestReworkParams = parse_params(params)?;
    let user_id = authenticated_user(context)?;
    let task = require_task_access(state, &params.task_id, user_id).await?;
    require_project_write_access(state, &task.project_id, user_id).await?;
    let service =
        services::TaskReworkService::new(Arc::clone(&state.db), Arc::clone(&state.event_bus));
    let result = service
        .request_rework(
            CollaborationActorSource::Human(user_id.to_owned()),
            services::RequestReworkInput {
                task_id: params.task_id.clone(),
                expected_task_version: params.expected_task_version,
                expected_lifecycle_version: params.expected_lifecycle_version,
                review_report_artifact_id: params.review_report_artifact_id,
                reviewed_work_unit_id: params.reviewed_work_unit_id,
                reviewed_work_unit_version: params.reviewed_work_unit_version,
                correction_title: params.correction_title,
                correction_scope: params.correction_scope,
                correction_role_id: params.correction_role_id,
                correction_actor: params.correction_actor,
                review_title: params.review_title,
                review_scope: params.review_scope,
                review_actor: params.review_actor,
                idempotency_key: params.idempotency_key,
                expected_source_configuration_json: None,
            },
        )
        .await?;
    let source = CollaborationActorSource::Human(user_id.to_owned());
    let correction =
        work_unit_value_for(state, source.clone(), result.correction_work_unit).await?;
    let review = work_unit_value_for(state, source, result.review_work_unit).await?;
    Ok(json!({
        "correction_work_unit": correction,
        "review_work_unit": review,
        "review_subject": result.review_subject.as_ref().map(services::task_rework::review_subject_response),
        "rounds_remaining": result.rounds_remaining,
        "replayed": result.replayed,
    }))
}

pub(super) async fn forge_update_task(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: UpdateTaskParams = parse_params(params)?;
    let task = state
        .task_service
        .update_task(UpdateTask {
            id: params.task_id,
            expected_version: params.version,
            title: params.title,
            description: params.description.map(Some),
            priority: params.priority,
            merge_config: None,
            error_annotation: None,
            blocked_json: None,
            failed_json: None,
            parent_task_id: params.parent_task_id,
            updated_at: now_rfc3339(),
        })
        .await?;
    task_public_value(state, task).await
}

pub(super) async fn forge_create_work_unit(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: CreateWorkUnitParams = parse_params(params)?;
    let source = work_unit_source(context)?;
    let provenance = params.provenance.map(|provenance| match provenance {
        api_types::CreateWorkUnitProvenance::Actor { actor } => {
            db::WorkUnitProvenance::Actor(work_unit_actor_to_db(actor))
        }
        api_types::CreateWorkUnitProvenance::WorkUnit { id } => {
            db::WorkUnitProvenance::WorkUnit(id)
        }
        api_types::CreateWorkUnitProvenance::Artifact { id } => {
            db::WorkUnitProvenance::Artifact(id)
        }
        api_types::CreateWorkUnitProvenance::External { id } => {
            db::WorkUnitProvenance::External(id)
        }
    });
    let unit = state
        .work_unit_service
        .create_idempotent(
            source.clone(),
            services::CreateWorkUnitInput {
                task_id: params.task_id,
                title: params.title,
                scope: params.scope,
                execution_instructions: params.execution_instructions,
                role: params.role,
                parent_work_unit_id: params.parent_work_unit_id,
                assigned_actor: params.assigned_actor.map(work_unit_actor_to_db),
                requires_integration: params.requires_integration,
                provenance,
                artifact_inputs: params.artifact_inputs.unwrap_or_default(),
                review_approval: params.review_approval,
                review_subject: params
                    .review_subject
                    .map(|subject| subject.reviewed_work_unit_id),
            },
            &params.idempotency_key,
        )
        .await?;
    work_unit_value_for(state, source, unit).await
}

pub(super) async fn forge_list_work_units(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: ListWorkUnitsParams = parse_params(params)?;
    let source = work_unit_source(context)?;
    let units = state
        .work_unit_service
        .list(source.clone(), &params.task_id)
        .await?;
    let mut items = Vec::with_capacity(units.len());
    for unit in units {
        items.push(work_unit_value_for(state, source.clone(), unit).await?);
    }
    let total_count = items.len();
    Ok(json!({"items": items, "total_count": total_count}))
}

pub(super) async fn forge_submit_work_unit_review(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    if context.copilot_run_id.is_some() {
        return Err(McpToolError::new(
            -32003,
            "Human authentication is required for WorkUnit review submission",
        ));
    }
    let params: SubmitWorkUnitReviewParams = parse_params(params)?;
    let user_id = authenticated_user(context)?;
    let task_id = WorkUnitRepo::get_task_id(&*state.db, &params.work_unit_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("work_unit", params.work_unit_id.clone()))?;
    let task = require_task_access(state, &task_id, user_id).await?;
    require_project_write_access(state, &task.project_id, user_id).await?;
    let (execution, report) = state
        .work_unit_service
        .submit_human_review(
            &state.task_service,
            &params.work_unit_id,
            user_id,
            params.expected_version,
            params.report,
        )
        .await?;
    Ok(json!({
        "review_execution": {
            "execution": execution_value(execution),
            "report": artifact_value(report.clone())?,
        },
        "report": artifact_value(report)?,
    }))
}

pub(super) async fn forge_get_work_unit(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: WorkUnitIdParams = parse_params(params)?;
    let source = work_unit_source(context)?;
    let unit = state
        .work_unit_service
        .get(source.clone(), &params.work_unit_id)
        .await?;
    work_unit_value_for(state, source, unit).await
}

pub(super) async fn forge_get_work_unit_artifact_package(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: WorkUnitIdParams = parse_params(params)?;
    let package = state
        .work_unit_service
        .artifact_package(work_unit_source(context)?, &params.work_unit_id)
        .await?;
    serde_json::to_value(package).map_err(|error| {
        McpToolError::new(
            -32603,
            format!("failed to serialize Artifact package: {error}"),
        )
    })
}

pub(super) async fn forge_list_task_review_lines(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: GetTaskParams = parse_params(params)?;
    let user_id = authenticated_user(context)?;
    require_task_access(state, &params.task_id, user_id).await?;
    let lines = services::task_rework::list_review_lines(&state.db, &params.task_id).await?;
    serde_json::to_value(lines).map_err(|error| {
        McpToolError::new(-32603, format!("failed to serialize review lines: {error}"))
    })
}

pub(super) async fn forge_update_work_unit(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: UpdateWorkUnitParams = parse_params(params)?;
    let source = work_unit_source(context)?;
    let unit = state
        .work_unit_service
        .update(
            source.clone(),
            &params.work_unit_id,
            params.expected_version,
            params.title,
            params.scope,
            params.execution_instructions,
            params.requires_integration,
        )
        .await?;
    work_unit_value_for(state, source, unit).await
}

pub(super) async fn forge_allocate_work_unit(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: AllocateWorkUnitParams = parse_params(params)?;
    let source = work_unit_source(context)?;
    let unit = state
        .work_unit_service
        .allocate(
            source.clone(),
            &params.work_unit_id,
            params.expected_version,
            params.role,
            params.assigned_actor.map(work_unit_actor_to_db),
        )
        .await?;
    work_unit_value_for(state, source, unit).await
}

pub(super) async fn forge_list_work_unit_dependencies(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: WorkUnitIdParams = parse_params(params)?;
    let dependencies = state
        .work_unit_service
        .dependencies(work_unit_source(context)?, &params.work_unit_id)
        .await?;
    Ok(json!({
        "items": dependencies.iter().map(work_unit_dependency_value).collect::<Vec<_>>(),
        "total_count": dependencies.len(),
    }))
}

pub(super) async fn forge_add_work_unit_dependency(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: WorkUnitDependencyParams = parse_params(params)?;
    let dependency = state
        .work_unit_service
        .add_dependency(
            work_unit_source(context)?,
            &params.work_unit_id,
            &params.depends_on_work_unit_id,
            params.expected_version,
        )
        .await?;
    Ok(work_unit_dependency_value(&dependency))
}

pub(super) async fn forge_remove_work_unit_dependency(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: WorkUnitDependencyParams = parse_params(params)?;
    let source = work_unit_source(context)?;
    let unit = state
        .work_unit_service
        .remove_dependency(
            source.clone(),
            &params.work_unit_id,
            &params.depends_on_work_unit_id,
            params.expected_version,
        )
        .await?;
    work_unit_value_for(state, source, unit).await
}

pub(super) async fn forge_transition_work_unit(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: WorkUnitTransitionParams = parse_params(params)?;
    let status = match params.status {
        api_types::WorkUnitStatus::Completed => db::WorkUnitStatus::Completed,
        api_types::WorkUnitStatus::Cancelled => db::WorkUnitStatus::Cancelled,
        api_types::WorkUnitStatus::Open => {
            return Err(McpToolError::new(
                -32602,
                "WorkUnit cannot transition back to open",
            ));
        }
    };
    let source = work_unit_source(context)?;
    let unit = state
        .work_unit_service
        .transition(
            source.clone(),
            &params.work_unit_id,
            params.expected_version,
            status,
        )
        .await?;
    work_unit_value_for(state, source, unit).await
}

fn work_unit_source(context: &McpContext) -> Result<CollaborationActorSource, McpToolError> {
    if let Some(run_id) = context.copilot_run_id.as_ref() {
        return Ok(CollaborationActorSource::ProjectCopilot(run_id.clone()));
    }
    let user_id = context
        .user_id
        .as_ref()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| McpToolError::new(-32003, "authenticated server identity is required"))?;
    Ok(CollaborationActorSource::Human(user_id.clone()))
}

fn work_unit_actor_to_db(actor: api_types::ActorRef) -> db::ActorRef {
    match actor {
        api_types::ActorRef::Human(id) => db::ActorRef::Human(id),
        api_types::ActorRef::Agent(id) => db::ActorRef::Agent(id),
    }
}

fn work_unit_actor_value(actor: &db::ActorRef) -> Value {
    match actor {
        db::ActorRef::Human(id) => json!(api_types::ActorRef::Human(id.clone())),
        db::ActorRef::Agent(id) => json!(api_types::ActorRef::Agent(id.clone())),
    }
}

fn work_unit_provenance_value(provenance: &db::WorkUnitProvenance) -> Value {
    match provenance {
        db::WorkUnitProvenance::Actor(actor) => {
            json!(api_types::WorkUnitProvenance::Actor {
                actor: match actor {
                    db::ActorRef::Human(id) => api_types::ActorRef::Human(id.clone()),
                    db::ActorRef::Agent(id) => api_types::ActorRef::Agent(id.clone()),
                }
            })
        }
        db::WorkUnitProvenance::WorkUnit(id) => {
            json!(api_types::WorkUnitProvenance::WorkUnit { id: id.clone() })
        }
        db::WorkUnitProvenance::Artifact(id) => {
            json!(api_types::WorkUnitProvenance::Artifact { id: id.clone() })
        }
        db::WorkUnitProvenance::External(id) => {
            json!(api_types::WorkUnitProvenance::External { id: id.clone() })
        }
        db::WorkUnitProvenance::LegacyActor(id) => {
            json!(api_types::WorkUnitProvenance::LegacyActor { id: id.clone() })
        }
    }
}

fn work_unit_dependency_value(dependency: &WorkUnitDependency) -> Value {
    json!({
        "work_unit_id": dependency.work_unit_id,
        "depends_on_work_unit_id": dependency.depends_on_work_unit_id,
        "satisfied": dependency.satisfied,
        "created_at": dependency.created_at,
    })
}

async fn work_unit_value_for(
    state: &AppState,
    source: CollaborationActorSource,
    unit: db::WorkUnit,
) -> Result<Value, McpToolError> {
    let readiness = state.work_unit_service.readiness(source, &unit.id).await?;
    let review_subject = services::task_rework::get_review_subject(&state.db, &unit.id)
        .await?
        .as_ref()
        .map(services::task_rework::review_subject_response);
    Ok(json!({
        "id": unit.id,
        "task_id": unit.task_id,
        "parent_work_unit_id": unit.parent_work_unit_id,
        "title": unit.title,
        "scope": unit.scope,
        "execution_instructions": unit.execution_instructions,
        "status": unit.status.to_string(),
        "role": unit.role,
        "assigned_actor": unit.assigned_actor.as_ref().map(work_unit_actor_value),
        "requires_integration": unit.requires_integration,
        "provenance": unit.provenance.as_ref().map(work_unit_provenance_value),
        "created_by": work_unit_actor_value(&unit.created_by),
        "version": unit.version,
        "created_at": unit.created_at,
        "updated_at": unit.updated_at,
        "readiness": {
            "runnable": readiness.runnable,
            "ready_for_allocation": readiness.ready_for_allocation,
            "active_execution_ids": readiness.active_execution_ids,
            "unsatisfied_dependency_ids": readiness.unsatisfied_dependency_ids,
            "awaiting_integration": readiness.awaiting_integration,
            "awaiting_acceptance": readiness.awaiting_acceptance,
        },
        "review_subject": review_subject,
    }))
}

pub(super) async fn forge_register_agent(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: RegisterAgentParams = parse_params(params)?;
    let owner_id = authenticated_user(context)?.to_owned();
    if params.daemon_id.is_some() && !context.is_admin {
        return Err(McpToolError::new(
            -32003,
            "admin access is required to pin an Agent to a daemon",
        ));
    }
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
            Some(owner_id),
            Some("account".to_owned()),
        )
        .await?;
    Ok(agent_value(agent, context.is_admin))
}

pub(super) async fn forge_list_agents(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let user_id = authenticated_user(context)?;
    let params: ListAgentsParams = parse_params(params)?;
    let page = AgentRepo::list_visible_to_user(
        &*state.db,
        user_id,
        AgentListQuery {
            status: params.status.map(Into::into),
            executor_type: None,
            capabilities: Vec::new(),
            harness_only: true,
            page: page_request(params.cursor, params.limit, None)?,
        },
    )
    .await?;
    let visible = page
        .items
        .into_iter()
        .map(|agent| {
            let is_owner = agent.owner_id.as_deref() == Some(user_id);
            agent_value(agent, context.is_admin && is_owner)
        })
        .collect::<Vec<_>>();
    let has_more = page.next_cursor.is_some();
    Ok(json!({
        "items": visible,
        "next_cursor": page.next_cursor,
        "has_more": has_more,
        "total_count": null,
    }))
}

pub(super) async fn forge_list_projects(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let user_id = authenticated_user(context)?;
    let params: ListProjectsParams = parse_params(params)?;
    if let Some(project_id) = context.project_id.as_deref() {
        let project = ProjectRepo::get_by_id(&*state.db, project_id)
            .await?
            .ok_or_else(|| McpToolError::not_found("project", project_id.to_owned()))?;
        require_project_access(state, project_id, user_id).await?;
        return project_page_value(db::Page {
            items: vec![project],
            next_cursor: None,
            total_count: Some(1),
        });
    }
    let page = ProjectRepo::list_visible_to_user(
        &*state.db,
        user_id,
        page_request(params.cursor, params.limit, None)?,
    )
    .await?;
    project_page_value(page)
}

pub(super) async fn forge_create_project(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let user_id = authenticated_user(context)?.to_owned();
    let params: CreateProjectParams = parse_params(params)?;
    if params.name.trim().is_empty() {
        return Err(McpToolError::new(-32602, "name must not be empty"));
    }
    let now = now_rfc3339();
    let project = ProjectRepo::create_with_owner_membership(
        &*state.db,
        CreateProject {
            id: new_uuid_v4(),
            name: params.name,
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(user_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        &new_uuid_v4(),
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
        None,
    )
    .await?;
    project_value(project)
}

pub(super) async fn forge_follow_up_execution(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let user_id = authenticated_user(context)?;
    let params = params
        .as_object()
        .ok_or_else(|| McpToolError::new(-32602, "params must be an object"))?;
    let execution_id = required_string_param(params, "execution_id")?;
    let idempotency_key = required_string_param(params, "idempotency_key")?;
    let message = required_string_param(params, "message")?;
    let agent_id = required_string_param(params, "agent_id")?;
    let overrides = optional_overrides_param(params, "overrides")?;

    services::agent_service::require_agent_account(&state.db, &agent_id, user_id).await?;
    let execution = ExecutionRepo::get_by_id(&*state.db, &execution_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("execution", execution_id.to_owned()))?;
    require_task_access(state, &execution.task_id, user_id).await?;

    let launched = state
        .task_service
        .follow_up_execution(
            execution_id,
            message,
            Some(agent_id),
            overrides,
            idempotency_key,
        )
        .await?;
    let task = task_public_value(state, launched.task).await?;

    Ok(json!({
        "task": task,
        "execution": execution_value(launched.execution),
        "workspace": launched.workspace.map(|workspace| json!({
            "id": workspace.id,
            "task_id": workspace.task_id,
            "repo_id": workspace.repo_id,
            "worktree_path": workspace.worktree_path,
            "branch": workspace.branch,
            "status": workspace.status.to_string(),
            "before_sha": workspace.before_sha,
            "error": workspace.error,
            "created_at": workspace.created_at,
            "updated_at": workspace.updated_at,
        })),
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
        let configuration_record =
            db::TaskRoleCapabilityConfigurationRepo::get_by_task_role_id(&*state.db, &role.id)
                .await?
                .ok_or_else(|| {
                    McpToolError::new(-32603, "TaskRole capability configuration is missing")
                })?;
        let (policy, configuration) = task_role_policy_and_configuration(
            &role.policy_json,
            &configuration_record.configuration_json,
        )?;
        roles.push(json!({
            "id": role.id,
            "task_id": role.task_id,
            "role": role.role,
            "coordination_mode": role.coordination_mode.map(|mode| mode.to_string()),
            "policy": policy,
            "configuration": configuration,
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
    let task = require_task_access(state, &params.task_id, user_id).await?;
    require_project_admin_access(state, &task.project_id, user_id).await?;
    let policy = params.policy.unwrap_or_else(|| json!({}));
    let configuration = params.configuration.unwrap_or_default();
    let policy_json = serde_json::to_string(&policy)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    let configuration_json =
        services::task_role_configuration::store_task_role_configuration(&configuration)
            .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    let coordination_mode = match params.coordination_mode {
        PublicCoordinationMode::Partitioned => db::CoordinationMode::Partitioned,
        PublicCoordinationMode::Collaborative => db::CoordinationMode::Collaborative,
        PublicCoordinationMode::Independent => db::CoordinationMode::Independent,
    };
    let role = state
        .task_service
        .create_task_role_with_configuration(
            &params.task_id,
            &params.role,
            coordination_mode,
            policy_json,
            configuration_json.clone(),
        )
        .await?;
    let members = RoleMembershipRepo::list_by_role(&*state.db, &role.id, false).await?;
    task_role_value(role, members, &configuration_json)
}

pub(super) async fn forge_update_task_role(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let user_id = authenticated_user(context)?;
    let params: UpdateTaskRoleParams = parse_params(params)?;
    let task = require_task_access(state, &params.task_id, user_id).await?;
    require_project_admin_access(state, &task.project_id, user_id).await?;
    let role_name = db::canonical_task_role_name(&params.role)
        .ok_or_else(|| McpToolError::new(-32602, "role is not a TaskRole"))?;
    let role = TaskRoleRepo::get_by_task_and_role(&*state.db, &task.id, &role_name)
        .await?
        .ok_or_else(|| McpToolError::not_found("task_role", role_name.clone()))?;
    let current_configuration_record =
        db::TaskRoleCapabilityConfigurationRepo::get_by_task_role_id(&*state.db, &role.id)
            .await?
            .ok_or_else(|| {
                McpToolError::new(-32603, "TaskRole capability configuration is missing")
            })?;
    let (current_policy, current_configuration) = task_role_policy_and_configuration(
        &role.policy_json,
        &current_configuration_record.configuration_json,
    )?;
    let policy_changed = params.policy.is_some();
    let configuration_changed = params.configuration.is_some();
    let policy = params.policy.unwrap_or(current_policy);
    let policy_json = policy_changed
        .then(|| serde_json::to_string(&policy))
        .transpose()
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    let configuration = params.configuration.unwrap_or(current_configuration);
    let configuration_json = if configuration_changed {
        services::task_role_configuration::store_task_role_configuration(&configuration)
            .map_err(|error| McpToolError::new(-32602, error.to_string()))?
    } else {
        current_configuration_record.configuration_json
    };
    let coordination_mode = params.coordination_mode.map(|mode| match mode {
        PublicCoordinationMode::Partitioned => db::CoordinationMode::Partitioned,
        PublicCoordinationMode::Collaborative => db::CoordinationMode::Collaborative,
        PublicCoordinationMode::Independent => db::CoordinationMode::Independent,
    });
    let updated = state
        .task_service
        .update_task_role_with_configuration(
            &task.id,
            &role_name,
            params.expected_version,
            coordination_mode,
            policy_json,
            configuration_json.clone(),
        )
        .await?;
    let members = RoleMembershipRepo::list_by_role(&*state.db, &updated.id, true).await?;
    task_role_value(updated, members, &configuration_json)
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
    configuration_json: &str,
) -> Result<Value, McpToolError> {
    let (policy, configuration) =
        task_role_policy_and_configuration(&role.policy_json, configuration_json)?;
    Ok(json!({
        "id": role.id,
        "task_id": role.task_id,
        "role": role.role,
        "coordination_mode": role.coordination_mode.map(|mode| mode.to_string()),
        "policy": policy,
        "configuration": configuration,
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

fn task_role_policy_and_configuration(
    policy_raw: &str,
    configuration_raw: &str,
) -> Result<(Value, api_types::TaskRoleConfiguration), McpToolError> {
    let policy = serde_json::from_str::<Value>(policy_raw).map_err(|error| {
        McpToolError::new(-32603, format!("invalid stored TaskRole policy: {error}"))
    })?;
    let configuration =
        services::task_role_configuration::parse_task_role_configuration(configuration_raw)
            .map_err(|error| McpToolError::new(-32603, error.to_string()))?;
    Ok((policy, configuration))
}

pub(crate) async fn require_project_access(
    state: &AppState,
    project_id: &str,
    user_id: &str,
) -> Result<(), McpToolError> {
    let project = ProjectRepo::get_by_id(&*state.db, project_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("project", project_id.to_owned()))?;
    if project_is_visible_to_user(state, &project, user_id).await? {
        Ok(())
    } else {
        Err(McpToolError::new(-32001, "project not accessible"))
    }
}

pub(crate) async fn require_project_write_access(
    state: &AppState,
    project_id: &str,
    user_id: &str,
) -> Result<(), McpToolError> {
    require_project_access(state, project_id, user_id).await?;
    let project = ProjectRepo::get_by_id(&*state.db, project_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("project", project_id.to_owned()))?;
    if services::project_authority::user_can_write_project(&state.db, &project, user_id).await? {
        Ok(())
    } else {
        Err(McpToolError::new(
            -32003,
            "Project write access is required",
        ))
    }
}

pub(crate) async fn require_project_admin_access(
    state: &AppState,
    project_id: &str,
    user_id: &str,
) -> Result<(), McpToolError> {
    require_project_access(state, project_id, user_id).await?;
    let project = ProjectRepo::get_by_id(&*state.db, project_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("project", project_id.to_owned()))?;
    if services::project_authority::user_can_administer_project(&state.db, &project, user_id)
        .await?
    {
        Ok(())
    } else {
        Err(McpToolError::new(-32003, "Project admin role is required"))
    }
}

async fn project_is_visible_to_user(
    state: &AppState,
    project: &db::Project,
    user_id: &str,
) -> Result<bool, McpToolError> {
    // REST treats ownerless rows as public system Projects. Keep that
    // established visibility rule for user/local and Project-scoped MCP.
    Ok(services::project_authority::user_can_read_project(&state.db, project, user_id).await?)
}

pub(crate) async fn require_task_access(
    state: &AppState,
    task_id: &str,
    user_id: &str,
) -> Result<Task, McpToolError> {
    let task = TaskRepo::get_by_id(&*state.db, task_id, false)
        .await?
        .ok_or_else(|| McpToolError::not_found("task", task_id.to_owned()))?;
    require_project_access(state, &task.project_id, user_id).await?;
    Ok(task)
}

fn dispatch_service(state: &AppState) -> services::task_lifecycle::TaskLifecycleService {
    services::task_lifecycle::TaskLifecycleService::new(
        Arc::clone(&state.db),
        Arc::clone(&state.event_bus),
    )
}

fn dispatch_intent_value(dispatch: &db::TaskDispatch) -> Value {
    json!({
        "task_id": dispatch.task_id,
        "state": dispatch.state.to_string(),
        "version": dispatch.version,
        "requested_by": match (
            dispatch.requested_by_kind.as_str(),
            dispatch.requested_by_id.as_deref(),
        ) {
            ("human", Some(id)) => Some(json!({"kind": "human", "id": id})),
            ("agent", Some(id)) => Some(json!({"kind": "agent", "id": id})),
            _ => None,
        },
        "reason": dispatch.reason,
        "created_at": dispatch.created_at,
        "updated_at": dispatch.updated_at,
    })
}

async fn dispatch_action_value(
    state: &AppState,
    task_id: &str,
    write: db::TaskDispatchWrite,
) -> Result<Value, McpToolError> {
    let task = TaskRepo::get_by_id(&*state.db, task_id, false)
        .await?
        .ok_or_else(|| McpToolError::not_found("task", task_id.to_owned()))?;
    let task = task_public_value(state, task).await?;
    let preflight = dispatch_service(state).launch_preflight(task_id).await?;
    Ok(json!({
        "task": task,
        "dispatch": dispatch_intent_value(&write.dispatch),
        "preflight": preflight,
    }))
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
    for execution in page.items {
        if !services::role_capabilities::execution_has_capability(
            &state.db,
            &execution.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?
        {
            continue;
        }
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
        .ok_or_else(|| McpToolError::not_found("review_execution", params.execution_id))?;
    if !services::role_capabilities::execution_has_capability(
        &state.db,
        &execution.id,
        api_types::TaskRoleCapability::FormalReview,
    )
    .await?
    {
        return Err(McpToolError::not_found("review_execution", execution.id));
    }
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
