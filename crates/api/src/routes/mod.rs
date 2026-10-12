use std::str::FromStr;

use api_types::{
    parse_project_hooks_json, AgentResponse, DaemonResponse, ExecutionResponse, PaginatedResponse,
    ProjectResponse, RepoResponse, RoleMembershipResponse, RoleMembershipStatus, TaskResponse,
    TaskRoleResponse, TaskType, WorkspaceResponse,
};
use db::{
    ActorKind, Agent, CoordinationMode as DbCoordinationMode, Daemon, Execution, Page, PageRequest,
    Project, Repo, RoleMembership, RoleMembershipRepo, SortBy, SortOrder, Task, TaskLifecycleRepo,
    TaskLifecycleState as DbTaskLifecycleState, TaskRoleRepo, Workspace, WorkspaceRepo,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;

use crate::errors::{ApiError, ApiResult};

pub mod admin;
pub mod agent_profiles;
pub mod agents;
pub mod auth;
pub mod clis;
pub mod collaboration;
pub mod daemons;
pub mod events;
pub mod executions;
pub mod executor_types;
pub mod external_links;
pub mod fs;
pub mod integrations;
pub mod mcp_config;
pub mod members;
pub mod notifications;
pub mod oauth;
pub mod operations;
pub mod project_media;
pub mod project_releases;
pub mod projects;
pub mod provider_authorizations;
pub mod providers;
pub mod repos;
pub mod reviews;
pub mod settings;
pub mod tasks;
pub mod terminals;
pub mod work_units;
pub mod workspaces;

#[derive(Debug, Clone, Deserialize)]
pub struct ListParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub include_total: Option<bool>,
    pub q: Option<String>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub status: Option<String>, // Agent status filter; Task filters use lifecycle_state.
    pub lifecycle_state: Option<String>,
    pub include_archived: Option<bool>,
    pub include_cancelled: Option<bool>,
    pub task_type: Option<String>,
    pub priority: Option<i64>,
    pub capabilities: Option<String>,
    pub executor_type: Option<String>,
    pub daemon_id: Option<String>,
}

pub fn page_request(params: &ListParams) -> ApiResult<PageRequest> {
    Ok(PageRequest {
        cursor: params.cursor.clone(),
        limit: params.limit.unwrap_or(20).clamp(1, 100),
        include_total: params.include_total.unwrap_or(false),
        sort_by: parse_sort_by(params.sort_by.as_deref())?,
        sort_order: parse_sort_order(params.sort_order.as_deref())?,
    })
}

pub fn task_page_request(params: &ListParams) -> ApiResult<PageRequest> {
    if params.sort_by.is_none() {
        return Ok(PageRequest {
            cursor: params.cursor.clone(),
            limit: params.limit.unwrap_or(20).clamp(1, 100),
            include_total: params.include_total.unwrap_or(false),
            sort_by: SortBy::BoardPosition,
            sort_order: SortOrder::Asc,
        });
    }

    Ok(PageRequest {
        cursor: params.cursor.clone(),
        limit: params.limit.unwrap_or(20).clamp(1, 100),
        include_total: params.include_total.unwrap_or(false),
        sort_by: parse_task_sort_by(params.sort_by.as_deref())?,
        sort_order: parse_sort_order(params.sort_order.as_deref())?,
    })
}

pub fn paginated<T, U>(page: Page<T>, map: impl Fn(T) -> U) -> PaginatedResponse<U> {
    let has_more = page.next_cursor.is_some();
    PaginatedResponse {
        items: page.items.into_iter().map(map).collect(),
        next_cursor: page.next_cursor,
        has_more,
        total_count: page.total_count.and_then(|count| u64::try_from(count).ok()),
    }
}

pub fn project_response(project: Project) -> ApiResult<ProjectResponse> {
    let project_hooks = parse_project_hooks_json(&project.project_hooks_json).map_err(|error| {
        ApiError::internal(format!(
            "invalid persisted project hooks for project {}: {error}",
            project.id
        ))
    })?;
    Ok(ProjectResponse {
        id: project.id,
        name: project.name,
        project_hooks,
        primary_repo_id: project.primary_repo_id,
        owner_id: project.owner_id,
        created_at: project.created_at,
        updated_at: project.updated_at,
        paused_at: project.paused_at.clone(),
        paused: project.paused_at.is_some(),
        version: project.version,
    })
}

pub fn repo_response(repo: Repo) -> RepoResponse {
    RepoResponse {
        id: repo.id,
        project_id: repo.project_id,
        name: repo.name,
        local_path: repo.local_path,
        remote_url: repo.remote_url,
        default_branch: repo.default_branch,
        work_mode: repo_work_mode_response(repo.work_mode),
        pr_provider: None,
        pr_provider_status: None,
        created_at: repo.created_at,
        updated_at: repo.updated_at,
    }
}

fn repo_work_mode_response(work_mode: db::WorkMode) -> api_types::WorkMode {
    match work_mode {
        db::WorkMode::DirectMerge => api_types::WorkMode::DirectMerge,
        db::WorkMode::PullRequest => api_types::WorkMode::PullRequest,
    }
}

pub async fn task_response(db: &db::SqliteDb, task: Task) -> ApiResult<TaskResponse> {
    task_response_inner(db, task).await
}

pub async fn task_response_light(db: &db::SqliteDb, task: Task) -> ApiResult<TaskResponse> {
    task_response_inner(db, task).await
}

async fn task_response_inner(db: &db::SqliteDb, task: Task) -> ApiResult<TaskResponse> {
    let task_roles = task_roles_response(db, &task.id, false).await?;
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(db, &task.id)
        .await?
        .ok_or_else(|| ApiError::not_found("task lifecycle", task.id.clone()))?;
    let lifecycle = api_types::TaskLifecycleResponse {
        task_id: lifecycle.task_id,
        state: match lifecycle.state {
            DbTaskLifecycleState::Backlog => api_types::TaskLifecycleState::Backlog,
            DbTaskLifecycleState::Ready => api_types::TaskLifecycleState::Ready,
            DbTaskLifecycleState::Active => api_types::TaskLifecycleState::Active,
            DbTaskLifecycleState::Blocked => api_types::TaskLifecycleState::Blocked,
            DbTaskLifecycleState::ReadyToMerge => api_types::TaskLifecycleState::ReadyToMerge,
            DbTaskLifecycleState::Merging => api_types::TaskLifecycleState::Merging,
            DbTaskLifecycleState::Done => api_types::TaskLifecycleState::Done,
            DbTaskLifecycleState::Cancelled => api_types::TaskLifecycleState::Cancelled,
        },
        version: lifecycle.version,
        reason_kind: lifecycle.reason_kind,
        reason_ref: lifecycle.reason_ref,
        created_at: lifecycle.created_at,
        updated_at: lifecycle.updated_at,
    };
    let workspace = WorkspaceRepo::get_by_task_id(db, &task.id)
        .await?
        .map(workspace_response);
    let execution_observability = task_execution_observability(db, &task.id).await?;
    let external_link = db::ExternalLinkRepo::get_by_task_id(db, &task.id).await?;

    Ok(TaskResponse {
        id: task.id,
        project_id: task.project_id,
        repo_id: task.repo_id,
        parent_task_id: task.parent_task_id.clone(),
        title: task.title,
        description: task.description,
        task_type: parse_task_type(&task.task_type),
        lifecycle,
        priority: task.priority,
        board_position: task.board_position,
        subtask_order: task.subtask_order,
        task_roles,
        execution_observability,
        workspace,
        external_issue_number: external_link.as_ref().map(|link| link.remote_issue_number),
        external_issue_url: external_link.as_ref().map(|link| link.remote_url.clone()),
        version: task.version,
        created_at: task.created_at,
        updated_at: task.updated_at,
    })
}

async fn task_execution_observability(
    db: &db::SqliteDb,
    task_id: &str,
) -> std::result::Result<api_types::TaskExecutionObservability, db::DbError> {
    let row = sqlx::query(
        "WITH task_executions AS (
             SELECT * FROM execution WHERE task_id = ?
         ),
         usage_totals AS (
             SELECT
                 COALESCE(SUM(eu.input_tokens), 0) AS total_input_tokens,
                 COALESCE(SUM(eu.output_tokens), 0) AS total_output_tokens,
                 COALESCE(SUM(eu.cache_read_tokens), 0) AS total_cache_read_tokens,
                 COALESCE(SUM(eu.cache_write_tokens), 0) AS total_cache_write_tokens,
                 SUM(eu.cost_usd) AS total_cost_usd
             FROM execution_usage eu
             JOIN task_executions e ON e.id = eu.execution_id
         )
         SELECT
             (SELECT COUNT(*) FROM task_executions) AS execution_count,
             (SELECT COALESCE(SUM(max(COALESCE(
                 (CASE
                     WHEN status = 'running' THEN CAST(strftime('%s', 'now') AS INTEGER)
                     ELSE CAST(strftime('%s', COALESCE(stopped_at, updated_at)) AS INTEGER)
                  END) - CAST(strftime('%s', created_at) AS INTEGER),
                 0), 0)), 0)
              FROM task_executions) AS total_runtime_seconds,
             usage_totals.total_input_tokens,
             usage_totals.total_output_tokens,
             usage_totals.total_cache_read_tokens,
             usage_totals.total_cache_write_tokens,
             usage_totals.total_cost_usd
         FROM usage_totals",
    )
    .bind(task_id)
    .fetch_one(db.pool())
    .await?;

    let total_input_tokens = row.try_get::<i64, _>("total_input_tokens")?;
    let total_output_tokens = row.try_get::<i64, _>("total_output_tokens")?;
    let total_cache_read_tokens = row.try_get::<i64, _>("total_cache_read_tokens")?;
    let total_cache_write_tokens = row.try_get::<i64, _>("total_cache_write_tokens")?;
    Ok(api_types::TaskExecutionObservability {
        execution_count: row.try_get("execution_count")?,
        total_runtime_seconds: row.try_get::<i64, _>("total_runtime_seconds")? as f64,
        total_input_tokens,
        total_output_tokens,
        total_cache_read_tokens,
        total_cache_write_tokens,
        total_tokens: total_input_tokens
            + total_output_tokens
            + total_cache_read_tokens
            + total_cache_write_tokens,
        total_cost_usd: row.try_get("total_cost_usd")?,
    })
}

fn parse_task_type(task_type: &str) -> TaskType {
    match task_type {
        "planning" => TaskType::Planning,
        "discovery" => TaskType::Discovery,
        "review" => TaskType::Review,
        "validation" => TaskType::Validation,
        _ => TaskType::Implementation,
    }
}

pub async fn task_roles_response(
    db: &db::SqliteDb,
    task_id: &str,
    include_ended: bool,
) -> ApiResult<Vec<TaskRoleResponse>> {
    let roles = TaskRoleRepo::list_by_task(db, task_id).await?;
    let mut response = Vec::with_capacity(roles.len());
    for role in roles {
        let members = RoleMembershipRepo::list_by_role(db, &role.id, include_ended).await?;
        let policy = serde_json::from_str(&role.policy_json).map_err(|error| {
            ApiError::internal(format!("invalid persisted TaskRole policy: {error}"))
        })?;
        response.push(TaskRoleResponse {
            id: role.id,
            task_id: role.task_id,
            role: role.role,
            coordination_mode: role.coordination_mode.map(|mode| match mode {
                DbCoordinationMode::Partitioned => api_types::CoordinationMode::Partitioned,
                DbCoordinationMode::Collaborative => api_types::CoordinationMode::Collaborative,
                DbCoordinationMode::Independent => api_types::CoordinationMode::Independent,
            }),
            policy,
            version: role.version,
            members: members.into_iter().map(role_membership_response).collect(),
            created_at: role.created_at,
            updated_at: role.updated_at,
        });
    }
    Ok(response)
}

pub(crate) fn role_membership_response(member: RoleMembership) -> RoleMembershipResponse {
    let actor_ref = match member.actor_kind {
        ActorKind::Human => api_types::ActorRef::Human(member.actor_id),
        ActorKind::Agent => api_types::ActorRef::Agent(member.actor_id),
    };
    RoleMembershipResponse {
        id: member.id,
        task_role_id: member.task_role_id,
        actor_ref,
        status: match member.status {
            db::RoleMembershipStatus::Active => RoleMembershipStatus::Active,
            db::RoleMembershipStatus::Suspended => RoleMembershipStatus::Suspended,
            db::RoleMembershipStatus::Ended => RoleMembershipStatus::Ended,
        },
        version: member.version,
        created_at: member.created_at,
        updated_at: member.updated_at,
        ended_at: member.ended_at,
    }
}

pub fn agent_response(
    agent: Agent,
    active_execution_count: Option<i64>,
    effective_status: Option<String>,
    stats: db::AgentExecutionStats,
) -> AgentResponse {
    AgentResponse {
        id: agent.id,
        name: agent.name,
        description: agent.description,
        profile_id: agent.profile_id,
        executor_type: agent.executor_type,
        provider: agent.provider,
        model: agent.model,
        reasoning_effort: agent.reasoning_effort,
        permission_policy: agent.permission_policy,
        prompt_template: agent.prompt_template,
        capabilities: serde_json::from_str(&agent.capabilities_json).unwrap_or_default(),
        config_json: redact_sensitive_config(parse_json_value_or_empty_object(agent.config_json)),
        credential_handle_id: agent.credential_ref,
        daemon_id: agent.daemon_id,
        max_concurrent_tasks: agent.max_concurrent_tasks,
        status: agent_status_response(agent.status),
        active_execution_count,
        effective_status,
        total_runs: stats.total_runs,
        avg_duration_ms: stats.avg_duration_ms,
        success_rate: stats.success_rate,
        is_default: agent.is_default,
        paused: agent.paused,
        owner_id: agent.owner_id,
        visibility: agent.visibility,
        version: agent.version,
        created_at: agent.created_at,
        updated_at: agent.updated_at,
    }
}

pub fn daemon_response(daemon: Daemon) -> DaemonResponse {
    DaemonResponse {
        id: daemon.id,
        machine_id: daemon.machine_id,
        hostname: daemon.hostname,
        os: daemon.os,
        arch: daemon.arch,
        agent_version: daemon.agent_version,
        status: daemon.status.to_string(),
        last_report_at: daemon.last_report_at,
        detected_clis: serde_json::from_str(&daemon.detected_clis_json).unwrap_or(Value::Null),
        labels: serde_json::from_str(&daemon.labels_json).unwrap_or(Value::Null),
        owner_id: daemon.owner_id,
        visibility: daemon.visibility,
        version: daemon.version,
        created_at: daemon.created_at,
        updated_at: daemon.updated_at,
    }
}

pub fn workspace_response(workspace: Workspace) -> WorkspaceResponse {
    WorkspaceResponse {
        id: workspace.id,
        task_id: workspace.task_id,
        repo_id: workspace.repo_id,
        worktree_path: workspace.worktree_path,
        branch: workspace.branch,
        status: workspace.status.to_string(),
        before_sha: workspace.before_sha,
        error: workspace.error,
        created_at: workspace.created_at,
        updated_at: workspace.updated_at,
    }
}

pub fn execution_response(execution: Execution) -> ExecutionResponse {
    let actor_ref = execution.actor_ref().map(|actor| match actor {
        db::ActorRef::Human(id) => api_types::ActorRef::Human(id),
        db::ActorRef::Agent(id) => api_types::ActorRef::Agent(id),
    });
    ExecutionResponse {
        id: execution.id,
        task_id: execution.task_id,
        actor_ref,
        role: execution.role,
        purpose: execution.purpose.map(execution_purpose_response),
        status: execution_status_response(execution.status),
        parent_execution_id: execution.parent_execution_id,
        harness_session_id: execution.harness_session_id,
        prompt: execution.prompt,
        summary: execution.summary,
        logs_path: execution.logs_path,
        before_sha: execution.before_sha,
        after_sha: execution.after_sha,
        error: execution.error,
        stop_reason: execution.stop_reason.map(stop_reason_response),
        stopped_by: execution.stopped_by,
        resume_policy: execution.resume_policy.map(resume_policy_response),
        stopped_at: execution.stopped_at,
        executor_config_snapshot: execution
            .executor_config_snapshot_json
            .map(parse_json_value),
        workspace_id: execution.workspace_id,
        usage: None,
        account_usage: None,
        created_at: execution.created_at,
        updated_at: execution.updated_at,
    }
}

fn execution_purpose_response(purpose: db::ExecutionPurpose) -> api_types::ExecutionPurpose {
    match purpose {
        db::ExecutionPurpose::Plan => api_types::ExecutionPurpose::Plan,
        db::ExecutionPurpose::Implement => api_types::ExecutionPurpose::Implement,
        db::ExecutionPurpose::Review => api_types::ExecutionPurpose::Review,
        db::ExecutionPurpose::Validate => api_types::ExecutionPurpose::Validate,
        db::ExecutionPurpose::Investigate => api_types::ExecutionPurpose::Investigate,
        db::ExecutionPurpose::Orchestrate => api_types::ExecutionPurpose::Orchestrate,
        db::ExecutionPurpose::General => api_types::ExecutionPurpose::General,
    }
}

pub async fn execution_response_with_usage(
    db: &db::SqliteDb,
    execution: Execution,
) -> ApiResult<ExecutionResponse> {
    let execution_id = execution.id.clone();
    let mut response = execution_response(execution);
    response.account_usage = execution_account_usage(db, &execution_id).await?;
    Ok(response)
}

async fn execution_account_usage(
    db: &db::SqliteDb,
    execution_id: &str,
) -> std::result::Result<Option<Value>, db::DbError> {
    let row = sqlx::query(
        "SELECT usage_json FROM account_usage_snapshot
         WHERE execution_id = ?
         ORDER BY captured_at DESC
         LIMIT 1",
    )
    .bind(execution_id)
    .fetch_optional(db.pool())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let usage_json: String = row.try_get("usage_json")?;
    Ok(serde_json::from_str(&usage_json).ok())
}

pub fn execution_usage_response(usage: db::ExecutionUsage) -> api_types::ExecutionUsageResponse {
    api_types::ExecutionUsageResponse {
        id: usage.id,
        execution_id: usage.execution_id,
        provider: usage.provider,
        model: usage.model,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        cost_usd: usage.cost_usd,
        created_at: usage.created_at,
    }
}

pub fn task_usage_summary_response(
    summary: db::TaskUsageSummary,
) -> api_types::TaskUsageSummaryResponse {
    api_types::TaskUsageSummaryResponse {
        total_input_tokens: summary.total_input_tokens,
        total_output_tokens: summary.total_output_tokens,
        total_cache_read_tokens: summary.total_cache_read_tokens,
        total_cache_write_tokens: summary.total_cache_write_tokens,
        total_cost_usd: summary.total_cost_usd,
        execution_count: summary.execution_count,
    }
}

pub fn serialize_json<T>(value: Option<T>) -> ApiResult<Option<String>>
where
    T: Serialize,
{
    value
        .map(|value| serde_json::to_string(&value))
        .transpose()
        .map_err(|error| ApiError::bad_request(format!("invalid JSON value: {error}")))
}

pub fn parse_csv<T>(value: Option<&String>, field: &str) -> ApiResult<Vec<T>>
where
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value
        .split(',')
        .filter(|item| !item.trim().is_empty())
        .map(|item| {
            item.trim()
                .parse()
                .map_err(|_| ApiError::bad_request(format!("invalid {field}: {item}")))
        })
        .collect()
}

pub fn parse_optional<T>(value: Option<&String>, field: &str) -> ApiResult<Option<T>>
where
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    value
        .map(|value| {
            value
                .parse()
                .map_err(|_| ApiError::bad_request(format!("invalid {field}: {value}")))
        })
        .transpose()
}

fn parse_json_value(value: impl Into<String>) -> Value {
    let value = value.into();
    serde_json::from_str(&value).unwrap_or(Value::String(value))
}

fn parse_json_value_or_empty_object(value: String) -> Value {
    serde_json::from_str(&value).unwrap_or_else(|_| serde_json::json!({}))
}

pub(crate) fn redact_sensitive_config(value: Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let normalized = key.to_ascii_lowercase().replace('-', "_");
                    let sensitive = [
                        "api_key",
                        "token",
                        "secret",
                        "password",
                        "authorization",
                        "credential",
                        "private_key",
                    ]
                    .iter()
                    .any(|candidate| normalized.contains(candidate));
                    (
                        key,
                        if sensitive {
                            Value::String("[redacted]".to_owned())
                        } else {
                            redact_sensitive_config(value)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(redact_sensitive_config).collect())
        }
        value => value,
    }
}

fn parse_sort_by(value: Option<&str>) -> ApiResult<SortBy> {
    match value.unwrap_or("created_at") {
        "created_at" => Ok(SortBy::CreatedAt),
        "updated_at" => Ok(SortBy::UpdatedAt),
        "priority" => Ok(SortBy::Priority),
        "board_position" => Ok(SortBy::BoardPosition),
        "id" => Ok(SortBy::Id),
        value => Err(ApiError::bad_request(format!("invalid sort_by: {value}"))),
    }
}

fn parse_task_sort_by(value: Option<&str>) -> ApiResult<SortBy> {
    match value.unwrap_or("board_position") {
        "created_at" => Ok(SortBy::CreatedAt),
        "updated_at" => Ok(SortBy::UpdatedAt),
        "priority" => Ok(SortBy::Priority),
        "board_position" => Ok(SortBy::BoardPosition),
        "title" => Ok(SortBy::Title),
        "lifecycle_state" => Ok(SortBy::LifecycleState),
        "task_type" => Ok(SortBy::TaskType),
        "id" => Ok(SortBy::Id),
        value => Err(ApiError::bad_request(format!("invalid sort_by: {value}"))),
    }
}

fn parse_sort_order(value: Option<&str>) -> ApiResult<SortOrder> {
    match value.unwrap_or("desc") {
        "asc" => Ok(SortOrder::Asc),
        "desc" => Ok(SortOrder::Desc),
        value => Err(ApiError::bad_request(format!(
            "invalid sort_order: {value}"
        ))),
    }
}

fn stop_reason_response(value: db::StopReason) -> api_types::StopReason {
    match value {
        db::StopReason::UserCancelled => api_types::StopReason::UserCancelled,
        db::StopReason::TaskCancelled => api_types::StopReason::TaskCancelled,
        db::StopReason::RoleReassigned => api_types::StopReason::RoleReassigned,
        db::StopReason::GracefulShutdown => api_types::StopReason::GracefulShutdown,
        db::StopReason::CrashRecovery => api_types::StopReason::CrashRecovery,
        db::StopReason::AgentTimeout => api_types::StopReason::AgentTimeout,
        db::StopReason::ExecutionStalled => api_types::StopReason::ExecutionStalled,
        db::StopReason::DaemonDisconnected => api_types::StopReason::DaemonDisconnected,
        db::StopReason::ExecutorCancelled => api_types::StopReason::ExecutorCancelled,
        db::StopReason::ExecutorFailed => api_types::StopReason::ExecutorFailed,
        db::StopReason::LegacyUnknown => api_types::StopReason::LegacyUnknown,
    }
}

fn resume_policy_response(value: db::ResumePolicy) -> api_types::ResumePolicy {
    match value {
        db::ResumePolicy::Auto => api_types::ResumePolicy::Auto,
        db::ResumePolicy::Manual => api_types::ResumePolicy::Manual,
        db::ResumePolicy::None => api_types::ResumePolicy::None,
    }
}

fn agent_status_response(value: db::AgentStatus) -> api_types::AgentStatus {
    match value {
        db::AgentStatus::Idle => api_types::AgentStatus::Idle,
        db::AgentStatus::Busy => api_types::AgentStatus::Busy,
        db::AgentStatus::Error => api_types::AgentStatus::Error,
        db::AgentStatus::Offline => api_types::AgentStatus::Offline,
    }
}

fn execution_status_response(value: db::ExecutionStatus) -> api_types::ExecutionStatus {
    match value {
        db::ExecutionStatus::Running => api_types::ExecutionStatus::Running,
        db::ExecutionStatus::Completed => api_types::ExecutionStatus::Completed,
        db::ExecutionStatus::Failed => api_types::ExecutionStatus::Failed,
        db::ExecutionStatus::Cancelled => api_types::ExecutionStatus::Cancelled,
    }
}
