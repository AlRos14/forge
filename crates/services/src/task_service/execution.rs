use super::*;
use crate::agent_capacity::count_running_executions;
use crate::workflow::dispatch::{
    build_effective_prompt, dispatch_intent_from_workflow_dispatch, effective_prompt_selection,
    loader::load_agent_dispatch_context,
};
use api_types::SystemComponent;
use db::{CreateReview, ExecutionUsageRepo, UpdateTask, UpdateTaskStatus};

mod cascade;
mod follow_up;
mod guards;
mod hooks;
mod launch;
mod recovery;
mod runner;
pub(in crate::task_service) mod subtasks;

pub(super) use cascade::should_block_task_for_failed_execution;

pub(super) fn publish_terminal_execution_event(service: &TaskService, execution: &Execution) {
    match execution.status {
        ExecutionStatus::Completed => service.publish(ForgeEvent {
            event_type: "execution.completed".to_owned(),
            entity_id: execution.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ExecutionCompleted {
                task_id: execution.task_id.clone(),
            },
        }),
        ExecutionStatus::Failed => service.publish(ForgeEvent {
            event_type: "execution.failed".to_owned(),
            entity_id: execution.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ExecutionFailed {
                task_id: execution.task_id.clone(),
                error: execution
                    .error
                    .clone()
                    .unwrap_or_else(|| "execution failed".to_owned()),
            },
        }),
        ExecutionStatus::Cancelled => service.publish(ForgeEvent {
            event_type: "execution.cancelled".to_owned(),
            entity_id: execution.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ExecutionCancelled {
                task_id: execution.task_id.clone(),
                reason: execution
                    .error
                    .clone()
                    .unwrap_or_else(|| "execution cancelled".to_owned()),
            },
        }),
        ExecutionStatus::Running => {}
    };
}

pub(super) async fn clear_execution_retry_metadata(db: &SqliteDb, task: &Task) -> Result<()> {
    let mut metadata = TaskMetadata::parse(task.metadata_json.as_deref()).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid task metadata for {}: {error}", task.id))
    })?;
    let mut changed = false;
    for key in [
        "execution_retry_count",
        "last_execution_failure_at",
        "deferred_dispatch",
    ] {
        changed |= metadata.extra.remove(key).is_some();
    }
    if changed {
        TaskRepo::set_metadata_json(db, &task.id, metadata.to_json(), &now_rfc3339()).await?;
    }
    Ok(())
}

pub(super) fn usage_provider_from_agent_config(agent_config: &Value) -> String {
    usage_provider_for_executor_type(agent_config.get("executor_type").and_then(Value::as_str))
}

pub(super) fn usage_provider_from_snapshot(snapshot_json: Option<&str>) -> String {
    let executor_type = snapshot_json
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| {
            value
                .get("executor_type")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    usage_provider_for_executor_type(executor_type.as_deref())
}

fn usage_provider_for_executor_type(executor_type: Option<&str>) -> String {
    match executor_type.unwrap_or_default() {
        "codex" => "openai",
        "claude_code" => "anthropic",
        "cursor" => "cursor",
        "opencode" => "opencode",
        other if !other.is_empty() => other,
        _ => "unknown",
    }
    .to_owned()
}

pub(super) fn normalize_account_usage(executor_type: &str, account_usage: &Value) -> Value {
    if executor_type != "codex" {
        return account_usage.clone();
    }
    if account_usage.get("rateLimits").is_some() {
        return account_usage.clone();
    }
    if account_usage.get("primary").is_some() || account_usage.get("planType").is_some() {
        return json!({ "rateLimits": account_usage });
    }
    account_usage.clone()
}

pub(super) fn account_usage_from_log_entry(entry: &executors::LogEntry) -> Option<Value> {
    if entry.payload.get("method").and_then(Value::as_str) != Some("account/rateLimits/updated") {
        return None;
    }
    entry
        .payload
        .get("params")
        .cloned()
        .or_else(|| Some(entry.payload.clone()))
}

/// Quota pool key used by GET /agents/{id}/usage: executor account key plus
/// the agent's pinned daemon, never an auto-resolved daemon id.
pub(super) fn snapshot_usage_account_key(value: &Value) -> Option<(String, Option<String>)> {
    let executor_type = value.get("executor_type").and_then(Value::as_str)?;
    let kind = executor_type.parse::<ExecutorKind>().ok()?;
    let config = value.get("config").unwrap_or(&Value::Null);
    let mut account_key = executors::account_key(&kind, config);
    let daemon_id = value
        .get("agent_daemon_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned);
    if let Some(daemon_id) = daemon_id.as_deref() {
        account_key.push('@');
        account_key.push_str(daemon_id);
    }
    Some((account_key, daemon_id))
}

pub(super) async fn persist_account_usage_snapshot(
    db: &SqliteDb,
    snapshot: Option<&str>,
    execution_id: &str,
    account_usage: &Value,
) -> Result<()> {
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    let value: Value = serde_json::from_str(snapshot).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "invalid executor snapshot for account usage: {error}"
        ))
    })?;
    let Some(executor_type) = value.get("executor_type").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some((account_key, daemon_id)) = snapshot_usage_account_key(&value) else {
        return Ok(());
    };
    let usage_json = normalize_account_usage(executor_type, account_usage);
    let captured_at = now_rfc3339();
    let stale_after = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    sqlx::query(
        "INSERT INTO account_usage_snapshot
         (id, account_key, executor_type, daemon_id, source, usage_json, captured_at, stale_after, execution_id)
         VALUES (?, ?, ?, ?, 'provider_event', ?, ?, ?, ?)",
    )
    .bind(new_uuid_v4())
    .bind(account_key)
    .bind(executor_type)
    .bind(daemon_id.as_deref())
    .bind(usage_json.to_string())
    .bind(captured_at)
    .bind(stale_after)
    .bind(execution_id)
    .execute(db.pool())
    .await?;
    Ok(())
}

const CURSOR_USAGE_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(45);

pub(super) fn spawn_cursor_usage_probe(
    db: std::sync::Arc<SqliteDb>,
    snapshot: Option<String>,
    execution_id: String,
) -> Option<tokio::sync::oneshot::Sender<()>> {
    let executor_type = snapshot.as_deref().and_then(|snapshot| {
        serde_json::from_str::<Value>(snapshot)
            .ok()?
            .get("executor_type")?
            .as_str()
            .map(ToOwned::to_owned)
    });
    if executor_type.as_deref() != Some("cursor") {
        return None;
    }
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        loop {
            persist_cursor_account_usage_probe(&db, snapshot.as_deref(), &execution_id).await;
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = tokio::time::sleep(CURSOR_USAGE_PROBE_INTERVAL) => {}
            }
        }
    });
    Some(stop_tx)
}

async fn persist_cursor_account_usage_probe(
    db: &SqliteDb,
    snapshot: Option<&str>,
    execution_id: &str,
) {
    let Some(snapshot) = snapshot else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(snapshot) else {
        return;
    };
    let config = serde_json::from_value::<executors::CursorConfig>(
        value.get("config").cloned().unwrap_or(Value::Null),
    )
    .unwrap_or_default();
    match cli_adapters::cursor::query_account_usage(&config).await {
        Ok(usage) => {
            if let Err(error) =
                persist_account_usage_snapshot(db, Some(snapshot), execution_id, &usage).await
            {
                tracing::warn!(
                    execution_id = %execution_id,
                    %error,
                    "failed to persist Cursor account usage snapshot"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                execution_id = %execution_id,
                %error,
                "failed to probe Cursor account usage"
            );
        }
    }
}

pub(super) async fn set_planning_awaiting_review_metadata(
    db: &SqliteDb,
    task: &Task,
    execution_id: Option<&str>,
    awaiting: bool,
) -> Result<Task> {
    let mut metadata = TaskMetadata::parse(task.metadata_json.as_deref()).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid task metadata for {}: {error}", task.id))
    })?;
    if awaiting {
        metadata
            .extra
            .insert("awaiting_human".to_owned(), json!(true));
        metadata
            .extra
            .insert("awaiting_human_reason".to_owned(), json!("plan_review"));
        metadata.extra.insert(
            "planning_completed_at".to_owned(),
            Value::String(now_rfc3339()),
        );
        if let Some(execution_id) = execution_id {
            metadata.extra.insert(
                "planning_execution_id".to_owned(),
                Value::String(execution_id.to_owned()),
            );
        }
    } else if metadata
        .extra
        .get("awaiting_human_reason")
        .and_then(Value::as_str)
        == Some("plan_review")
    {
        metadata.extra.remove("awaiting_human");
        metadata.extra.remove("awaiting_human_reason");
        metadata.extra.remove("planning_completed_at");
        metadata.extra.remove("planning_execution_id");
    } else {
        return Ok(task.clone());
    }

    TaskRepo::set_metadata_json(db, &task.id, metadata.to_json(), &now_rfc3339()).await?;
    TaskRepo::get_by_id(db, &task.id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))
}

pub(super) async fn reviewer_role_assigned(db: &SqliteDb, task_id: &str) -> Result<bool> {
    Ok(TaskRoleAssignmentRepo::get_by_task_and_role(
        db,
        task_id,
        crate::workflow::default_roles::REVIEWER,
    )
    .await?
    .is_some_and(|assignment| assignment.assignee_id.is_some()))
}

pub(super) async fn persist_planner_result(
    db: &SqliteDb,
    task: &Task,
    execution: &Execution,
) -> Result<&'static str> {
    let payload = execution
        .summary
        .as_deref()
        .unwrap_or_default()
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("FORGE_RESULT: "));
    let Some(payload) = payload else {
        return Err(ServiceError::invalid_operation(
            "planner structured result missing",
        ));
    };
    let value: Value = serde_json::from_str(payload).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "planner structured result is invalid JSON: {error}"
        ))
    })?;
    if value.get("schema_version").and_then(Value::as_i64) != Some(1) {
        return Err(ServiceError::invalid_operation(
            "planner structured result version is unsupported",
        ));
    }
    match value.get("kind").and_then(Value::as_str) {
        Some("plan_ready") => {
            let workspace = WorkspaceRepo::get_by_task_id(db, &task.id)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation("planner completed without a workspace")
                })?;
            crate::plan_artifact::capture_plan_revision(
                db,
                &task.id,
                std::path::Path::new(&workspace.worktree_path),
                "planner_ready",
                Some(&execution.id),
            )
            .await
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            Ok("plan_review")
        }
        Some("decision_request") => {
            let scope = value
                .get("authority_scope")
                .and_then(Value::as_str)
                .unwrap_or("task");
            if !matches!(scope, "task" | "project_scope" | "policy" | "risk") {
                return Err(ServiceError::invalid_operation(
                    "planner decision authority_scope is invalid",
                ));
            }
            let questions = value
                .get("questions")
                .filter(|value| {
                    value.as_array().is_some_and(|items| {
                        !items.is_empty()
                            && items.iter().all(|item| {
                                item.as_object().is_some_and(|question| {
                                    question
                                        .get("question")
                                        .and_then(Value::as_str)
                                        .is_some_and(|text| !text.trim().is_empty())
                                })
                            })
                    })
                })
                .ok_or_else(|| {
                    ServiceError::invalid_operation("planner decision request requires questions")
                })?;
            sqlx::query(
                "INSERT OR IGNORE INTO task_decision_request
                 (id, task_id, execution_id, role, authority_scope, questions_json, context, status, created_at)
                 VALUES (?, ?, ?, 'planner', ?, ?, ?, 'pending', ?)",
            )
            .bind(new_uuid_v4()).bind(&task.id).bind(&execution.id).bind(scope)
            .bind(questions.to_string())
            .bind(value.get("context").and_then(Value::as_str))
            .bind(now_rfc3339()).execute(db.pool()).await?;
            Ok("decision_request")
        }
        _ => Err(ServiceError::invalid_operation(
            "planner structured result kind is invalid",
        )),
    }
}

pub(super) async fn conclude_planner_ready(
    service: &TaskService,
    task: &Task,
    execution_id: &str,
    persist_reason: &str,
) -> Result<()> {
    let current = TaskRepo::get_by_id(&*service.db, &task.id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?;
    if persist_reason == "plan_review"
        && crate::workflow::task_requests_plan_review(current.task_state_config.as_deref())
        && reviewer_role_assigned(&service.db, &current.id).await?
    {
        service
            .transition(
                current.id.clone(),
                crate::workflow::default_states::PLAN_REVIEW.to_owned(),
                TransitionOptions {
                    version: current.version,
                    reason: Some("planner produced a plan for independent review".to_owned()),
                    triggered_by: Actor::system(SystemComponent::Workflow),
                    rejection: false,
                    defer_dispatch_seconds: None,
                },
            )
            .await?;
        return Ok(());
    }

    let marked =
        set_planning_awaiting_review_metadata(&service.db, &current, Some(execution_id), true)
            .await?;
    if persist_reason != "plan_review" {
        if let Ok(mut metadata) = TaskMetadata::parse(marked.metadata_json.as_deref()) {
            metadata
                .extra
                .insert("awaiting_human_reason".to_owned(), json!(persist_reason));
            let _ = TaskRepo::set_metadata_json(
                &*service.db,
                &marked.id,
                metadata.to_json(),
                &now_rfc3339(),
            )
            .await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        account_usage_from_log_entry, normalize_account_usage, snapshot_usage_account_key,
    };
    use executors::{LogEntry, LogKind, LogStream};
    use serde_json::json;

    #[test]
    fn wraps_codex_rate_limit_params_as_rate_limits() {
        let params = json!({
            "planType": "plus",
            "primary": { "usedPercent": 19, "windowDurationMins": 300 }
        });
        assert_eq!(
            normalize_account_usage("codex", &params),
            json!({ "rateLimits": params })
        );
    }

    #[test]
    fn leaves_cursor_and_wrapped_codex_payloads_unchanged() {
        let cursor = json!({ "plan": "Pro", "categories": { "included": 11 } });
        let wrapped = json!({
            "rateLimits": { "planType": "plus", "primary": { "usedPercent": 2 } }
        });
        assert_eq!(normalize_account_usage("cursor", &cursor), cursor);
        assert_eq!(normalize_account_usage("codex", &wrapped), wrapped);
    }

    #[test]
    fn usage_account_key_ignores_auto_resolved_daemon() {
        let snapshot = json!({
            "executor_type": "codex",
            "config": { "base_command_override": "codex2" },
            "resolved_daemon_id": "daemon-auto",
            "agent_daemon_id": null
        });
        assert_eq!(
            snapshot_usage_account_key(&snapshot),
            Some(("codex:cmd=codex2".to_owned(), None))
        );
    }

    #[test]
    fn usage_account_key_pins_only_agent_daemon() {
        let snapshot = json!({
            "executor_type": "codex",
            "config": { "env": { "CODEX_HOME": "/tmp/codex-plus2" } },
            "resolved_daemon_id": "daemon-auto",
            "agent_daemon_id": "daemon-pinned"
        });
        assert_eq!(
            snapshot_usage_account_key(&snapshot),
            Some((
                "codex:home=/tmp/codex-plus2@daemon-pinned".to_owned(),
                Some("daemon-pinned".to_owned())
            ))
        );
    }

    #[test]
    fn extracts_rate_limit_params_from_session_log() {
        let entry = LogEntry {
            schema_version: 1,
            sequence: 3,
            timestamp: "2026-09-07T00:00:00Z".to_owned(),
            execution_id: "exec".to_owned(),
            kind: LogKind::SessionInfo,
            stream: LogStream::Main,
            payload: json!({
                "method": "account/rateLimits/updated",
                "params": { "planType": "plus", "primary": { "usedPercent": 21 } }
            }),
            truncated: false,
        };
        assert_eq!(
            account_usage_from_log_entry(&entry),
            Some(json!({ "planType": "plus", "primary": { "usedPercent": 21 } }))
        );
        let other = LogEntry {
            payload: json!({ "method": "turn/completed" }),
            ..entry.clone()
        };
        assert_eq!(account_usage_from_log_entry(&other), None);
    }
}
