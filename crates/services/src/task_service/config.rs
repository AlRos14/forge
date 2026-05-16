use api_types::GateConfig;

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryBudgetKind {
    Review,
    MergeFix,
    Execution,
}

impl RetryBudgetKind {
    fn key(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::MergeFix => "merge_fix",
            Self::Execution => "execution",
        }
    }

    fn default_value(self) -> i32 {
        match self {
            Self::Review => 3,
            Self::MergeFix => 1,
            Self::Execution => 3,
        }
    }
}

pub(crate) fn runtime_retry_budget(
    task: &Task,
    kind: RetryBudgetKind,
    state_config: Option<&Value>,
    gate_config: Option<&GateConfig>,
) -> Result<i32> {
    if let Some(value) = configured_task_retry_budget(task, kind) {
        return Ok(value);
    }
    if let Some(value) = state_config.and_then(|value| retry_budget_from_value(value, kind)) {
        return Ok(value);
    }
    if matches!(kind, RetryBudgetKind::Review | RetryBudgetKind::MergeFix) {
        if let Some(value) = gate_config.and_then(|config| config.max_rejections) {
            return Ok(value);
        }
    }
    Ok(kind.default_value())
}

pub(crate) fn configured_task_retry_budget(task: &Task, kind: RetryBudgetKind) -> Option<i32> {
    task.task_state_config
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| retry_budget_from_value(&value, kind))
}

fn retry_budget_from_value(value: &Value, kind: RetryBudgetKind) -> Option<i32> {
    value
        .get("retry_budgets")
        .and_then(|budgets| budgets.get(kind.key()))
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| *value >= 0)
}

pub(super) fn executor_snapshot_with_resume_thread(
    snapshot_json: &str,
    agent_session_id: &str,
) -> Result<String> {
    let mut snapshot = parse_json_value("executor config snapshot", snapshot_json)?;
    let executor_type = snapshot
        .get("executor_type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(config) = snapshot.get_mut("config").and_then(Value::as_object_mut) {
        match executor_type.as_deref() {
            Some("codex") => {
                config.insert(
                    RESUME_THREAD_ID_CONFIG_KEY.to_owned(),
                    Value::String(agent_session_id.to_owned()),
                );
                // Task execution follow-ups resume the coder's existing thread.
                // Review/auditor runs use their own config path when they need a
                // separate review context.
                config.insert("resume_thread_in_place".to_owned(), Value::Bool(true));
                config.remove("resume_fallback_prompt");
            }
            Some("claude_code") => {
                config.insert(
                    "resume_session_id".to_owned(),
                    Value::String(agent_session_id.to_owned()),
                );
            }
            _ => {}
        }
    }
    // Mark this snapshot as a session-resume dispatch so the UI can show continuity context
    // without inspecting executor-specific config fields. Keep the existing `dispatch`
    // object in sync because older snapshots and debug views already read it.
    if let Some(obj) = snapshot.as_object_mut() {
        let dispatch = obj
            .entry("dispatch".to_owned())
            .or_insert_with(|| json!({}));
        if let Some(dispatch_obj) = dispatch.as_object_mut() {
            dispatch_obj.insert(
                "execution_policy".to_owned(),
                Value::String("resume_latest_target_role_thread".to_owned()),
            );
        }
        obj.insert(
            "dispatch_metadata".to_owned(),
            json!({ "execution_policy": "resume_latest_target_role_thread" }),
        );
    }
    serde_json::to_string(&snapshot).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid executor config snapshot: {error}"))
    })
}

#[allow(dead_code)]
pub(super) fn executor_snapshot_without_resume_thread(snapshot_json: &str) -> Result<String> {
    let mut snapshot = parse_json_value("executor config snapshot", snapshot_json)?;
    if let Some(config) = snapshot.get_mut("config").and_then(Value::as_object_mut) {
        config.remove(RESUME_THREAD_ID_CONFIG_KEY);
        config.remove("resume_thread_in_place");
        config.remove("resume_fallback_prompt");
        config.remove("resume_session_id");
    }
    if let Some(obj) = snapshot.as_object_mut() {
        obj.remove("dispatch_metadata");
        if let Some(dispatch_obj) = obj.get_mut("dispatch").and_then(Value::as_object_mut) {
            dispatch_obj.remove("execution_policy");
        }
    }
    serde_json::to_string(&snapshot).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid executor config snapshot: {error}"))
    })
}

pub(super) fn truncate_utf8_bytes(bytes: &[u8], max_bytes: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= max_bytes {
        return text.into_owned();
    }

    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = text[..end].to_owned();
    truncated.push_str("[truncated]");
    truncated
}

pub(super) async fn build_executor_config_snapshot(
    db: &SqliteDb,
    _task: &Task,
    agent: &Agent,
    overrides: Option<ExecutionOverrides>,
) -> Result<Option<String>> {
    let resolved_daemon = crate::agent_service::resolve_daemon_for_agent(db, agent).await?;
    let mut base_config = parse_json_value("agent config_json", &agent.config_json)?;
    apply_agent_fields_to_config(agent, &mut base_config)?;
    let capabilities = parse_json_value("agent capabilities_json", &agent.capabilities_json)?;
    let kind = agent
        .executor_type
        .parse::<ExecutorKind>()
        .map_err(ServiceError::invalid_operation)?;
    let execution_overrides = execution_overrides_to_config_layer(overrides)?;
    let (merged_config, overrides_applied) =
        merge_config_layers(&base_config, &execution_overrides);
    let normalized_config =
        resolve_config_value(kind, &merged_config, &ExecutionOverrides::default())?;
    let overrides_applied = overrides_applied.retain_config_keys(&normalized_config);
    let snapshot = json!({
        "agent_id": agent.id,
        "executor_type": agent.executor_type,
        "model": agent.model,
        "reasoning_effort": agent.reasoning_effort,
        "permission_policy": agent.permission_policy,
        "config": normalized_config,
        "capabilities": capabilities,
        "resolved_daemon_id": resolved_daemon.id,
        "overrides_applied": overrides_applied.to_json(),
        "snapshotted_at": now_rfc3339(),
    });
    serde_json::to_string(&snapshot)
        .map(Some)
        .map_err(|error| ServiceError::invalid_operation(format!("invalid JSON snapshot: {error}")))
}

pub(super) async fn create_failed_execution_record(
    db: &SqliteDb,
    task_id: &str,
    agent: &Agent,
    workspace: &Workspace,
    execution_id: &str,
    error: String,
) -> Result<()> {
    let now = now_rfc3339();
    ExecutionRepo::create(
        db,
        CreateExecution {
            id: execution_id.to_owned(),
            task_id: task_id.to_owned(),
            agent_id: Some(agent.id.clone()),
            role: "executor".to_owned(),
            status: ExecutionStatus::Failed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: Some(error),
            executor_config_snapshot_json: None,
            workspace_id: Some(workspace.id.clone()),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await?;
    Ok(())
}

fn apply_agent_fields_to_config(agent: &Agent, config: &mut Value) -> Result<()> {
    let Some(config_object) = config.as_object_mut() else {
        return Err(ServiceError::invalid_operation(
            "agent config_json must be a JSON object",
        ));
    };
    if let Some(model) = &agent.model {
        config_object.insert("model".to_owned(), Value::String(model.clone()));
    }
    if let Some(reasoning_effort) = &agent.reasoning_effort {
        config_object.insert(
            "model_reasoning_effort".to_owned(),
            Value::String(reasoning_effort.clone()),
        );
        config_object.insert("effort".to_owned(), Value::String(reasoning_effort.clone()));
    }
    if let Some(permission_policy) = &agent.permission_policy {
        config_object.insert(
            "permission_policy".to_owned(),
            Value::String(permission_policy.clone()),
        );
    }
    if let Some(prompt_template) = &agent.prompt_template {
        config_object.insert(
            "prompt_template".to_owned(),
            Value::String(prompt_template.clone()),
        );
    }
    Ok(())
}

pub(super) fn parse_json_value(field: &str, value: &str) -> Result<Value> {
    serde_json::from_str(value)
        .map_err(|error| ServiceError::invalid_operation(format!("invalid {field}: {error}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OverridesApplied {
    pub(super) agent: Vec<String>,
    pub(super) execution: Vec<String>,
}

impl OverridesApplied {
    fn to_json(&self) -> Value {
        json!({
            "agent": self.agent,
            "execution": self.execution,
        })
    }

    pub(super) fn retain_config_keys(mut self, config: &Value) -> Self {
        let Some(config_object) = config.as_object() else {
            self.agent.clear();
            self.execution.clear();
            return self;
        };

        self.agent
            .retain(|key| config_object.contains_key(key.as_str()));
        self.execution
            .retain(|key| config_object.contains_key(key.as_str()));
        self
    }
}

pub(super) fn merge_config_layers(agent: &Value, execution: &Value) -> (Value, OverridesApplied) {
    let mut merged = agent.clone();
    let mut overrides_applied = OverridesApplied {
        agent: object_keys(agent),
        execution: Vec::new(),
    };

    merge_override_layer(
        "execution overrides",
        &mut merged,
        execution,
        &mut overrides_applied.execution,
    );

    (merged, overrides_applied)
}

fn object_keys(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

pub(super) fn execution_overrides_to_config_layer(
    overrides: Option<ExecutionOverrides>,
) -> Result<Value> {
    let mut layer = json!({});
    if let Some(overrides) = overrides {
        merge_overrides(&mut layer, &overrides)?;
    }
    Ok(layer)
}

#[cfg(test)]
pub(super) fn parse_config_override_layer(field: &str, value: &str) -> Value {
    match serde_json::from_str::<Value>(value) {
        Ok(value) => override_value_or_empty(field, Some(value)),
        Err(error) => {
            tracing::warn!(field = %field, %error, "config override ignored because it is invalid JSON");
            Value::Object(serde_json::Map::new())
        }
    }
}

#[cfg(test)]
pub(super) fn override_value_or_empty(field: &str, value: Option<Value>) -> Value {
    match value {
        Some(Value::Object(map)) => Value::Object(map),
        Some(Value::Null) | None => Value::Object(serde_json::Map::new()),
        Some(value) => {
            tracing::warn!(
                field = %field,
                value = %value,
                "config override ignored because it is not a JSON object"
            );
            Value::Object(serde_json::Map::new())
        }
    }
}

fn merge_override_layer(
    field: &str,
    merged: &mut Value,
    layer: &Value,
    applied_keys: &mut Vec<String>,
) {
    let Some(layer_object) = layer.as_object() else {
        tracing::warn!(
            field = %field,
            layer = %layer,
            "config override layer ignored because it is not a JSON object"
        );
        return;
    };
    let Some(merged_object) = merged.as_object_mut() else {
        return;
    };
    for (key, value) in layer_object {
        merged_object.insert(key.clone(), value.clone());
        applied_keys.push(key.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_snapshot_with_resume_thread_sets_codex_resume_thread_id() {
        let snapshot_json = r#"{"executor_type":"codex","dispatch":{"execution_policy":"new_execution","target_role":"coder"},"config":{"model":"gpt-5-codex","resume_fallback_prompt":"full prompt should not be reused"}}"#;

        let updated = executor_snapshot_with_resume_thread(snapshot_json, "thread-123")
            .expect("snapshot updates");
        let snapshot: Value = serde_json::from_str(&updated).expect("snapshot is valid json");

        assert_eq!(
            snapshot["config"][RESUME_THREAD_ID_CONFIG_KEY],
            "thread-123"
        );
        assert_eq!(snapshot["config"]["resume_thread_in_place"], true);
        assert!(snapshot["config"].get("resume_session_id").is_none());
        assert!(snapshot["config"].get("resume_fallback_prompt").is_none());
        assert_eq!(
            snapshot["dispatch"]["execution_policy"],
            "resume_latest_target_role_thread"
        );
        assert_eq!(snapshot["dispatch"]["target_role"], "coder");
        assert_eq!(
            snapshot["dispatch_metadata"]["execution_policy"],
            "resume_latest_target_role_thread"
        );
    }
}
