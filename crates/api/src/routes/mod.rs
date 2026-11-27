use std::str::FromStr;

use api_types::{
    parse_project_hooks_json, AgentResponse, DaemonResponse, ExecutionResponse, PaginatedResponse,
    ProjectResponse, RepoResponse, RoleMembershipResponse, RoleMembershipStatus, TaskResponse,
    TaskRoleResponse, WorkspaceResponse,
};
use db::{
    ActorKind, Agent, CoordinationMode as DbCoordinationMode, Daemon, Execution, ExecutionRepo,
    Page, PageRequest, PrMetadataRepo, Project, Repo, RoleMembership, RoleMembershipRepo, SortBy,
    SortOrder, Task, TaskLifecycleRepo, TaskLifecycleState as DbTaskLifecycleState,
    TaskRepositoryMergeBatchRepo, TaskRepositorySetRepo, TaskRoleRepo, WorkUnitRepo,
    WorkUnitRepositoryIntegrationRepo, Workspace, WorkspaceRepo,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use std::collections::BTreeMap;

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
pub mod human_decisions;
pub mod integrations;
pub mod mcp_config;
pub(crate) mod mcp_config_fs;
pub mod members;
pub mod notifications;
pub mod oauth;
pub mod operations;
pub mod phase_three;
pub mod pr_feedback;
pub mod project_copilot;
pub mod project_media;
pub mod project_plan;
pub mod project_releases;
pub mod projects;
pub mod provider_authorizations;
pub mod providers;
pub mod repos;
pub mod reviews;
pub mod settings;
pub mod task_coordination;
pub mod task_dispatch;
pub mod task_rework;
pub mod tasks;
pub mod terminals;
pub mod validation_checks;
pub mod work_unit_reviews;
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
        default_review_policy: None,
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
    let repository_set = TaskRepositorySetRepo::get_current(db, &task.id)
        .await?
        .ok_or_else(|| ApiError::not_found("Task repository-set revision", task.id.clone()))?;
    let batches = TaskRepositoryMergeBatchRepo::list_by_task(db, &task.id).await?;
    let mut batch_repositories = BTreeMap::new();
    let mut repository_merge_batches = Vec::with_capacity(batches.len());
    for batch in batches {
        let rows = TaskRepositoryMergeBatchRepo::list_repositories(db, &batch.id).await?;
        let repositories = rows
            .into_iter()
            .map(|row| {
                let response = api_types::TaskRepositoryMergeResponse {
                    batch_id: row.batch_id.clone(),
                    repo_id: row.repo_id.clone(),
                    state: merge_state_response(row.state),
                    integration_id: row.work_unit_repository_integration_id.clone(),
                    task_merge_operation_id: row.task_merge_operation_id.clone(),
                    pr_metadata_id: row.pr_metadata_id.clone(),
                    before_sha: row.before_sha.clone(),
                    after_sha: row.after_sha.clone(),
                };
                batch_repositories
                    .entry(row.repo_id.clone())
                    .or_insert_with(Vec::new)
                    .push(response.clone());
                response
            })
            .collect();
        repository_merge_batches.push(api_types::TaskRepositoryMergeBatchResponse {
            id: batch.id,
            task_id: batch.task_id,
            gate_evaluation_id: batch.gate_evaluation_id,
            repository_set_revision: batch.repository_set_revision,
            state: match batch.state {
                db::TaskRepositoryMergeBatchState::Integrating => {
                    api_types::TaskRepositoryMergeBatchState::Integrating
                }
                db::TaskRepositoryMergeBatchState::Publishing => {
                    api_types::TaskRepositoryMergeBatchState::Publishing
                }
                db::TaskRepositoryMergeBatchState::Open => {
                    api_types::TaskRepositoryMergeBatchState::Open
                }
                db::TaskRepositoryMergeBatchState::Completed => {
                    api_types::TaskRepositoryMergeBatchState::Completed
                }
                db::TaskRepositoryMergeBatchState::Blocked => {
                    api_types::TaskRepositoryMergeBatchState::Blocked
                }
                db::TaskRepositoryMergeBatchState::Failed => {
                    api_types::TaskRepositoryMergeBatchState::Failed
                }
            },
            version: batch.version,
            repositories,
            created_at: batch.created_at,
            updated_at: batch.updated_at,
        });
    }
    let prs = PrMetadataRepo::list_by_task(db, &task.id).await?;
    let mut pull_requests_by_id = BTreeMap::new();
    for pr in prs {
        if let Some(repo_id) = pr.repo_id.clone() {
            pull_requests_by_id.insert(
                pr.id.clone(),
                api_types::TaskRepositoryPullRequestResponse {
                    repo_id,
                    admission_id: pr.task_merge_operation_id.clone(),
                    pr_metadata_id: pr.id,
                    provider_type: pr.provider_type,
                    provider_pr_id: pr.provider_pr_id,
                    pr_url: pr.pr_url,
                    pr_state: pr.pr_state,
                    merge_status: pr.merge_status,
                    admission_status: pr.admission_status,
                },
            );
        }
    }
    let work_units = WorkUnitRepo::list_by_task(db, &task.id).await?;
    let mut integrations =
        BTreeMap::<String, (String, api_types::TaskRepositoryIntegrationResponse)>::new();
    for unit in work_units {
        for integration in
            WorkUnitRepositoryIntegrationRepo::list_by_work_unit(db, &unit.id).await?
        {
            if integration.repository_set_revision == repository_set.revision {
                let replace = integrations
                    .get(&integration.repo_id)
                    .is_none_or(|(updated_at, _)| updated_at <= &integration.updated_at);
                if replace {
                    let outcome = match integration.outcome {
                        db::WorkUnitRepositoryIntegrationOutcome::Running => "pending",
                        db::WorkUnitRepositoryIntegrationOutcome::Success => "success",
                        db::WorkUnitRepositoryIntegrationOutcome::Unchanged => "unchanged",
                        db::WorkUnitRepositoryIntegrationOutcome::Conflict => "conflict",
                        db::WorkUnitRepositoryIntegrationOutcome::Failed
                        | db::WorkUnitRepositoryIntegrationOutcome::Rejected => "failed",
                    };
                    integrations.insert(
                        integration.repo_id.clone(),
                        (
                            integration.updated_at.clone(),
                            api_types::TaskRepositoryIntegrationResponse {
                                repo_id: integration.repo_id,
                                integration_id: Some(integration.id),
                                outcome: outcome.to_owned(),
                                before_sha: Some(integration.target_before_sha),
                                after_sha: integration.target_after_sha,
                            },
                        ),
                    );
                }
            }
        }
    }
    let primary_repo_id = repository_set
        .repositories
        .iter()
        .find(|repository| repository.role == db::TaskRepositoryRole::Primary)
        .map(|repository| repository.repo_id.clone());
    let repositories = repository_set
        .repositories
        .into_iter()
        .map(|repository| {
            let merge_rows = batch_repositories
                .get(&repository.repo_id)
                .into_iter()
                .flatten()
                .filter(|row| {
                    repository_merge_batches.iter().any(|batch| {
                        batch.id == row.batch_id
                            && batch.repository_set_revision == repository_set.revision
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            let merge = merge_rows.last().cloned();
            let integration = integrations
                .get(&repository.repo_id)
                .map(|(_, integration)| integration.clone());
            let change_state = if merge.is_some()
                || integration
                    .as_ref()
                    .is_some_and(|integration| integration.outcome == "success")
            {
                api_types::TaskRepositoryChangeState::Changed
            } else if repository.role == db::TaskRepositoryRole::Context
                || integration
                    .as_ref()
                    .is_some_and(|integration| integration.outcome == "unchanged")
            {
                api_types::TaskRepositoryChangeState::Unchanged
            } else {
                api_types::TaskRepositoryChangeState::Pending
            };
            let pull_request = merge
                .as_ref()
                .and_then(|row| row.pr_metadata_id.as_ref())
                .and_then(|metadata_id| pull_requests_by_id.get(metadata_id))
                .cloned();
            api_types::TaskRepositoryResponse {
                repo_id: repository.repo_id.clone(),
                role: match repository.role {
                    db::TaskRepositoryRole::Primary => api_types::TaskRepositoryRole::Primary,
                    db::TaskRepositoryRole::Affected => api_types::TaskRepositoryRole::Affected,
                    db::TaskRepositoryRole::Context => api_types::TaskRepositoryRole::Context,
                },
                position: repository.position,
                change_state,
                integration,
                merge,
                pull_request,
            }
        })
        .collect();
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
    let workspace = if let Some(primary_repo_id) = primary_repo_id {
        WorkspaceRepo::get_integration_by_task_repo(db, &task.id, &primary_repo_id)
            .await?
            .map(workspace_response)
    } else {
        None
    };
    let execution_observability = task_execution_observability(db, &task.id).await?;
    let external_link = db::ExternalLinkRepo::get_by_task_id(db, &task.id).await?;

    Ok(TaskResponse {
        id: task.id,
        project_id: task.project_id,
        repo_id: task.repo_id,
        repositories,
        repository_set_revision: repository_set.revision,
        repository_set_digest: repository_set.digest,
        repository_merge_batches,
        parent_task_id: task.parent_task_id.clone(),
        title: task.title,
        description: task.description,
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

fn merge_state_response(
    state: db::TaskRepositoryMergeBatchRepoState,
) -> api_types::TaskRepositoryMergeState {
    match state {
        db::TaskRepositoryMergeBatchRepoState::Pending => {
            api_types::TaskRepositoryMergeState::Pending
        }
        db::TaskRepositoryMergeBatchRepoState::Integrating => {
            api_types::TaskRepositoryMergeState::Integrating
        }
        db::TaskRepositoryMergeBatchRepoState::Integrated => {
            api_types::TaskRepositoryMergeState::Integrated
        }
        db::TaskRepositoryMergeBatchRepoState::Publishing => {
            api_types::TaskRepositoryMergeState::Publishing
        }
        db::TaskRepositoryMergeBatchRepoState::Open => api_types::TaskRepositoryMergeState::Open,
        db::TaskRepositoryMergeBatchRepoState::Merged => {
            api_types::TaskRepositoryMergeState::Merged
        }
        db::TaskRepositoryMergeBatchRepoState::Conflict => {
            api_types::TaskRepositoryMergeState::Conflict
        }
        db::TaskRepositoryMergeBatchRepoState::Closed => {
            api_types::TaskRepositoryMergeState::Closed
        }
        db::TaskRepositoryMergeBatchRepoState::Failed => {
            api_types::TaskRepositoryMergeState::Failed
        }
    }
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

pub async fn task_roles_response(
    db: &db::SqliteDb,
    task_id: &str,
    include_ended: bool,
) -> ApiResult<Vec<TaskRoleResponse>> {
    let roles = TaskRoleRepo::list_by_task(db, task_id).await?;
    let mut response = Vec::with_capacity(roles.len());
    for role in roles {
        let members = RoleMembershipRepo::list_by_role(db, &role.id, include_ended).await?;
        let policy = serde_json::from_str::<Value>(&role.policy_json).map_err(|error| {
            ApiError::internal(format!("invalid persisted TaskRole policy: {error}"))
        })?;
        let configuration_record =
            db::TaskRoleCapabilityConfigurationRepo::get_by_task_role_id(db, &role.id)
                .await?
                .ok_or_else(|| {
                    ApiError::internal("TaskRole capability configuration is missing")
                })?;
        let configuration = services::task_role_configuration::parse_task_role_configuration(
            &configuration_record.configuration_json,
        )
        .map_err(ApiError::from)?;
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
            configuration,
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
        work_unit_id: execution.work_unit_id,
        work_unit_version: execution.work_unit_version,
        repository_results: Vec::new(),
        usage: None,
        account_usage: None,
        created_at: execution.created_at,
        updated_at: execution.updated_at,
    }
}

pub async fn execution_response_with_repository_results(
    db: &db::SqliteDb,
    execution: Execution,
) -> ApiResult<ExecutionResponse> {
    let results = ExecutionRepo::list_repository_results(db, &execution.id).await?;
    let mut response = execution_response(execution);
    response.repository_results = results
        .into_iter()
        .map(|result| api_types::ExecutionRepositoryResultResponse {
            repo_id: result.repo_id,
            workspace_id: result.workspace_id,
            before_sha: result.before_sha,
            after_sha: result.after_sha,
            snapshot_digest: result.snapshot_digest,
        })
        .collect();
    Ok(response)
}

pub async fn execution_response_with_usage(
    db: &db::SqliteDb,
    execution: Execution,
) -> ApiResult<ExecutionResponse> {
    let execution_id = execution.id.clone();
    let mut response = execution_response_with_repository_results(db, execution).await?;
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
