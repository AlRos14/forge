use api_types::{SaveWorkflowTemplateRequest, WorkflowTemplateResponse, WorkflowTemplateSummary};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use services::ServiceError;

use crate::{
    errors::{ApiError, ApiResult},
    state::AppState,
};

pub async fn list_templates(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<WorkflowTemplateSummary>>> {
    let templates = state
        .workflow_template_service
        .list_templates()
        .await
        .map_err(workflow_template_service_error)?;
    Ok(Json(templates))
}

pub async fn get_template(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<WorkflowTemplateResponse>> {
    let template = state
        .workflow_template_service
        .get_template(&name)
        .await
        .map_err(|error| workflow_template_named_error(&name, error))?;
    Ok(Json(template))
}

pub async fn save_template(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<SaveWorkflowTemplateRequest>,
) -> ApiResult<Json<WorkflowTemplateResponse>> {
    let display_name = request
        .display_name
        .unwrap_or_else(|| default_display_name(&name));
    let description = request.description.unwrap_or_default();

    state
        .workflow_template_service
        .save_template(&name, display_name, description, request.definition)
        .await
        .map_err(|error| workflow_template_named_error(&name, error))?;

    let template = state
        .workflow_template_service
        .get_template(&name)
        .await
        .map_err(|error| workflow_template_named_error(&name, error))?;
    Ok(Json(template))
}

pub async fn delete_template(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    if name == "default" {
        return Err(ApiError::forbidden_with_code(
            "BUILTIN_TEMPLATE",
            "the default template cannot be deleted",
        ));
    }
    state
        .workflow_template_service
        .delete_template(&name)
        .await
        .map_err(|error| workflow_template_named_error(&name, error))?;
    Ok(StatusCode::NO_CONTENT)
}

fn workflow_template_service_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::InvalidOperation { message } => ApiError::bad_request(message),
        other => ApiError::from(other),
    }
}

fn workflow_template_named_error(name: &str, error: ServiceError) -> ApiError {
    match error {
        ServiceError::NotFound { .. } => ApiError::not_found("workflow_template", name),
        ServiceError::InvalidOperation { message } => ApiError::bad_request(message),
        other => ApiError::from(other),
    }
}

fn default_display_name(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut display_name = String::with_capacity(segment.len());
            display_name.push(first.to_ascii_uppercase());
            display_name.extend(chars);
            display_name
        })
        .collect::<Vec<_>>()
        .join(" ")
}
