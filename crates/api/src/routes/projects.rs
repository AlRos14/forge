use std::collections::HashSet;

use api_types::{
    CiStepAnalytics, CreateProjectRequest, ModelTokenBreakdown as ApiModelTokenBreakdown,
    PaginatedResponse, ProjectAnalyticsResponse, ProjectResponse, ProjectSettings, ReviewConfig,
    ReviewSummaryAnalytics, StateKind, TestLifecycleHookRequest, TokenUsageAnalytics,
    UpdateProjectRequest, UpdateProjectWorkflowRequest, WorkflowDefinition,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use db::{
    new_uuid_v4, now_rfc3339, CiStepStats, CreateProject, ModelTokenBreakdown,
    ProjectAnalyticsRepo, ProjectRepo, ProjectReviewSummary, ProjectTokenStats, UpdateProject,
};
use events::{event_timestamp, EventContext, ForgeEvent};
use serde::Deserialize;
use services::{
    workflow::{engine::WorkflowEngine, validation::validate_workflow},
    ServiceError,
};

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    routes::{page_request, project_response, ListParams},
    state::AppState,
};

const DEFAULT_REVIEW_CONFIG_KEY: &str = "default_review_config";

#[derive(Debug, Deserialize)]
pub struct AnalyticsQuery {
    pub from: Option<String>,
    pub to: Option<String>,
}

pub async fn create_project(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(request): Json<CreateProjectRequest>,
) -> ApiResult<Json<ProjectResponse>> {
    let now = now_rfc3339();
    let mut settings = request.settings.unwrap_or_else(|| serde_json::json!({}));
    apply_default_review_config(&mut settings, request.default_review_config.as_ref())?;
    let workflow = WorkflowEngine::resolve_workflow("{}");
    validate_project_settings(&state.db, &settings, &workflow, None, None).await?;
    let settings = serialize_settings(&settings)?;
    let project = ProjectRepo::create(
        &*state.db,
        CreateProject {
            id: new_uuid_v4(),
            name: request.name,
            settings,
            workflow_definition: "{}".to_string(),
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

    Ok(Json(project_response(project)))
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
    // Filter: keep projects where the user is a member OR owner_id is None (system projects)
    let mut visible_items = Vec::new();
    for project in page.items {
        if project.owner_id.is_none() {
            visible_items.push(project);
        } else {
            let is_member =
                db::ProjectMemberRepo::get_member(&*state.db, &project.id, &user.user_id)
                    .await?
                    .is_some();
            if is_member {
                visible_items.push(project);
            }
        }
    }
    let response = PaginatedResponse {
        items: visible_items.into_iter().map(project_response).collect(),
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
    // If project has an owner, verify user is a member
    if project.owner_id.is_some() {
        let is_member = db::ProjectMemberRepo::get_member(&*state.db, &project.id, &user.user_id)
            .await?
            .is_some();
        if !is_member {
            return Err(ApiError::not_found("project", id));
        }
    }
    Ok(Json(project_response(project)))
}

pub async fn get_project_analytics(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<AnalyticsQuery>,
) -> Result<Json<ProjectAnalyticsResponse>, ApiError> {
    ProjectRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", id.clone()))?;

    let from = params.from.as_deref();
    let to = params.to.as_deref();

    let ci_steps = ProjectAnalyticsRepo::get_project_ci_analytics(&*state.db, &id, from, to)
        .await?
        .into_iter()
        .map(
            |CiStepStats {
                 command,
                 total_runs,
                 pass_count,
                 fail_count,
                 avg_duration_ms,
                 p50_duration_ms,
                 p95_duration_ms,
                 last_run_at,
             }| CiStepAnalytics {
                command,
                total_runs,
                pass_count,
                fail_count,
                success_rate: if total_runs > 0 {
                    pass_count as f64 / total_runs as f64
                } else {
                    0.0
                },
                avg_duration_ms,
                p50_duration_ms,
                p95_duration_ms,
                last_run_at,
            },
        )
        .collect();

    let token_usage =
        ProjectAnalyticsRepo::get_project_token_analytics(&*state.db, &id, from, to).await?;
    let ProjectTokenStats {
        total_input_tokens,
        total_output_tokens,
        total_cache_read_tokens,
        total_cache_write_tokens,
        total_cost_usd,
        execution_count,
        by_model,
    } = token_usage;
    let token_usage = TokenUsageAnalytics {
        total_input_tokens,
        total_output_tokens,
        total_cache_read_tokens,
        total_cache_write_tokens,
        total_cost_usd,
        execution_count,
        by_model: by_model
            .into_iter()
            .map(
                |ModelTokenBreakdown {
                     provider,
                     model,
                     input_tokens,
                     output_tokens,
                     cache_read_tokens,
                     cache_write_tokens,
                     cost_usd,
                     execution_count,
                 }| ApiModelTokenBreakdown {
                    provider,
                    model,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    cost_usd,
                    execution_count,
                },
            )
            .collect(),
    };

    let review_summary =
        ProjectAnalyticsRepo::get_project_review_summary(&*state.db, &id, from, to).await?;
    let review_summary = review_summary_analytics(review_summary);

    Ok(Json(ProjectAnalyticsResponse {
        ci_steps,
        token_usage,
        review_summary,
    }))
}

pub async fn get_project_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<WorkflowDefinition>> {
    let project = ProjectRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", id))?;
    Ok(Json(WorkflowEngine::resolve_workflow(
        &project.workflow_definition,
    )))
}

pub async fn update_project_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateProjectWorkflowRequest>,
) -> ApiResult<Json<WorkflowDefinition>> {
    let UpdateProjectWorkflowRequest {
        template_name,
        definition,
    } = request;
    let (definition, workflow_template_name) = if let Some(template_name) = template_name {
        let template = state
            .workflow_template_service
            .get_template(&template_name)
            .await
            .map_err(|error| workflow_template_service_error(&template_name, error))?;
        (template.definition, Some(template_name))
    } else if let Some(definition) = definition {
        (definition, None)
    } else {
        return Err(ApiError::bad_request(
            "either template_name or definition must be provided",
        ));
    };
    validate_workflow(&definition).map_err(workflow_validation_error)?;

    let project = ProjectRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", id.clone()))?;
    let old_workflow = WorkflowEngine::resolve_workflow(&project.workflow_definition);
    validate_workflow_update_safety(&state.db, &id, &old_workflow, &definition).await?;

    let workflow_definition = serde_json::to_string(&definition)?;
    let updated_at = now_rfc3339();
    let result = sqlx::query(
        "UPDATE project SET workflow_definition = ?, workflow_template_name = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&workflow_definition)
    .bind(&workflow_template_name)
    .bind(&updated_at)
    .bind(&id)
    .execute(state.db.pool())
    .await
    .map_err(db::DbError::from)?;

    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("project", id));
    }

    Ok(Json(definition))
}

pub async fn delete_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    ProjectRepo::delete(&*state.db, &id).await?;
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
        return Ok(Json(project_response(project)));
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

    Ok(Json(project_response(project)))
}

pub async fn resume_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ProjectResponse>> {
    let project = ProjectRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", id.clone()))?;
    if project.paused_at.is_none() {
        return Ok(Json(project_response(project)));
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

    Ok(Json(project_response(project)))
}

pub async fn update_project(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Json(request): Json<UpdateProjectRequest>,
) -> ApiResult<Json<ProjectResponse>> {
    let settings = update_settings(
        &state.db,
        &id,
        &user.user_id,
        request.settings,
        request.default_review_config.as_ref(),
    )
    .await?;
    let project = ProjectRepo::update(
        &*state.db,
        UpdateProject {
            id,
            name: request.name,
            settings,
            primary_repo_id: request.primary_repo_id.map(Some),
            paused_at: request.paused.map(|paused: bool| paused.then(now_rfc3339)),
            updated_at: now_rfc3339(),
        },
    )
    .await?;
    state.event_bus.publish(ForgeEvent {
        event_type: "project.updated".to_owned(),
        entity_id: project.id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::ProjectUpdated {},
    });

    Ok(Json(project_response(project)))
}

pub async fn test_project_lifecycle_hook(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<TestLifecycleHookRequest>,
) -> ApiResult<Json<api_types::LifecycleHookTestResponse>> {
    let response = state
        .task_service
        .test_lifecycle_hook(&id, &request.task_id, request.event, request.hook_index)
        .await
        .map_err(|error| match error {
            ServiceError::InvalidOperation { message } => ApiError::bad_request(message),
            other => ApiError::from(other),
        })?;
    Ok(Json(response))
}

async fn validate_workflow_update_safety(
    db: &db::SqliteDb,
    project_id: &str,
    old_workflow: &WorkflowDefinition,
    new_workflow: &WorkflowDefinition,
) -> ApiResult<()> {
    let new_non_terminal_states: HashSet<&str> = new_workflow
        .states
        .iter()
        .filter(|state| state.kind != StateKind::Terminal)
        .map(|state| state.name.as_str())
        .collect();
    let old_terminal_states: HashSet<&str> = old_workflow
        .states
        .iter()
        .filter(|state| state.kind == StateKind::Terminal)
        .map(|state| state.name.as_str())
        .collect();
    let statuses = sqlx::query_as::<_, (String, i64)>(
        "SELECT status, COUNT(*) FROM task WHERE project_id = ? AND deleted_at IS NULL GROUP BY status",
    )
    .bind(project_id)
    .fetch_all(db.pool())
    .await
    .map_err(db::DbError::from)?;

    for (status, count) in statuses {
        if !new_non_terminal_states.contains(status.as_str())
            && !old_terminal_states.contains(status.as_str())
        {
            return Err(ApiError::conflict_with_code(
                "workflow_state_in_use",
                format!("cannot remove state {status}: {count} active tasks in this state"),
            ));
        }
    }

    Ok(())
}

fn workflow_validation_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::InvalidOperation { message } => ApiError::bad_request(message),
        other => ApiError::from(other),
    }
}

fn workflow_template_service_error(name: &str, error: ServiceError) -> ApiError {
    match error {
        ServiceError::NotFound { .. } => ApiError::not_found("workflow_template", name),
        ServiceError::InvalidOperation { message } => ApiError::bad_request(message),
        other => ApiError::from(other),
    }
}

async fn update_settings(
    db: &db::SqliteDb,
    project_id: &str,
    user_id: &str,
    settings: Option<serde_json::Value>,
    default_review_config: Option<&ReviewConfig>,
) -> ApiResult<Option<String>> {
    if settings.is_none() && default_review_config.is_none() {
        return Ok(None);
    }

    let project = ProjectRepo::get_by_id(db, project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", project_id.to_owned()))?;

    let mut settings = match settings {
        Some(settings) => settings,
        None => serde_json::from_str(&project.settings)
            .map_err(|error| ApiError::bad_request(format!("invalid settings: {error}")))?,
    };
    apply_default_review_config(&mut settings, default_review_config)?;
    let workflow = WorkflowEngine::resolve_workflow(&project.workflow_definition);
    validate_project_settings(db, &settings, &workflow, Some(project_id), Some(user_id)).await?;
    Ok(Some(serialize_settings(&settings)?))
}

async fn validate_project_settings(
    db: &db::SqliteDb,
    settings: &serde_json::Value,
    workflow: &WorkflowDefinition,
    project_id: Option<&str>,
    user_id: Option<&str>,
) -> ApiResult<()> {
    let settings: ProjectSettings = serde_json::from_value(settings.clone())
        .map_err(|error| ApiError::bad_request(format!("invalid settings: {error}")))?;
    let role_names: HashSet<&str> = workflow
        .roles
        .iter()
        .map(|role| role.name.as_str())
        .collect();

    for assignment in &settings.default_role_assignments {
        if !role_names.contains(assignment.role_name.as_str()) {
            return Err(ApiError::bad_request(format!(
                "unknown role: {}",
                assignment.role_name
            )));
        }

        match assignment.assignee_type.as_str() {
            "agent" => {
                if option_is_blank(assignment.assignee_id.as_ref()) {
                    return Err(ApiError::bad_request(format!(
                        "default role assignment for role '{}' requires assignee_id",
                        assignment.role_name
                    )));
                }
                if let (Some(project_id), Some(user_id), Some(assignee_id)) =
                    (project_id, user_id, assignment.assignee_id.as_ref())
                {
                    let usable_agents = db
                        .list_agents_usable_in_project(project_id, user_id)
                        .await
                        .map_err(ApiError::from)?;
                    let is_usable = usable_agents
                        .into_iter()
                        .any(|agent| agent.id == *assignee_id);
                    if !is_usable {
                        return Err(ApiError::bad_request("agent not usable in this project"));
                    }
                }
            }
            "user" => {
                if option_is_blank(assignment.assignee_id.as_ref()) {
                    return Err(ApiError::bad_request(format!(
                        "default role assignment for role '{}' requires assignee_id",
                        assignment.role_name
                    )));
                }
                if is_legacy_manual_default_assignee(assignment.assignee_id.as_deref()) {
                    continue;
                }
                if let (Some(project_id), Some(assignee_id)) =
                    (project_id, assignment.assignee_id.as_ref())
                {
                    let member =
                        db::ProjectMemberRepo::get_member(db, project_id, assignee_id).await?;
                    if member.is_none() {
                        return Err(ApiError::bad_request("assignee must be a project member"));
                    }
                }
            }
            _ => {
                return Err(ApiError::bad_request(format!(
                    "default role assignment for role '{}' must use assignee_type 'agent' or 'user'",
                    assignment.role_name
                )));
            }
        }
    }

    for (name, value) in [
        ("review", settings.retry_budgets.review),
        ("merge_fix", settings.retry_budgets.merge_fix),
    ] {
        if value.is_some_and(|value| value < 0) {
            return Err(ApiError::bad_request(format!(
                "retry_budgets.{name} must be 0 or greater"
            )));
        }
    }

    for (event, hooks) in &settings.lifecycle_hooks {
        for hook in hooks {
            if let api_types::LifecycleHookDef::Script { blocking, .. } = hook {
                if *blocking && *event != api_types::LifecycleEvent::BeforeWork {
                    return Err(ApiError::bad_request(
                        "blocking lifecycle hooks are only supported for before_work",
                    ));
                }
            }
        }
    }

    Ok(())
}

fn option_is_blank(value: Option<&String>) -> bool {
    value.map(|value| value.trim().is_empty()).unwrap_or(true)
}

fn is_legacy_manual_default_assignee(assignee_id: Option<&str>) -> bool {
    assignee_id == Some("human")
}

fn apply_default_review_config(
    settings: &mut serde_json::Value,
    default_review_config: Option<&ReviewConfig>,
) -> ApiResult<()> {
    let Some(default_review_config) = default_review_config else {
        return Ok(());
    };
    let settings = settings.as_object_mut().ok_or_else(|| {
        ApiError::bad_request("settings must be a JSON object when default_review_config is set")
    })?;
    let value = serde_json::to_value(default_review_config).map_err(|error| {
        ApiError::bad_request(format!("invalid default_review_config: {error}"))
    })?;
    settings.insert(DEFAULT_REVIEW_CONFIG_KEY.to_owned(), value);
    Ok(())
}

fn serialize_settings(settings: &serde_json::Value) -> ApiResult<String> {
    serde_json::to_string(settings)
        .map_err(|error| ApiError::bad_request(format!("invalid settings: {error}")))
}

fn review_summary_analytics(summary: ProjectReviewSummary) -> ReviewSummaryAnalytics {
    ReviewSummaryAnalytics {
        total_reviews: summary.total_reviews,
        passed: summary.passed,
        failed: summary.failed,
        cancelled: summary.cancelled,
        avg_duration_ms: summary.avg_duration_ms,
        pass_rate: summary.pass_rate,
    }
}

#[cfg(test)]
mod tests {
    use super::is_legacy_manual_default_assignee;

    #[test]
    fn recognizes_legacy_manual_default_assignee() {
        assert!(is_legacy_manual_default_assignee(Some("human")));
        assert!(!is_legacy_manual_default_assignee(Some("user-123")));
        assert!(!is_legacy_manual_default_assignee(None));
    }
}
