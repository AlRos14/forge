use std::{path::Path, sync::Arc};

use api_types::{
    ActiveExecutionSummary, AgentPressureSummary, BlockedTaskSummary, DaemonIssueSummary,
    DaemonPressureSummary, EffectiveExecutionPolicy, OperatorSeverity, OperatorStatusResponse,
    RecentErrorSummary, RetryPressureSummary, TaskLifecycleState, TokenTotalsSummary, UsageSummary,
    WorkspaceCleanupSummary,
};
use chrono::{DateTime, Duration, Utc};
use db::SqliteDb;
use executors::{LogKind, LogReader};
use serde_json::Value;
use sqlx::{sqlite::SqliteRow, Row};

use crate::{agent_capacity::daemon_session_cap_from_labels, ServiceError};

pub struct OperatorStatusService {
    db: Arc<SqliteDb>,
}

impl OperatorStatusService {
    pub fn new(db: Arc<SqliteDb>) -> Self {
        Self { db }
    }

    pub async fn compute_status(&self) -> Result<OperatorStatusResponse, ServiceError> {
        let now = Utc::now();
        let computed_at = now.to_rfc3339();

        let active_executions = self.active_executions(now).await?;
        let blocked_tasks = self.blocked_tasks().await?;
        let daemon_issues = self.daemon_issues(now).await?;
        let daemon_error_count = self.daemon_error_count().await?;
        let daemon_pressure = self.daemon_pressure().await?;
        let agent_pressure = self.agent_pressure().await?;
        let workspace_cleanup = self.workspace_cleanup_backlog(now).await?;
        let retry_pressure = self.retry_pressure().await?;
        let usage_summary = Some(self.usage_summary(active_executions.len() as u32).await?);
        let recent_errors = self.recent_errors(now).await?;

        let mut overall_severity = OperatorSeverity::Healthy;
        if !daemon_issues.is_empty() || !workspace_cleanup.is_empty() {
            raise_severity(&mut overall_severity, OperatorSeverity::Attention);
        }
        if daemon_pressure.iter().any(|item| item.at_capacity)
            || agent_pressure.iter().any(|item| item.at_capacity)
        {
            raise_severity(&mut overall_severity, OperatorSeverity::Attention);
        }
        if !blocked_tasks.is_empty() {
            raise_severity(&mut overall_severity, OperatorSeverity::Blocked);
        }
        if !recent_errors.is_empty() || daemon_error_count > 0 {
            raise_severity(&mut overall_severity, OperatorSeverity::Error);
        }

        Ok(OperatorStatusResponse {
            overall_severity,
            active_executions,
            blocked_tasks,
            daemon_issues,
            daemon_pressure,
            agent_pressure,
            workspace_cleanup,
            retry_pressure,
            usage_summary,
            recent_errors,
            computed_at,
        })
    }

    async fn active_executions(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<ActiveExecutionSummary>, ServiceError> {
        let rows = sqlx::query(
            "SELECT
                e.id AS execution_id,
                e.task_id,
                t.title AS task_title,
                e.role,
                e.agent_id,
                a.name AS agent_name,
                a.daemon_id,
                e.workspace_id,
                w.worktree_path,
                e.harness_session_id,
                e.created_at AS started_at,
                e.logs_path,
                e.last_activity_at,
                eu.input_tokens,
                eu.output_tokens,
                eu.cache_read_tokens,
                eu.cache_write_tokens,
                eu.cost_usd,
                e.executor_config_snapshot_json
             FROM execution e
             JOIN task t ON t.id = e.task_id
             LEFT JOIN agent_current a ON a.id = e.agent_id
             LEFT JOIN workspace w ON w.id = e.workspace_id
             LEFT JOIN execution_usage eu ON eu.execution_id = e.id
             WHERE e.status = 'running'
             ORDER BY e.created_at ASC, e.id ASC",
        )
        .fetch_all(self.db.pool())
        .await?;

        let mut active_executions = Vec::with_capacity(rows.len());
        for row in rows {
            let started_at: String = row.try_get("started_at")?;
            let workspace_path: Option<String> = row.try_get("worktree_path")?;
            let runtime_seconds = seconds_since(&started_at, now);
            let snapshot_json: Option<String> = row.try_get("executor_config_snapshot_json")?;
            let effective_policy =
                effective_policy(snapshot_json.as_deref(), workspace_path.as_deref());
            let rate_limit_snapshot = rate_limit_snapshot(snapshot_json.as_deref());
            let log_snapshot =
                execution_log_snapshot(row.try_get::<Option<String>, _>("logs_path")?).await;
            let token_totals = token_totals_from_row(&row)?;

            active_executions.push(ActiveExecutionSummary {
                execution_id: row.try_get("execution_id")?,
                task_id: row.try_get("task_id")?,
                task_title: row.try_get("task_title")?,
                role: row.try_get("role")?,
                agent_id: row.try_get("agent_id")?,
                agent_name: row.try_get("agent_name")?,
                daemon_id: row.try_get("daemon_id")?,
                workspace_id: row.try_get("workspace_id")?,
                workspace_path,
                harness_session_id: row.try_get("harness_session_id")?,
                started_at,
                runtime_seconds,
                elapsed_seconds: runtime_seconds,
                latest_event: log_snapshot.last_event.clone(),
                last_event: log_snapshot.last_event,
                last_event_time: log_snapshot.last_event_time,
                turn_count: log_snapshot.turn_count,
                token_totals,
                rate_limit_snapshot,
                effective_policy,
            });
        }

        Ok(active_executions)
    }

    async fn blocked_tasks(&self) -> Result<Vec<BlockedTaskSummary>, ServiceError> {
        let rows = sqlx::query(
            "SELECT t.id, t.title, lifecycle.version, lifecycle.reason_kind,
                    lifecycle.reason_ref, lifecycle.updated_at,
                    transition.id AS transition_id
             FROM task t
             JOIN task_lifecycle lifecycle ON lifecycle.task_id = t.id
             LEFT JOIN task_lifecycle_transition transition
               ON transition.task_id = lifecycle.task_id
              AND transition.to_version = lifecycle.version
              AND transition.to_state = 'blocked'
             WHERE lifecycle.state = 'blocked' AND t.deleted_at IS NULL
             ORDER BY lifecycle.updated_at DESC, t.id ASC",
        )
        .fetch_all(self.db.pool())
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(BlockedTaskSummary {
                    task_id: row.try_get("id")?,
                    title: row.try_get("title")?,
                    lifecycle_state: TaskLifecycleState::Blocked,
                    lifecycle_version: row.try_get("version")?,
                    transition_id: row.try_get("transition_id")?,
                    reason_kind: row.try_get("reason_kind")?,
                    reason_ref: row.try_get("reason_ref")?,
                    blocked_since: row.try_get("updated_at")?,
                })
            })
            .collect()
    }

    async fn daemon_issues(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<DaemonIssueSummary>, ServiceError> {
        let stale_before = now - Duration::minutes(5);
        let rows = sqlx::query(
            "SELECT id, hostname, status, last_report_at, updated_at
             FROM daemon
             WHERE status = 'offline'
                OR status = 'error'
                OR (last_report_at IS NOT NULL AND last_report_at < ?)
             ORDER BY updated_at DESC, id ASC",
        )
        .bind(stale_before.to_rfc3339())
        .fetch_all(self.db.pool())
        .await?;

        rows.into_iter()
            .map(|row| {
                let status: String = row.try_get("status")?;
                let last_report_at: Option<String> = row.try_get("last_report_at")?;
                let (issue, severity) = match status.as_str() {
                    "error" => ("error".to_owned(), OperatorSeverity::Error),
                    "offline" => ("offline".to_owned(), OperatorSeverity::Attention),
                    _ => ("stale".to_owned(), OperatorSeverity::Attention),
                };
                Ok(DaemonIssueSummary {
                    daemon_id: row.try_get("id")?,
                    hostname: row.try_get("hostname")?,
                    issue,
                    severity,
                    detected_at: last_report_at.or_else(|| row.try_get("updated_at").ok()),
                })
            })
            .collect()
    }

    async fn daemon_error_count(&self) -> Result<i64, ServiceError> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM daemon WHERE status = 'error'")
                .fetch_one(self.db.pool())
                .await?,
        )
    }

    async fn daemon_pressure(&self) -> Result<Vec<DaemonPressureSummary>, ServiceError> {
        let rows = sqlx::query(
            "SELECT
                d.id AS daemon_id,
                d.hostname,
                d.labels_json,
                (
                    SELECT COUNT(*)
                    FROM execution e
                    JOIN agent_current a ON a.id = e.agent_id
                    WHERE a.daemon_id = d.id AND e.status = 'running'
                ) AS running_executions
             FROM daemon d
             ORDER BY d.updated_at DESC, d.id ASC",
        )
        .fetch_all(self.db.pool())
        .await?;

        let mut pressure = Vec::new();
        for row in rows {
            let running_executions: i64 = row.try_get("running_executions")?;
            let active_execution_count = running_executions.max(0) as u32;
            let labels_json: String = row.try_get("labels_json")?;
            let max_sessions = daemon_session_cap_from_labels(&labels_json)
                .and_then(|value| u32::try_from(value).ok());
            let at_capacity = max_sessions.is_some_and(|max| active_execution_count >= max);
            if active_execution_count == 0 && max_sessions.is_none() {
                continue;
            }
            pressure.push(DaemonPressureSummary {
                daemon_id: row.try_get("daemon_id")?,
                hostname: row.try_get("hostname")?,
                active_execution_count,
                max_sessions,
                at_capacity,
            });
        }
        Ok(pressure)
    }

    async fn agent_pressure(&self) -> Result<Vec<AgentPressureSummary>, ServiceError> {
        let rows = sqlx::query(
            "SELECT
                a.id AS agent_id,
                a.name AS agent_name,
                a.daemon_id,
                a.max_concurrent_tasks,
                (
                    SELECT COUNT(*)
                    FROM execution e
                    WHERE e.agent_id = a.id AND e.status = 'running'
                ) AS running_executions
             FROM agent_current a
             ORDER BY a.name ASC, a.id ASC",
        )
        .fetch_all(self.db.pool())
        .await?;

        let mut pressure = Vec::new();
        for row in rows {
            let running_executions: i64 = row.try_get("running_executions")?;
            let active_execution_count = running_executions.max(0) as u32;
            let max_concurrent_tasks = row.try_get::<i64, _>("max_concurrent_tasks")?.max(0) as u32;
            let at_capacity =
                max_concurrent_tasks > 0 && active_execution_count >= max_concurrent_tasks;
            if active_execution_count == 0 && !at_capacity {
                continue;
            }
            pressure.push(AgentPressureSummary {
                agent_id: row.try_get("agent_id")?,
                agent_name: row.try_get("agent_name")?,
                daemon_id: row.try_get("daemon_id")?,
                active_execution_count,
                max_concurrent_tasks,
                at_capacity,
            });
        }
        pressure.sort_by(|left, right| {
            right
                .active_execution_count
                .cmp(&left.active_execution_count)
                .then_with(|| left.agent_name.cmp(&right.agent_name))
        });
        Ok(pressure)
    }

    async fn workspace_cleanup_backlog(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<WorkspaceCleanupSummary>, ServiceError> {
        let rows = sqlx::query(
            "SELECT id, task_id, worktree_path, cleanup_after
             FROM workspace
             WHERE status IN ('ready', 'cleaning')
               AND cleanup_after IS NOT NULL
               AND cleanup_after < ?
             ORDER BY cleanup_after ASC, id ASC",
        )
        .bind(now.to_rfc3339())
        .fetch_all(self.db.pool())
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(WorkspaceCleanupSummary {
                    workspace_id: row.try_get("id")?,
                    task_id: row.try_get("task_id")?,
                    worktree_path: row.try_get("worktree_path")?,
                    cleanup_after: row.try_get("cleanup_after")?,
                })
            })
            .collect()
    }

    async fn retry_pressure(&self) -> Result<Vec<RetryPressureSummary>, ServiceError> {
        let rows = sqlx::query(
            "SELECT
                receipt.id AS retry_receipt_id,
                t.id AS task_id,
                t.title,
                receipt.failure_kind,
                receipt.failure_ref,
                receipt.source_event_id,
                receipt.receipt_event_id,
                receipt.attempt_number,
                receipt.retry_budget,
                receipt.disposition,
                lifecycle.state AS lifecycle_state,
                lifecycle.version AS lifecycle_version,
                receipt.created_at
             FROM task t
             JOIN task_lifecycle lifecycle ON lifecycle.task_id = t.id
             JOIN task_failure_retry_receipt receipt ON receipt.task_id = t.id
             WHERE t.deleted_at IS NULL
               AND lifecycle.state NOT IN ('done', 'cancelled')
             ORDER BY lifecycle.state, t.id, receipt.failure_kind,
                      receipt.retry_epoch, receipt.attempt_number, receipt.id",
        )
        .fetch_all(self.db.pool())
        .await?;

        rows.into_iter()
            .map(|row| {
                let attempt_number = u32::try_from(row.try_get::<i64, _>("attempt_number")?)
                    .map_err(|_| ServiceError::invalid_operation("invalid retry attempt number"))?;
                let retry_budget = u32::try_from(row.try_get::<i64, _>("retry_budget")?)
                    .map_err(|_| ServiceError::invalid_operation("invalid retry budget"))?;
                let disposition: String = row.try_get("disposition")?;
                if !matches!(disposition.as_str(), "rework" | "exhausted") {
                    return Err(ServiceError::invalid_operation(
                        "invalid retry receipt disposition",
                    ));
                }
                Ok(RetryPressureSummary {
                    retry_receipt_id: row.try_get("retry_receipt_id")?,
                    task_id: row.try_get("task_id")?,
                    title: row.try_get("title")?,
                    failure_kind: row.try_get("failure_kind")?,
                    failure_ref: row.try_get("failure_ref")?,
                    source_event_id: row.try_get("source_event_id")?,
                    receipt_event_id: row.try_get("receipt_event_id")?,
                    attempt_number,
                    retry_budget,
                    disposition,
                    lifecycle_state: api_lifecycle_state(
                        &row.try_get::<String, _>("lifecycle_state")?,
                    )?,
                    lifecycle_version: row.try_get("lifecycle_version")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }

    async fn usage_summary(
        &self,
        active_execution_count: u32,
    ) -> Result<UsageSummary, ServiceError> {
        let result = sqlx::query(
            "SELECT
                COALESCE(SUM(input_tokens), 0) AS total_input_tokens,
                COALESCE(SUM(output_tokens), 0) AS total_output_tokens,
                SUM(cost_usd) AS total_cost_usd
             FROM execution_usage",
        )
        .fetch_one(self.db.pool())
        .await;

        match result {
            Ok(row) => Ok(UsageSummary {
                available: true,
                total_input_tokens: Some(row.try_get("total_input_tokens")?),
                total_output_tokens: Some(row.try_get("total_output_tokens")?),
                total_cost_usd: row.try_get("total_cost_usd")?,
                active_execution_count,
            }),
            Err(error) if is_missing_table(&error) => Ok(UsageSummary {
                available: false,
                total_input_tokens: Some(0),
                total_output_tokens: Some(0),
                total_cost_usd: Some(0.0),
                active_execution_count,
            }),
            Err(error) => Err(error.into()),
        }
    }

    async fn recent_errors(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<RecentErrorSummary>, ServiceError> {
        let cutoff = now - Duration::hours(1);
        let rows = sqlx::query(
            "SELECT
                e.id AS execution_id,
                e.task_id,
                e.error,
                COALESCE(e.stopped_at, e.updated_at) AS occurred_at
             FROM execution e
             JOIN task t ON t.id = e.task_id
             WHERE e.status = 'failed'
               AND t.deleted_at IS NULL
               AND t.status NOT IN ('done', 'cancelled')
               AND COALESCE(e.stopped_at, e.updated_at) >= ?
             ORDER BY occurred_at DESC, e.id ASC",
        )
        .bind(cutoff.to_rfc3339())
        .fetch_all(self.db.pool())
        .await?;

        rows.into_iter()
            .map(|row| {
                let error: Option<String> = row.try_get("error")?;
                Ok(RecentErrorSummary {
                    entity_type: "task".to_owned(),
                    entity_id: row.try_get("task_id")?,
                    error: error.unwrap_or_else(|| "execution failed".to_owned()),
                    occurred_at: row.try_get("occurred_at")?,
                    severity: OperatorSeverity::Error,
                })
            })
            .collect()
    }
}

fn raise_severity(current: &mut OperatorSeverity, candidate: OperatorSeverity) {
    if candidate > *current {
        *current = candidate;
    }
}

fn seconds_since(started_at: &str, now: DateTime<Utc>) -> f64 {
    parse_rfc3339(started_at)
        .map(|started_at| (now - started_at).num_milliseconds().max(0) as f64 / 1000.0)
        .unwrap_or(0.0)
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

#[derive(Default)]
struct ExecutionLogSnapshot {
    turn_count: u32,
    last_event: Option<String>,
    last_event_time: Option<String>,
}

async fn execution_log_snapshot(logs_path: Option<String>) -> ExecutionLogSnapshot {
    let Some(logs_path) = logs_path else {
        return ExecutionLogSnapshot::default();
    };
    let path = Path::new(&logs_path);
    let mut from_sequence = 0;
    let mut snapshot = ExecutionLogSnapshot::default();

    loop {
        let result = match LogReader::read(path, from_sequence, 500).await {
            Ok(result) => result,
            Err(error) => {
                tracing::debug!(logs_path = %logs_path, %error, "failed to read execution log snapshot");
                return snapshot;
            }
        };
        for entry in &result.entries {
            if entry.kind == LogKind::Assistant {
                snapshot.turn_count = snapshot.turn_count.saturating_add(1);
            }
            snapshot.last_event = Some(entry.kind.to_string());
            snapshot.last_event_time = Some(entry.timestamp.clone());
        }
        if !result.has_more {
            break;
        }
        let Some(next_sequence) = result.next_sequence else {
            break;
        };
        from_sequence = next_sequence;
    }

    snapshot
}

fn token_totals_from_row(row: &SqliteRow) -> Result<Option<TokenTotalsSummary>, sqlx::Error> {
    let Some(input_tokens) = row.try_get::<Option<i64>, _>("input_tokens")? else {
        return Ok(None);
    };
    Ok(Some(TokenTotalsSummary {
        input_tokens,
        output_tokens: row
            .try_get::<Option<i64>, _>("output_tokens")?
            .unwrap_or_default(),
        cache_read_tokens: row
            .try_get::<Option<i64>, _>("cache_read_tokens")?
            .unwrap_or_default(),
        cache_write_tokens: row
            .try_get::<Option<i64>, _>("cache_write_tokens")?
            .unwrap_or_default(),
        cost_usd: row.try_get("cost_usd")?,
    }))
}

fn rate_limit_snapshot(snapshot_json: Option<&str>) -> Option<Value> {
    let snapshot = serde_json::from_str::<Value>(snapshot_json?).ok()?;
    let config = snapshot.get("config").unwrap_or(&snapshot);
    snapshot
        .get("rate_limit_snapshot")
        .or_else(|| snapshot.get("rate_limit"))
        .or_else(|| config.get("rate_limit_snapshot"))
        .or_else(|| config.get("rate_limit"))
        .cloned()
}

fn api_lifecycle_state(value: &str) -> Result<TaskLifecycleState, ServiceError> {
    match value {
        "backlog" => Ok(TaskLifecycleState::Backlog),
        "ready" => Ok(TaskLifecycleState::Ready),
        "active" => Ok(TaskLifecycleState::Active),
        "blocked" => Ok(TaskLifecycleState::Blocked),
        "ready_to_merge" => Ok(TaskLifecycleState::ReadyToMerge),
        "merging" => Ok(TaskLifecycleState::Merging),
        "done" => Ok(TaskLifecycleState::Done),
        "cancelled" => Ok(TaskLifecycleState::Cancelled),
        _ => Err(ServiceError::invalid_operation(format!(
            "unknown TaskLifecycle state `{value}`"
        ))),
    }
}

fn effective_policy(
    snapshot_json: Option<&str>,
    workspace_path: Option<&str>,
) -> Option<EffectiveExecutionPolicy> {
    let snapshot = serde_json::from_str::<Value>(snapshot_json?).ok()?;
    if let Some(mut policy) = snapshot
        .get("effective_execution_policy")
        .cloned()
        .and_then(|value| serde_json::from_value::<EffectiveExecutionPolicy>(value).ok())
    {
        if let Some(workspace_path) = workspace_path {
            policy.effective_cwd = Some(workspace_path.to_owned());
            policy.workspace_root = Some(workspace_path.to_owned());
        }
        return Some(policy);
    }
    // Historical snapshot compatibility. New executions persist the adapter
    // interpretation above; PR13 removes this reader with legacy snapshots.
    let config = snapshot.get("config").unwrap_or(&Value::Null);
    let executor_kind = snapshot
        .get("executor_type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let permission_policy = config
        .get("permission_policy")
        .and_then(Value::as_str)
        .or_else(|| snapshot.get("permission_policy").and_then(Value::as_str))
        .unwrap_or("supervised")
        .to_owned();
    let isolation_posture = isolation_posture(&executor_kind, &permission_policy, config);
    let is_high_risk = matches!(isolation_posture.as_str(), "danger-full-access")
        || config
            .get("dangerously_skip_permissions")
            .and_then(Value::as_bool)
            .unwrap_or(false);

    Some(EffectiveExecutionPolicy {
        executor_kind,
        permission_policy,
        isolation_posture,
        is_high_risk,
        effective_cwd: workspace_path.map(str::to_owned),
        workspace_root: workspace_path.map(str::to_owned),
        environment_posture: environment_posture(config),
        scoped_tools: string_array(config.get("scoped_tools")),
        mcp_servers: string_array(config.get("mcp_servers")),
    })
}

fn isolation_posture(executor_kind: &str, permission_policy: &str, config: &Value) -> String {
    match executor_kind {
        "codex" => config
            .get("sandbox")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| match permission_policy {
                "plan" => "read-only".to_owned(),
                _ => "workspace-write".to_owned(),
            }),
        "claude_code" => {
            if config
                .get("dangerously_skip_permissions")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "dangerously_skip_permissions".to_owned()
            } else if config.get("plan").and_then(Value::as_bool).unwrap_or(false) {
                "plan".to_owned()
            } else {
                config
                    .get("approvals")
                    .and_then(Value::as_str)
                    .unwrap_or("default")
                    .to_owned()
            }
        }
        "cursor" => {
            if config
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(permission_policy != "plan")
            {
                "force".to_owned()
            } else {
                "propose_only".to_owned()
            }
        }
        _ => "not_applicable".to_owned(),
    }
}

fn environment_posture(config: &Value) -> String {
    match config.get("env").and_then(Value::as_object) {
        Some(env) if !env.is_empty() => "custom".to_owned(),
        _ => "inherits_process".to_owned(),
    }
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn is_missing_table(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Database(database_error) => database_error.message().contains("no such table"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    //! Fixture tests for operator status payloads. Each test sets up specific DB rows and
    //! workspace state, calls compute_status(), and asserts the resulting payload shape.
    //! To add a new fixture: insert rows using the helpers in this module, call compute_status(),
    //! and assert the fields you care about. Tests are independent (in-memory SQLite per test).

    use super::*;
    use db::{create_sqlite_pool, new_uuid_v4, run_migrations, CreateDomainEvent, DomainEventRepo};
    use serde_json::json;

    async fn test_service() -> (Arc<SqliteDb>, OperatorStatusService) {
        let pool = create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        run_migrations(&pool).await.expect("migrations run");
        let db = Arc::new(SqliteDb::new(pool));
        let service = OperatorStatusService::new(Arc::clone(&db));
        (db, service)
    }

    async fn seed_project_repo(db: &SqliteDb) -> (String, String) {
        let project_id = new_uuid_v4();
        let repo_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
             VALUES (?, ?, '{}', '{}', ?, ?)",
        )
        .bind(&project_id)
        .bind(format!("Project {project_id}"))
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("project inserts");

        sqlx::query(
            "INSERT INTO repo (id, project_id, name, remote_url, local_path, work_mode, default_branch, created_at, updated_at)
             VALUES (?, ?, ?, ?, NULL, 'direct_merge', 'main', ?, ?)",
        )
        .bind(&repo_id)
        .bind(&project_id)
        .bind(format!("Repo {repo_id}"))
        .bind(format!("https://example.com/{repo_id}.git"))
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("repo inserts");

        (project_id, repo_id)
    }

    async fn insert_task(db: &SqliteDb, status: &str) -> String {
        let (project_id, repo_id) = seed_project_repo(db).await;
        let task_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO task (id, project_id, repo_id, title, status, priority, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(&task_id)
        .bind(&project_id)
        .bind(&repo_id)
        .bind(format!("Task {task_id}"))
        .bind(status)
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("task inserts");
        task_id
    }

    async fn insert_execution(
        db: &SqliteDb,
        task_id: &str,
        status: &str,
        error: Option<&str>,
        updated_at: DateTime<Utc>,
    ) -> String {
        let execution_id = new_uuid_v4();
        let now = updated_at.to_rfc3339();
        sqlx::query(
            "INSERT INTO execution (id, task_id, role, status, error, created_at, updated_at, stopped_at)
             VALUES (?, ?, 'executor', ?, ?, ?, ?, ?)",
        )
        .bind(&execution_id)
        .bind(task_id)
        .bind(status)
        .bind(error)
        .bind(&now)
        .bind(&now)
        .bind((status != "running").then_some(now.as_str()))
        .execute(db.pool())
        .await
        .expect("execution inserts");
        execution_id
    }

    async fn insert_rejected_transition(db: &SqliteDb, task_id: &str) {
        let transition_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO transition_log (id, task_id, from_state, to_state, trigger_name, triggered_by, trigger_reason, rejection, created_at)
             VALUES (?, ?, 'review', 'in_progress', NULL, 'system', 'fixture rejection', 1, ?)",
        )
        .bind(&transition_id)
        .bind(task_id)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("transition log inserts");
    }

    async fn insert_retry_receipt(db: &SqliteDb, task_id: &str) -> (String, String, String) {
        let now = Utc::now().to_rfc3339();
        let failure_ref = new_uuid_v4();
        let source_event = DomainEventRepo::append_event(
            db,
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "execution.failed".to_owned(),
                entity_type: "execution".to_owned(),
                entity_id: failure_ref.clone(),
                actor_type: "agent".to_owned(),
                actor_id: Some("agent-exact".to_owned()),
                scope_type: "task".to_owned(),
                scope_id: task_id.to_owned(),
                correlation_id: failure_ref.clone(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: None,
                payload_json: json!({"execution_id": failure_ref.clone()}).to_string(),
                created_at: now.clone(),
            },
        )
        .await
        .expect("source failure event");
        let retry_receipt_id = new_uuid_v4();
        let receipt_event = DomainEventRepo::append_event(
            db,
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.rework_requested".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: task_id.to_owned(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task_id.to_owned(),
                correlation_id: retry_receipt_id.clone(),
                causation_id: Some(source_event.id.clone()),
                causation_depth: 1,
                dedupe_key: None,
                payload_json: json!({
                    "task_id": task_id,
                    "failure_kind": "execution_failed",
                    "failure_ref": failure_ref.clone(),
                    "source_event_id": source_event.id.clone(),
                    "attempt_number": 2,
                    "retry_budget": 3,
                    "disposition": "rework",
                })
                .to_string(),
                created_at: now.clone(),
            },
        )
        .await
        .expect("retry receipt event");
        sqlx::query(
            "INSERT INTO task_failure_retry_receipt (
                id, task_id, failure_kind, failure_ref, source_event_id,
                attempt_number, retry_budget, disposition, policy_ref,
                policy_version, policy_digest, receipt_event_id, created_at, retry_epoch
             ) VALUES (?, ?, 'execution_failed', ?, ?, 2, 3, 'rework',
                       'forge.task_failure_retry', 3, ?, ?, ?, 0)",
        )
        .bind(&retry_receipt_id)
        .bind(task_id)
        .bind(&failure_ref)
        .bind(&source_event.id)
        .bind("a".repeat(64))
        .bind(&receipt_event.id)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("exact retry receipt");
        (retry_receipt_id, source_event.id, receipt_event.id)
    }

    async fn insert_daemon(db: &SqliteDb, status: &str) -> String {
        let daemon_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO daemon (id, machine_id, hostname, os, arch, labels_json, status, detected_clis_json, created_at, updated_at)
             VALUES (?, ?, 'host', 'macos', 'aarch64', '{}', ?, '[]', ?, ?)",
        )
        .bind(&daemon_id)
        .bind(format!("machine-{daemon_id}"))
        .bind(status)
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("daemon inserts");
        daemon_id
    }

    async fn insert_workspace_at_path(
        db: &SqliteDb,
        task_id: &str,
        status: &str,
        cleanup_after: &str,
        worktree_path: &str,
    ) -> String {
        let repo_id = sqlx::query_scalar::<_, String>("SELECT repo_id FROM task WHERE id = ?")
            .bind(task_id)
            .fetch_one(db.pool())
            .await
            .expect("task repo exists");
        let workspace_id = new_uuid_v4();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO workspace (id, task_id, repo_id, worktree_path, branch, status, cleanup_after, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'main', ?, ?, ?, ?)",
        )
        .bind(&workspace_id)
        .bind(task_id)
        .bind(&repo_id)
        .bind(worktree_path)
        .bind(status)
        .bind(cleanup_after)
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("workspace inserts");
        workspace_id
    }

    async fn insert_workspace(db: &SqliteDb, task_id: &str, status: &str, cleanup_after: &str) {
        let workspace_path = format!("/tmp/worktree-{}", new_uuid_v4());
        insert_workspace_at_path(db, task_id, status, cleanup_after, &workspace_path).await;
    }

    #[tokio::test]
    async fn empty_db_is_healthy() {
        let (_db, service) = test_service().await;

        let status = service.compute_status().await.expect("status computes");

        assert_eq!(status.overall_severity, OperatorSeverity::Healthy);
        assert!(status.active_executions.is_empty());
        assert!(status.blocked_tasks.is_empty());
        assert!(status.daemon_issues.is_empty());
        assert!(status.workspace_cleanup.is_empty());
        assert!(status.retry_pressure.is_empty());
        assert!(status.recent_errors.is_empty());
    }

    #[tokio::test]
    async fn running_execution_healthy() {
        let (db, service) = test_service().await;
        let task_id = insert_task(&db, "todo").await;
        insert_execution(&db, &task_id, "running", None, Utc::now()).await;

        let status = service.compute_status().await.expect("status computes");

        assert_eq!(status.overall_severity, OperatorSeverity::Healthy);
        assert_eq!(status.active_executions.len(), 1);
        assert_eq!(status.active_executions[0].task_id, task_id);
    }

    #[tokio::test]
    async fn blocked_task_severity() {
        let (db, service) = test_service().await;
        let task_id = insert_task(&db, "blocked").await;

        let status = service.compute_status().await.expect("status computes");

        assert_eq!(status.overall_severity, OperatorSeverity::Blocked);
        assert_eq!(status.blocked_tasks.len(), 1);
        assert_eq!(status.blocked_tasks[0].task_id, task_id);
        assert_eq!(
            status.blocked_tasks[0].lifecycle_state,
            TaskLifecycleState::Blocked
        );
        assert_eq!(status.blocked_tasks[0].lifecycle_version, 1);
    }

    #[tokio::test]
    async fn offline_daemon_attention() {
        let (db, service) = test_service().await;
        let daemon_id = insert_daemon(&db, "offline").await;

        let status = service.compute_status().await.expect("status computes");

        assert_eq!(status.overall_severity, OperatorSeverity::Attention);
        assert_eq!(status.daemon_issues.len(), 1);
        assert_eq!(status.daemon_issues[0].daemon_id, daemon_id);
        assert_eq!(status.daemon_issues[0].issue, "offline");
        assert_eq!(
            status.daemon_issues[0].severity,
            OperatorSeverity::Attention
        );
    }

    #[tokio::test]
    async fn error_daemon_reports_daemon_issue() {
        let (db, service) = test_service().await;
        // This fixture exercises a forward-compatible status value consumed by
        // operator status even though current daemon lifecycle only emits
        // online/offline.
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(db.pool())
            .await
            .expect("check constraints disabled for fixture");
        let daemon_id = insert_daemon(&db, "error").await;
        sqlx::query("PRAGMA ignore_check_constraints = OFF")
            .execute(db.pool())
            .await
            .expect("check constraints restored");

        let status = service.compute_status().await.expect("status computes");

        assert_eq!(status.overall_severity, OperatorSeverity::Error);
        assert_eq!(status.daemon_issues.len(), 1);
        assert_eq!(status.daemon_issues[0].daemon_id, daemon_id);
        assert_eq!(status.daemon_issues[0].issue, "error");
        assert_eq!(status.daemon_issues[0].severity, OperatorSeverity::Error);
    }

    #[tokio::test]
    async fn cleanup_backlog_attention() {
        let (db, service) = test_service().await;
        let task_id = insert_task(&db, "done").await;
        let cleanup_after = (Utc::now() - Duration::hours(1)).to_rfc3339();
        insert_workspace(&db, &task_id, "ready", &cleanup_after).await;

        let status = service.compute_status().await.expect("status computes");

        assert_eq!(status.overall_severity, OperatorSeverity::Attention);
        assert_eq!(status.workspace_cleanup.len(), 1);
        assert_eq!(status.workspace_cleanup[0].task_id, task_id);
    }

    #[tokio::test]
    async fn failed_plus_blocked_error() {
        let (db, service) = test_service().await;
        let blocked_task_id = insert_task(&db, "blocked").await;
        let failed_task_id = insert_task(&db, "todo").await;
        insert_execution(
            &db,
            &failed_task_id,
            "failed",
            Some("executor failed"),
            Utc::now(),
        )
        .await;

        let status = service.compute_status().await.expect("status computes");

        assert_eq!(status.overall_severity, OperatorSeverity::Error);
        assert_eq!(status.blocked_tasks.len(), 1);
        assert_eq!(status.blocked_tasks[0].task_id, blocked_task_id);
        assert_eq!(status.recent_errors.len(), 1);
        assert_eq!(status.recent_errors[0].entity_type, "task");
        assert_eq!(status.recent_errors[0].entity_id, failed_task_id);
    }

    #[tokio::test]
    async fn terminal_tasks_do_not_report_stale_execution_errors() {
        let (db, service) = test_service().await;
        let task_id = insert_task(&db, "done").await;
        insert_execution(
            &db,
            &task_id,
            "failed",
            Some("superseded by successful retry"),
            Utc::now(),
        )
        .await;

        let status = service.compute_status().await.expect("status computes");

        assert!(status.recent_errors.is_empty());
        assert_eq!(status.overall_severity, OperatorSeverity::Healthy);
    }

    #[tokio::test]
    async fn legacy_transition_logs_do_not_report_retry_pressure() {
        let (db, service) = test_service().await;
        let active_task_id = insert_task(&db, "in_progress").await;
        insert_rejected_transition(&db, &active_task_id).await;

        let status = service.compute_status().await.expect("status computes");

        assert!(status.retry_pressure.is_empty());
    }

    #[tokio::test]
    async fn retry_pressure_reports_exact_receipt_and_lifecycle_identity() {
        let (db, service) = test_service().await;
        let task_id = insert_task(&db, "in_progress").await;
        let (retry_receipt_id, source_event_id, receipt_event_id) =
            insert_retry_receipt(&db, &task_id).await;

        let status = service.compute_status().await.expect("status computes");

        assert_eq!(status.retry_pressure.len(), 1);
        let receipt = &status.retry_pressure[0];
        assert_eq!(receipt.retry_receipt_id, retry_receipt_id);
        assert_eq!(receipt.task_id, task_id);
        assert_eq!(receipt.failure_kind, "execution_failed");
        assert_eq!(receipt.attempt_number, 2);
        assert_eq!(receipt.retry_budget, 3);
        assert_eq!(receipt.disposition, "rework");
        assert_eq!(receipt.lifecycle_state, TaskLifecycleState::Active);
        assert_eq!(receipt.lifecycle_version, 1);
        assert_eq!(receipt.source_event_id, source_event_id);
        assert_eq!(receipt.receipt_event_id, receipt_event_id);
    }

    #[tokio::test]
    async fn mixed_operational_indicators_use_highest_severity() {
        let (db, service) = test_service().await;
        let running_task_id = insert_task(&db, "in_progress").await;
        insert_execution(&db, &running_task_id, "running", None, Utc::now()).await;

        let blocked_task_id = insert_task(&db, "blocked").await;
        let daemon_id = insert_daemon(&db, "offline").await;

        let cleanup_task_id = insert_task(&db, "done").await;
        let cleanup_after = (Utc::now() - Duration::hours(1)).to_rfc3339();
        insert_workspace(&db, &cleanup_task_id, "ready", &cleanup_after).await;

        let failed_task_id = insert_task(&db, "todo").await;
        insert_execution(
            &db,
            &failed_task_id,
            "failed",
            Some("executor failed"),
            Utc::now(),
        )
        .await;

        let status = service.compute_status().await.expect("status computes");

        assert_eq!(status.overall_severity, OperatorSeverity::Error);
        assert!(!status.active_executions.is_empty());
        assert!(!status.blocked_tasks.is_empty());
        assert_eq!(status.blocked_tasks[0].task_id, blocked_task_id);
        assert!(!status.daemon_issues.is_empty());
        assert_eq!(status.daemon_issues[0].daemon_id, daemon_id);
        assert!(!status.workspace_cleanup.is_empty());
    }
}
