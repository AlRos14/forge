use api_types::{
    parse_project_hooks_json, CreateProjectRequest, PaginatedResponse, ProjectHookRunResponse,
    ProjectHookRunStatus, ProjectHookRunsResponse, ProjectResponse, UpdateProjectRequest,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use db::{
    new_uuid_v4, now_rfc3339, CreateProject, PageRequest, ProjectHookRun, ProjectHookRunRepo,
    ProjectRepo, SortBy, SortOrder, UpdateProject,
};
use events::{event_timestamp, EventContext, ForgeEvent};
use services::workflow::default_workflow::default_workflow;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    routes::{page_request, project_response, ListParams},
    state::AppState,
};

pub async fn create_project(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(request): Json<CreateProjectRequest>,
) -> ApiResult<Response> {
    let now = now_rfc3339();
    let settings = serde_json::json!({});
    let workflow_definition = serde_json::to_string(&default_workflow())
        .map_err(|error| ApiError::internal(format!("serialize default workflow: {error}")))?;
    let settings = serialize_settings(&settings)?;
    let project = ProjectRepo::create(
        &*state.db,
        CreateProject {
            id: new_uuid_v4(),
            name: request.name,
            settings,
            workflow_definition,
            primary_repo_id: None,
            owner_id: Some(user.user_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await?;

    // Auto-create owner membership (best-effort; may fail if user row doesn't exist yet)
    let _ = db::ProjectMemberRepo::add_member(
        &*state.db,
        db::CreateProjectMember {
            id: new_uuid_v4(),
            project_id: project.id.clone(),
            user_id: user.user_id,
            role: "owner".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await;

    state.event_bus.publish(ForgeEvent {
        event_type: "project.created".to_owned(),
        entity_id: project.id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::ProjectCreated {
            name: project.name.clone(),
        },
    });

    Ok((StatusCode::OK, Json(project_response(project)?)).into_response())
}

pub async fn list_projects(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<PaginatedResponse<ProjectResponse>>> {
    let page = ProjectRepo::list(&*state.db, page_request(&params)?).await?;
    let has_more = page.next_cursor.is_some();
    let next_cursor = page.next_cursor;
    let total_count = page.total_count.and_then(|count| u64::try_from(count).ok());
    // Owner visibility is canonical even if a legacy/direct-create row was
    // left without its best-effort membership row.  Membership additionally
    // grants visibility to collaborators; owner_id = NULL denotes a public
    // system Project.
    let mut visible_items = Vec::new();
    for project in page.items {
        if project_is_visible(&state, &project, &user.user_id).await? {
            visible_items.push(project);
        }
    }
    let response = PaginatedResponse {
        items: visible_items
            .into_iter()
            .map(project_response)
            .collect::<ApiResult<Vec<_>>>()?,
        next_cursor,
        has_more,
        total_count,
    };
    Ok(Json(response))
}

pub async fn get_project(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<ProjectResponse>> {
    let project = ProjectRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", id.clone()))?;
    if !project_is_visible(&state, &project, &user.user_id).await? {
        return Err(ApiError::not_found("project", id));
    }
    Ok(Json(project_response(project)?))
}

pub async fn list_project_hook_runs(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<ProjectHookRunsResponse>> {
    require_project_visible(&state, &id, &user.user_id).await?;
    let page = ProjectHookRunRepo::list_for_project(
        &*state.db,
        &id,
        PageRequest {
            cursor: params.cursor,
            limit: params.limit.unwrap_or(20).clamp(1, 100),
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    Ok(Json(ProjectHookRunsResponse {
        items: page
            .items
            .into_iter()
            .map(project_hook_run_response)
            .collect(),
        next_cursor: page.next_cursor,
    }))
}

pub async fn delete_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    services::project_deletion::delete_project(
        state.db.clone(),
        state.cleanup_scheduler.workspace_root(),
        &id,
    )
    .await?;
    state.event_bus.publish(ForgeEvent {
        event_type: "project.deleted".to_owned(),
        entity_id: id,
        timestamp: event_timestamp(),
        context: EventContext::ProjectDeleted {},
    });
    Ok(StatusCode::NO_CONTENT)
}

pub async fn pause_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ProjectResponse>> {
    let project = ProjectRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", id.clone()))?;
    if project.paused_at.is_some() {
        return Ok(Json(project_response(project)?));
    }

    let paused_at = now_rfc3339();
    ProjectRepo::set_paused_at(&*state.db, &id, Some(paused_at.clone())).await?;
    let project = ProjectRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", id.clone()))?;
    tracing::info!(project_id = %project.id, project_name = %project.name, "project paused");
    state.event_bus.publish(ForgeEvent {
        event_type: "project.paused".to_owned(),
        entity_id: project.id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::ProjectPaused { paused_at },
    });

    Ok(Json(project_response(project)?))
}

pub async fn resume_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ProjectResponse>> {
    let project = ProjectRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", id.clone()))?;
    if project.paused_at.is_none() {
        return Ok(Json(project_response(project)?));
    }

    ProjectRepo::set_paused_at(&*state.db, &id, None).await?;
    let project = ProjectRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", id.clone()))?;
    tracing::info!(project_id = %project.id, project_name = %project.name, "project resumed");
    state.event_bus.publish(ForgeEvent {
        event_type: "project.resumed".to_owned(),
        entity_id: project.id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::ProjectResumed {},
    });

    Ok(Json(project_response(project)?))
}

pub async fn update_project(
    State(state): State<AppState>,
    _user: AuthenticatedUser,
    Path(id): Path<String>,
    Json(request): Json<UpdateProjectRequest>,
) -> ApiResult<Json<ProjectResponse>> {
    let UpdateProjectRequest {
        name,
        paused,
        project_hooks,
        version,
    } = request;
    let project_hooks_json = match project_hooks {
        Some(rules) => {
            let serialized = serde_json::to_string(&rules).map_err(|error| {
                ApiError::bad_request(format!("invalid project hooks: {error}"))
            })?;
            parse_project_hooks_json(&serialized).map_err(ApiError::bad_request)?;
            Some(serialized)
        }
        None => None,
    };
    let project = ProjectRepo::update_at_version(
        &*state.db,
        UpdateProject {
            id,
            name,
            settings: None,
            primary_repo_id: None,
            paused_at: paused.map(|paused: bool| paused.then(now_rfc3339)),
            updated_at: now_rfc3339(),
        },
        version,
        project_hooks_json,
    )
    .await?;
    state.event_bus.publish(ForgeEvent {
        event_type: "project.updated".to_owned(),
        entity_id: project.id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::ProjectUpdated {},
    });

    Ok(Json(project_response(project)?))
}

async fn require_project_visible(
    state: &AppState,
    project_id: &str,
    user_id: &str,
) -> ApiResult<db::Project> {
    let project = ProjectRepo::get_by_id(&*state.db, project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", project_id.to_owned()))?;
    if !project_is_visible(state, &project, user_id).await? {
        return Err(ApiError::not_found("project", project_id.to_owned()));
    }
    Ok(project)
}

async fn project_is_visible(
    state: &AppState,
    project: &db::Project,
    user_id: &str,
) -> ApiResult<bool> {
    if project.owner_id.is_none() || project.owner_id.as_deref() == Some(user_id) {
        return Ok(true);
    }
    Ok(
        db::ProjectMemberRepo::get_member(&*state.db, &project.id, user_id)
            .await?
            .is_some(),
    )
}

fn project_hook_run_response(run: ProjectHookRun) -> ProjectHookRunResponse {
    ProjectHookRunResponse {
        id: run.id,
        project_id: run.project_id,
        rule_id: run.rule_id,
        trigger_type: run.trigger_type,
        dedupe_key: run.dedupe_key,
        status: match run.status {
            db::ProjectHookRunStatus::Queued => ProjectHookRunStatus::Queued,
            db::ProjectHookRunStatus::Running => ProjectHookRunStatus::Running,
            db::ProjectHookRunStatus::Dispatched => ProjectHookRunStatus::Dispatched,
            db::ProjectHookRunStatus::Skipped => ProjectHookRunStatus::Skipped,
            db::ProjectHookRunStatus::Failed => ProjectHookRunStatus::Failed,
            db::ProjectHookRunStatus::Completed => ProjectHookRunStatus::Completed,
        },
        source_task_id: run.source_task_id,
        source_execution_id: run.source_execution_id,
        automation_task_id: run.automation_task_id,
        execution_id: run.execution_id,
        agent_id: run.agent_id,
        reason: run.reason,
        created_at: run.created_at,
        updated_at: run.updated_at,
        completed_at: run.completed_at,
    }
}

fn serialize_settings(settings: &serde_json::Value) -> ApiResult<String> {
    serde_json::to_string(settings)
        .map_err(|error| ApiError::bad_request(format!("invalid settings: {error}")))
}
