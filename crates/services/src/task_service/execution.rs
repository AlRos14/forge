use super::*;
use crate::agent_capacity::count_running_executions;
use crate::workflow::dispatch::{
    build_effective_prompt, dispatch_intent_from_workflow_dispatch, effective_prompt_selection,
    loader::load_agent_dispatch_context,
};
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

pub(crate) fn account_usage_from_log_entry(entry: &executors::LogEntry) -> Option<Value> {
    account_usage_from_payload(&entry.payload)
}

pub(crate) fn account_usage_from_payload(payload: &Value) -> Option<Value> {
    matches!(
        payload.get("method").and_then(Value::as_str),
        Some("account/rateLimits/updated" | "forge/cursor/usage")
    )
    .then(|| payload.get("params").cloned())
    .flatten()
}

pub(crate) fn account_usage_source_from_payload(payload: &Value) -> Option<&'static str> {
    match payload.get("method").and_then(Value::as_str) {
        Some("account/rateLimits/updated") => Some("provider_event"),
        Some("forge/cursor/usage") => Some("cursor_poll"),
        _ => None,
    }
}

fn snapshot_usage_account_key(value: &Value) -> Option<(String, Option<String>)> {
    snapshot_usage_account_key_for_host(value, None)
}

fn snapshot_usage_account_key_for_host(
    value: &Value,
    host_identity_override: Option<&str>,
) -> Option<(String, Option<String>)> {
    let executor_type = value.get("executor_type").and_then(Value::as_str)?;
    let kind = executor_type.parse::<ExecutorKind>().ok()?;
    let config = value.get("config").unwrap_or(&Value::Null);
    let daemon_id = host_identity_override
        .filter(|id| !id.trim().is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .get("resolved_daemon_id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(ToOwned::to_owned)
        });
    let host_identity = daemon_id.as_deref().unwrap_or("server-local");
    let credential_ref = value
        .get("credential_ref")
        .and_then(Value::as_str)
        .filter(|reference| !reference.trim().is_empty());
    let account_key =
        executors::account_key_for_context(&kind, config, host_identity, credential_ref);
    Some((account_key, daemon_id))
}

fn account_usage_source(value: &Value) -> &'static str {
    if value.get("executor_type").and_then(Value::as_str) == Some("cursor") {
        "cursor_poll"
    } else {
        "provider_event"
    }
}

pub(crate) async fn persist_account_usage_snapshot(
    db: &SqliteDb,
    snapshot: Option<&str>,
    execution_id: &str,
    account_usage: &Value,
) -> Result<()> {
    persist_account_usage_snapshot_with_host(db, snapshot, execution_id, account_usage, None).await
}

pub(crate) async fn persist_account_usage_snapshot_with_host(
    db: &SqliteDb,
    snapshot: Option<&str>,
    execution_id: &str,
    account_usage: &Value,
    host_identity: Option<&str>,
) -> Result<()> {
    let source = snapshot
        .and_then(|snapshot| serde_json::from_str::<Value>(snapshot).ok())
        .map(|value| account_usage_source(&value))
        .unwrap_or("provider_event");
    persist_account_usage_snapshot_with_source_and_host(
        db,
        snapshot,
        execution_id,
        account_usage,
        source,
        host_identity,
    )
    .await
}

pub(crate) async fn persist_account_usage_snapshot_with_source(
    db: &SqliteDb,
    snapshot: Option<&str>,
    execution_id: &str,
    account_usage: &Value,
    source: &str,
) -> Result<()> {
    persist_account_usage_snapshot_with_source_and_host(
        db,
        snapshot,
        execution_id,
        account_usage,
        source,
        None,
    )
    .await
}

pub(crate) async fn persist_account_usage_snapshot_with_source_and_host(
    db: &SqliteDb,
    snapshot: Option<&str>,
    execution_id: &str,
    account_usage: &Value,
    source: &str,
    host_identity: Option<&str>,
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
    let Some((account_key, daemon_id)) = (match host_identity {
        Some(host_identity) => snapshot_usage_account_key_for_host(&value, Some(host_identity)),
        None => snapshot_usage_account_key(&value),
    }) else {
        return Ok(());
    };
    let usage_json = normalize_account_usage(executor_type, account_usage);
    let captured_at = now_rfc3339();
    let stale_after = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    sqlx::query(
        "INSERT INTO account_usage_snapshot
         (id, account_key, executor_type, daemon_id, source, usage_json, captured_at, stale_after, execution_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(new_uuid_v4())
    .bind(account_key)
    .bind(executor_type)
    .bind(daemon_id.as_deref())
    .bind(source)
    .bind(usage_json.to_string())
    .bind(captured_at)
    .bind(stale_after)
    .bind(execution_id)
    .execute(db.pool())
    .await?;
    Ok(())
}

pub(super) struct CursorUsageProbe {
    cancel: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl CursorUsageProbe {
    pub(super) async fn stop(self) {
        self.cancel.cancel();
        let _ = self.task.await;
    }
}

pub(super) fn spawn_cursor_usage_probe(
    db: Arc<SqliteDb>,
    snapshot: Option<String>,
    execution_id: String,
) -> Option<CursorUsageProbe> {
    let is_cursor = snapshot.as_deref().and_then(|snapshot| {
        serde_json::from_str::<Value>(snapshot)
            .ok()?
            .get("executor_type")
            .and_then(Value::as_str)
            .map(|executor_type| executor_type == "cursor")
    }) == Some(true);
    if !is_cursor {
        return None;
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        loop {
            if task_cancel.is_cancelled() {
                break;
            }
            persist_cursor_account_usage_probe(
                &db,
                snapshot.as_deref(),
                &execution_id,
                task_cancel.clone(),
            )
            .await;
            tokio::select! {
                _ = task_cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(45)) => {}
            }
        }
    });
    Some(CursorUsageProbe { cancel, task })
}

async fn persist_cursor_account_usage_probe(
    db: &SqliteDb,
    snapshot: Option<&str>,
    execution_id: &str,
    cancel: tokio_util::sync::CancellationToken,
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
    match cli_adapters::cursor::query_account_usage_with_cancel(&config, cancel.clone()).await {
        Ok(usage) => {
            if let Err(error) = persist_account_usage_snapshot_with_source(
                db,
                Some(snapshot),
                execution_id,
                &usage,
                "cursor_poll",
            )
            .await
            {
                tracing::warn!(
                    execution_id = %execution_id,
                    %error,
                    "failed to persist Cursor account usage snapshot"
                );
            }
        }
        Err(error) if !cancel.is_cancelled() => {
            tracing::warn!(
                execution_id = %execution_id,
                %error,
                "failed to probe Cursor account usage"
            );
        }
        Err(_) => {}
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

#[cfg(test)]
mod tests {
    use super::{
        account_usage_from_log_entry, account_usage_source_from_payload, normalize_account_usage,
        snapshot_usage_account_key,
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
    fn usage_account_key_uses_resolved_daemon_and_wrapper_independent_context() {
        let snapshot = json!({
            "executor_type": "codex",
            "config": { "base_command_override": "codex-work" },
            "resolved_daemon_id": "daemon-auto",
            "agent_daemon_id": null
        });
        assert_eq!(
            snapshot_usage_account_key(&snapshot),
            Some((
                "codex@daemon-auto".to_owned(),
                Some("daemon-auto".to_owned())
            ))
        );
    }

    #[test]
    fn usage_account_key_records_resolved_host_for_pinned_agent() {
        let snapshot = json!({
            "executor_type": "codex",
            "config": { "env": { "CODEX_HOME": "/tmp/codex-plus2" } },
            "resolved_daemon_id": "daemon-auto",
            "agent_daemon_id": "daemon-pinned"
        });
        assert_eq!(
            snapshot_usage_account_key(&snapshot),
            Some((
                "codex@daemon-auto:home=/tmp/codex-plus2".to_owned(),
                Some("daemon-auto".to_owned())
            ))
        );
    }

    #[test]
    fn usage_account_key_uses_explicit_credential_reference_across_hosts() {
        let mut snapshot = json!({
            "executor_type": "codex",
            "config": { "env": { "CODEX_HOME": "/tmp/codex-plus2" } },
            "resolved_daemon_id": "daemon-a",
            "credential_ref": "credential-1"
        });
        let first = snapshot_usage_account_key(&snapshot);
        snapshot["resolved_daemon_id"] = json!("daemon-b");
        assert_eq!(
            first.as_ref().map(|value| &value.0),
            snapshot_usage_account_key(&snapshot)
                .as_ref()
                .map(|value| &value.0)
        );
        assert_eq!(
            first,
            Some((
                "codex:credential=credential-1".to_owned(),
                Some("daemon-a".to_owned())
            ))
        );
    }

    #[test]
    fn extracts_native_codex_rate_limit_events_from_logs() {
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
        assert_eq!(
            account_usage_from_log_entry(&LogEntry {
                payload: json!({ "method": "turn/completed" }),
                ..entry
            }),
            None
        );
    }

    #[test]
    fn classifies_remote_cursor_usage_as_explicit_polling() {
        let payload = json!({
            "method": "forge/cursor/usage",
            "params": { "plan": "Pro" },
            "source": "cursor_poll"
        });
        assert_eq!(
            account_usage_source_from_payload(&payload),
            Some("cursor_poll")
        );
        assert_eq!(
            super::account_usage_from_payload(&payload),
            Some(json!({ "plan": "Pro" }))
        );
    }
}
