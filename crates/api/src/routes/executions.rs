use api_types::{ExecutionResponse, FollowUpRequest, PaginatedResponse, StartExecutionRequest};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::{ExecutionRepo, ExecutionUsageRepo};
use executors::{ExecutionOverrides, LogReader};
use serde::Deserialize;
use services::ServiceError;

use crate::{
    errors::{ApiError, ApiResult},
    routes::{
        execution_response, execution_response_with_usage, execution_usage_response, page_request,
        paginated, task_usage_summary_response, ListParams,
    },
    state::AppState,
};

pub async fn list_executions(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<PaginatedResponse<ExecutionResponse>>> {
    let page = ExecutionRepo::list_by_task(&*state.db, &task_id, page_request(&params)?).await?;
    Ok(Json(paginated(page, execution_response)))
}

pub async fn start_task_execution(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
    Json(request): Json<StartExecutionRequest>,
) -> ApiResult<Json<ExecutionResponse>> {
    crate::routes::tasks::require_task_visible(&state, &task_id, &user).await?;
    if request.role.trim().is_empty() || request.prompt.trim().is_empty() {
        return Err(ApiError::bad_request(
            "role and prompt must be non-empty for an Execution",
        ));
    }
    let purpose = match request.purpose {
        api_types::ExecutionPurpose::Plan => db::ExecutionPurpose::Plan,
        api_types::ExecutionPurpose::Implement => db::ExecutionPurpose::Implement,
        api_types::ExecutionPurpose::Review => db::ExecutionPurpose::Review,
        api_types::ExecutionPurpose::Validate => db::ExecutionPurpose::Validate,
        api_types::ExecutionPurpose::Investigate => db::ExecutionPurpose::Investigate,
        api_types::ExecutionPurpose::Orchestrate => db::ExecutionPurpose::Orchestrate,
        api_types::ExecutionPurpose::General => db::ExecutionPurpose::General,
    };
    let execution = state
        .task_service
        .dispatch_initial_role_execution_with_artifacts(
            &task_id,
            &request.agent_id,
            &request.role,
            purpose,
            request.prompt,
            request.input_artifact_ids,
            None,
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(
        execution_response_with_usage(&state.db, execution).await?,
    ))
}

pub async fn get_execution(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ExecutionResponse>> {
    let execution = ExecutionRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("execution", id))?;
    Ok(Json(
        execution_response_with_usage(&state.db, execution).await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    pub tail: Option<usize>,
    pub from_sequence: Option<u64>,
    pub limit: Option<usize>,
}

const DEFAULT_LOG_LIMIT: usize = 200;
const MAX_LOG_LIMIT: usize = 1_000;

pub async fn get_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<LogsQuery>,
) -> ApiResult<Json<api_types::ExecutionLogsResponse>> {
    let execution = ExecutionRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("execution", id.clone()))?;

    let Some(path) = execution.logs_path else {
        return Ok(Json(api_types::ExecutionLogsResponse {
            items: Vec::new(),
            has_more: false,
            next_sequence: None,
        }));
    };

    let log_path = std::path::Path::new(&path);

    let limit = params
        .limit
        .unwrap_or(DEFAULT_LOG_LIMIT)
        .clamp(1, MAX_LOG_LIMIT);
    let result = if let Some(n) = params.tail {
        LogReader::tail(log_path, n.clamp(1, MAX_LOG_LIMIT)).await
    } else {
        LogReader::read(log_path, params.from_sequence.unwrap_or(0), limit).await
    };

    let result = match result {
        Ok(r) => r,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::execution_logs_unavailable(format!(
                "log file not found for execution {id}"
            )));
        }
        Err(error) => return Err(ApiError::bad_request(error.to_string())),
    };

    Ok(Json(api_types::ExecutionLogsResponse {
        items: result
            .entries
            .into_iter()
            .map(execution_log_response)
            .collect(),
        has_more: result.has_more,
        next_sequence: result.next_sequence,
    }))
}

fn execution_log_response(entry: executors::LogEntry) -> api_types::ExecutionLogEntry {
    use api_types::{ExecutionLogKind as PublicKind, ExecutionLogStream as PublicStream};
    let kind = match entry.kind {
        executors::LogKind::Stdout => PublicKind::Stdout,
        executors::LogKind::Stderr => PublicKind::Stderr,
        executors::LogKind::ToolCall => PublicKind::ToolCall,
        executors::LogKind::ToolResult => PublicKind::ToolResult,
        executors::LogKind::Assistant => PublicKind::Assistant,
        executors::LogKind::AssistantDelta => PublicKind::AssistantDelta,
        executors::LogKind::User => PublicKind::User,
        executors::LogKind::System => PublicKind::System,
        executors::LogKind::FileChange => PublicKind::FileChange,
        executors::LogKind::ShellCommand => PublicKind::ShellCommand,
        executors::LogKind::ApprovalQuestion => PublicKind::ApprovalQuestion,
        executors::LogKind::SessionInfo => PublicKind::SessionInfo,
        executors::LogKind::Unknown => PublicKind::Unknown,
    };
    let stream = match entry.stream {
        executors::LogStream::Main => PublicStream::Main,
        executors::LogStream::Heartbeat => PublicStream::Heartbeat,
    };
    api_types::ExecutionLogEntry {
        schema_version: entry.schema_version,
        sequence: entry.sequence,
        timestamp: entry.timestamp,
        execution_id: entry.execution_id,
        kind,
        stream,
        payload: entry.payload,
        truncated: entry.truncated,
    }
}

pub async fn get_hook_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let execution = ExecutionRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("execution", id.clone()))?;
    let Some(logs_path) = execution.logs_path.as_deref() else {
        return Ok(Json(Vec::new()));
    };
    let log_dir = match std::path::Path::new(logs_path).parent() {
        Some(dir) => dir,
        None => return Ok(Json(Vec::new())),
    };
    let entries = match std::fs::read_dir(log_dir) {
        Ok(e) => e,
        Err(_) => return Ok(Json(Vec::new())),
    };
    let mut hook_entries = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("hook-") || !name.ends_with(".jsonl") {
            continue;
        }
        let content = match std::fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for line in content.lines() {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                hook_entries.push(value);
            }
        }
    }
    Ok(Json(hook_entries))
}

pub async fn follow_up_execution(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<FollowUpRequest>,
) -> ApiResult<Json<ExecutionResponse>> {
    let overrides = request.overrides.map(|overrides| ExecutionOverrides {
        model_id: overrides.model_id,
        reasoning_effort: overrides.reasoning_effort,
        permission_policy: overrides.permission_policy,
    });
    let launched = state
        .task_service
        .follow_up_execution(id, request.message, Some(request.agent_id), overrides)
        .await
        .map_err(map_follow_up_error)?;

    let execution_id = launched.execution.id.clone();
    state.task_service.start_execution(execution_id).await?;
    Ok(Json(
        execution_response_with_usage(&state.db, launched.execution).await?,
    ))
}

pub async fn cancel_execution(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ExecutionResponse>> {
    let execution = state
        .task_service
        .cancel_execution(id, "cancelled by user".to_owned())
        .await?;
    Ok(Json(execution_response(execution)))
}

fn map_follow_up_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::InvalidOperation { message } => {
            if message.contains("follow-up requires a completed, failed, or cancelled execution") {
                ApiError::conflict_with_code("follow_up.execution_active", message)
            } else if message.contains("parent execution has no resumable session") {
                ApiError::conflict_with_code("follow_up.no_session", message)
            } else if message.contains("follow-up requires same executor type") {
                ApiError::conflict_with_code("follow_up.executor_mismatch", message)
            } else if message.contains("interactive execution already running") {
                ApiError::conflict_with_code("execution.already_running", message)
            } else if message.contains("terminal status") {
                ApiError::conflict_with_code("task.terminal", message)
            } else {
                ApiError::invalid_operation_conflict(message)
            }
        }
        other => ApiError::from(other),
    }
}

pub async fn get_execution_usage(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<api_types::ExecutionUsageResponse>>> {
    let _ = ExecutionRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("execution", id.clone()))?;
    let usage = ExecutionUsageRepo::list_by_execution(&*state.db, &id).await?;
    Ok(Json(
        usage.into_iter().map(execution_usage_response).collect(),
    ))
}

pub async fn get_task_usage(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> ApiResult<Json<api_types::TaskUsageSummaryResponse>> {
    let summary = ExecutionUsageRepo::get_task_usage_summary(&*state.db, &task_id).await?;
    Ok(Json(task_usage_summary_response(summary)))
}
