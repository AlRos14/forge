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

pub(super) async fn persist_account_usage_snapshot(
    db: &SqliteDb,
    snapshot: Option<&str>,
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
    let kind = executor_type
        .parse::<ExecutorKind>()
        .map_err(ServiceError::invalid_operation)?;
    let config = value.get("config").unwrap_or(&Value::Null);
    let mut account_key = executors::account_key(&kind, config);
    let daemon_id = value.get("resolved_daemon_id").and_then(Value::as_str);
    if let Some(daemon_id) = daemon_id {
        account_key.push('@');
        account_key.push_str(daemon_id);
    }
    let captured_at = now_rfc3339();
    let stale_after = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    sqlx::query(
        "INSERT INTO account_usage_snapshot
         (id, account_key, executor_type, daemon_id, source, usage_json, captured_at, stale_after)
         VALUES (?, ?, ?, ?, 'provider_event', ?, ?, ?)",
    )
    .bind(new_uuid_v4())
    .bind(account_key)
    .bind(executor_type)
    .bind(daemon_id)
    .bind(account_usage.to_string())
    .bind(captured_at)
    .bind(stale_after)
    .execute(db.pool())
    .await?;
    Ok(())
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
