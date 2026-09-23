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

pub(super) fn executor_snapshot_for_harness_resume(snapshot_json: &str) -> Result<String> {
    let mut snapshot = parse_json_value("executor config snapshot", snapshot_json)?;
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
                Value::String("explicit_harness_session".to_owned()),
            );
        }
        obj.insert(
            "dispatch_metadata".to_owned(),
            json!({ "execution_policy": "explicit_harness_session" }),
        );
    }
    serde_json::to_string(&snapshot).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid executor config snapshot: {error}"))
    })
}

/// Promote the exact session-producing candidate to the front of a fresh
/// route. Resume intent and provider translation remain runtime-only.
pub(super) fn executor_snapshot_with_sticky_candidate(
    fresh_snapshot_json: &str,
    parent_snapshot_json: &str,
    adapter_registry: Option<&executors::HarnessAdapterRegistry>,
) -> Result<String> {
    let parent = parse_json_value("parent executor config snapshot", parent_snapshot_json)?;
    let fresh = parse_json_value("executor config snapshot", fresh_snapshot_json)?;

    // Normalize before keying: legacy snapshots may hold un-normalized
    // configs, and `{}` must key identically to its normalized expansion.
    let candidate_key_of = |value: &Value| -> Option<String> {
        let kind = value
            .get("executor_type")
            .and_then(Value::as_str)?
            .parse::<ExecutorKind>()
            .ok()?;
        let config = value.get("config")?;
        let normalized = normalize_candidate_config(&kind, config, adapter_registry).ok()?;
        Some(executors::candidate_key(&kind, &normalized))
    };

    let Some(parent_key) = candidate_key_of(&parent) else {
        return Err(ServiceError::invalid_operation(
            "cannot resolve exact HarnessSession candidate from parent snapshot",
        ));
    };

    if candidate_key_of(&fresh).as_deref() == Some(parent_key.as_str()) {
        return executor_snapshot_for_harness_resume(fresh_snapshot_json);
    }

    let mut fresh = fresh;
    let route_match = fresh
        .get(executors::ROUTING_SNAPSHOT_KEY)
        .and_then(|routing| routing.get("candidates"))
        .and_then(Value::as_array)
        .and_then(|candidates| {
            candidates.iter().find(|candidate| {
                candidate_key_of(candidate).as_deref() == Some(parent_key.as_str())
            })
        })
        .cloned();

    match route_match {
        Some(candidate) => {
            let candidate_kind = candidate
                .get("executor_type")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "exact HarnessSession route candidate has no executor_type",
                    )
                })?
                .parse::<ExecutorKind>()
                .map_err(ServiceError::invalid_operation)?;
            let raw_candidate_config = candidate.get("config").cloned().unwrap_or_else(|| json!({}));
            let candidate_config = normalize_candidate_config(
                &candidate_kind,
                &raw_candidate_config,
                adapter_registry,
            )
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            let capabilities = effective_harness_capabilities(
                &candidate_kind,
                &candidate_config,
                adapter_registry,
            )
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            let effective_policy = effective_harness_policy(
                &candidate_kind,
                &candidate_config,
                adapter_registry,
            )
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            if let Some(object) = fresh.as_object_mut() {
                object.insert(
                    "executor_type".to_owned(),
                    Value::String(candidate_kind.to_string()),
                );
                object.insert("config".to_owned(), candidate_config);
                object.insert(
                    "harness_capabilities".to_owned(),
                    serde_json::to_value(capabilities).map_err(|error| {
                        ServiceError::invalid_operation(format!(
                            "invalid HarnessCapabilities snapshot: {error}"
                        ))
                    })?,
                );
                if let Some(policy) = effective_policy {
                    object.insert(
                        "effective_execution_policy".to_owned(),
                        serde_json::to_value(policy).map_err(|error| {
                            ServiceError::invalid_operation(format!(
                                "invalid effective execution policy: {error}"
                            ))
                        })?,
                    );
                } else {
                    object.remove("effective_execution_policy");
                }
            }
            let promoted = serde_json::to_string(&fresh).map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "invalid executor config snapshot: {error}"
                ))
            })?;
            executor_snapshot_for_harness_resume(&promoted)
        }
        None => Err(ServiceError::invalid_operation(
            "exact HarnessSession candidate is no longer in the configured route",
        )),
    }
}

pub(super) fn executor_snapshot_for_fresh_start(snapshot_json: &str) -> Result<String> {
    let mut snapshot = parse_json_value("executor config snapshot", snapshot_json)?;
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
    task: &Task,
    agent: &Agent,
    overrides: Option<ExecutionOverrides>,
    adapter_registry: Option<&executors::HarnessAdapterRegistry>,
) -> Result<Option<String>> {
    // Native profiles are hosted by Forge itself and deliberately have no
    // daemon authority.  CLI profiles retain the existing daemon resolution
    // and snapshot provenance.
    let resolved_daemon_id = if agent.backend_kind == "native" {
        None
    } else {
        Some(
            crate::agent_service::resolve_daemon_for_agent(db, agent)
                .await?
                .id,
        )
    };
    let mut base_config = parse_json_value("agent config_json", &agent.config_json)?;
    // Extract before normalization: the typed config round-trip drops
    // unknown fields, which would silently delete the chain.
    let fallbacks = extract_fallbacks(&mut base_config)?;
    apply_agent_fields_to_config(agent, &mut base_config)?;
    let capabilities = parse_json_value("agent capabilities_json", &agent.capabilities_json)?;
    let kind = agent
        .executor_type
        .parse::<ExecutorKind>()
        .map_err(ServiceError::invalid_operation)?;
    let execution_overrides = execution_overrides_to_config_layer(overrides)?;
    let (merged_config, overrides_applied) =
        merge_config_layers(&base_config, &execution_overrides);
    let normalized_config = normalize_candidate_config(&kind, &merged_config, adapter_registry)
        .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
    let overrides_applied = overrides_applied.retain_config_keys(&normalized_config);
    let harness_capabilities = effective_harness_capabilities(&kind, &normalized_config, adapter_registry)
        .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
    let effective_execution_policy =
        effective_harness_policy(&kind, &normalized_config, adapter_registry)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
    let mut snapshot = json!({
        "agent_id": agent.id,
        // Native execution consumes this immutable profile reference from the
        // Task snapshot.  Provider credentials remain behind the protected
        // profile/store boundary and are never copied into public execution
        // snapshot JSON.
        "profile_id": agent.profile_id,
        // This opaque durable reference is not a credential. It allows usage
        // accounting to prove that host-local observations share a logical
        // account without exposing provider secrets.
        "credential_ref": agent.credential_ref,
        "provider": agent.provider,
        "executor_type": agent.executor_type,
        "model": agent.model,
        "prompt_template": agent.prompt_template,
        "reasoning_effort": agent.reasoning_effort,
        "permission_policy": agent.permission_policy,
        "config": normalized_config,
        "capabilities": capabilities,
        "harness_capabilities": harness_capabilities,
        // Keep both the Agent's explicit daemon binding and the daemon chosen
        // for this Execution. The resolved daemon is routing, not Agent
        // identity, but it is factual host provenance for host-local usage.
        "agent_daemon_id": agent.daemon_id,
        "resolved_daemon_id": resolved_daemon_id,
        "overrides_applied": overrides_applied.to_json(),
        "snapshotted_at": now_rfc3339(),
    });
    if let Some(policy) = effective_execution_policy {
        snapshot["effective_execution_policy"] = serde_json::to_value(policy).map_err(|error| {
            ServiceError::invalid_operation(format!("invalid effective execution policy: {error}"))
        })?;
    }
    if let Some(routing) =
        routing_snapshot_value(kind, &snapshot["config"], &fallbacks, adapter_registry)?
    {
        snapshot[executors::ROUTING_SNAPSHOT_KEY] = routing;
    }
    // Charter-backed discovery and planning Tasks may inspect a repository,
    // but they are never allowed to receive a write-capable execution
    // profile.  Persist the capability in the immutable execution snapshot
    // so every executor backend sees the same server-derived restriction.
    if matches!(
        task.task_type.as_str(),
        "planning" | "discovery" | "review" | "validation"
    ) {
        executors::mark_worktree_read_only(&mut snapshot);
    }
    serde_json::to_string(&snapshot)
        .map(Some)
        .map_err(|error| ServiceError::invalid_operation(format!("invalid JSON snapshot: {error}")))
}

/// Remove and return the authored `fallbacks` entries from an agent config.
pub(super) fn extract_fallbacks(config: &mut Value) -> Result<Vec<Value>> {
    let Some(object) = config.as_object_mut() else {
        return Ok(Vec::new());
    };
    match object.remove(executors::FALLBACKS_CONFIG_KEY) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(entries)) => Ok(entries),
        Some(other) => Err(ServiceError::invalid_operation(format!(
            "agent config fallbacks must be a JSON array, got: {other}"
        ))),
    }
}

/// What a routed execution actually did, applied back onto its snapshot for
/// provenance and sticky selection. Built from the local `ExecutionResult`
/// or the remote terminal notification — both structured, never log prose.
#[derive(Debug, Default, Clone)]
pub(crate) struct RouteOutcome {
    /// (candidate_key, executor_type, config, capabilities, effective policy).
    pub selected: Option<(String, String, Value, Value, Option<Value>)>,
    /// (candidate_key, outcome) per attempt, in attempt order.
    pub attempts: Vec<(String, String)>,
    /// RFC3339 retry hint when the whole route was unavailable.
    pub unavailable_retry_at: Option<Option<String>>,
}

/// Fold the actual route winner and its effective capabilities into the
/// execution snapshot. Single-candidate snapshots receive the same evidence.
pub(crate) fn apply_route_outcome_to_snapshot(
    snapshot_json: &str,
    outcome: &RouteOutcome,
) -> Result<Option<String>> {
    let mut snapshot = parse_json_value("executor config snapshot", snapshot_json)?;
    let has_routing = snapshot
        .get(executors::ROUTING_SNAPSHOT_KEY)
        .and_then(Value::as_object)
        .is_some();
    if !has_routing && outcome.selected.is_none() {
        return Ok(None);
    }
    let Some(object) = snapshot.as_object_mut() else {
        return Ok(None);
    };
    if let Some((_, executor_type, config, harness_capabilities, effective_policy)) =
        &outcome.selected
    {
        object.insert(
            "executor_type".to_owned(),
            Value::String(executor_type.clone()),
        );
        object.insert("config".to_owned(), config.clone());
        object.insert("harness_capabilities".to_owned(), harness_capabilities.clone());
        if let Some(effective_policy) = effective_policy {
            object.insert("effective_execution_policy".to_owned(), effective_policy.clone());
        }
    }
    if !has_routing {
        return serde_json::to_string(&snapshot)
            .map(Some)
            .map_err(|error| ServiceError::invalid_operation(format!("invalid executor config snapshot: {error}")));
    }
    let Some(routing) = object
        .get_mut(executors::ROUTING_SNAPSHOT_KEY)
        .and_then(Value::as_object_mut)
    else {
        return Ok(None);
    };
    if let Some((candidate_key, _, _, _, _)) = &outcome.selected {
        routing.insert(
            "selected_candidate_key".to_owned(),
            Value::String(candidate_key.clone()),
        );
    }
    if !outcome.attempts.is_empty() {
        let attempts: Vec<Value> = outcome
            .attempts
            .iter()
            .map(|(candidate_key, outcome)| {
                json!({"candidate_key": candidate_key, "outcome": outcome})
            })
            .collect();
        routing.insert("attempts".to_owned(), Value::Array(attempts));
    }
    if let Some(retry_at) = &outcome.unavailable_retry_at {
        routing.insert(
            "disposition".to_owned(),
            json!({
                "failure_class": "executor_unavailable",
                "retry_at": retry_at,
            }),
        );
    }
    serde_json::to_string(&snapshot).map(Some).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid executor config snapshot: {error}"))
    })
}

/// Build the validated `routing` snapshot block, or `None` when the agent
/// has no fallbacks (legacy snapshots stay byte-identical).
fn routing_snapshot_value(
    kind: ExecutorKind,
    normalized_primary: &Value,
    fallbacks: &[Value],
    adapter_registry: Option<&executors::HarnessAdapterRegistry>,
) -> Result<Option<Value>> {
    if fallbacks.is_empty() {
        return Ok(None);
    }
    if kind == ExecutorKind::Embedded {
        return Err(ServiceError::invalid_operation(
            "embedded executor cannot use CLI fallback routing",
        ));
    }
    let mut candidates = vec![executors::ExecutorCandidate {
        executor_type: kind,
        config: normalized_primary.clone(),
    }];
    for (index, entry) in fallbacks.iter().enumerate() {
        let object = entry.as_object().ok_or_else(|| {
            ServiceError::invalid_operation(format!("fallbacks[{index}] must be a JSON object"))
        })?;
        let executor_type = object
            .get("executor_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ServiceError::invalid_operation(format!("fallbacks[{index}] is missing executor_type"))
            })?;
        let candidate_kind = executor_type.parse::<ExecutorKind>().map_err(|_| {
            ServiceError::invalid_operation(format!("fallbacks[{index}] has unknown executor_type"))
        })?;
        if candidate_kind == ExecutorKind::Embedded {
            return Err(ServiceError::invalid_operation(
                "embedded executor cannot be a CLI fallback candidate",
            ));
        }
        let raw_config = object
            .get("config")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !raw_config.is_object() {
            return Err(ServiceError::invalid_operation(format!(
                "fallbacks[{index}] config must be a JSON object"
            )));
        }
        candidates.push(executors::ExecutorCandidate {
            executor_type: candidate_kind.clone(),
            config: normalize_candidate_config(&candidate_kind, &raw_config, adapter_registry)
                .map_err(|error| ServiceError::invalid_operation(error.to_string()))?,
        });
    }
    let routing = executors::validate_ordered_fallback_routing(candidates)
        .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
    serde_json::to_value(&routing).map(Some).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid routing snapshot: {error}"))
    })
}

fn normalize_candidate_config(
    kind: &ExecutorKind,
    config: &Value,
    adapter_registry: Option<&executors::HarnessAdapterRegistry>,
) -> std::result::Result<Value, executors::ExecutorError> {
    if kind == &ExecutorKind::Embedded {
        return executors::normalize_harness_config::<executors::EmbeddedConfig>(
            kind.clone(),
            config,
            &ExecutionOverrides::default(),
        );
    }
    let normalize = |registry: &executors::HarnessAdapterRegistry| {
        let adapter = registry.get(kind).ok_or_else(|| {
            executors::ExecutorError::Other(format!("No HarnessAdapter registered for {kind}"))
        })?;
        adapter.normalize_config(config, &ExecutionOverrides::default())
    };
    match adapter_registry {
        Some(registry) => normalize(registry),
        None => normalize(&cli_adapters::default_registry()),
    }
}

fn effective_harness_capabilities(
    kind: &ExecutorKind,
    normalized_config: &Value,
    adapter_registry: Option<&executors::HarnessAdapterRegistry>,
) -> std::result::Result<api_types::HarnessCapabilities, executors::ExecutorError> {
    if kind == &ExecutorKind::Embedded {
        // Embedded cognition remains the bounded PR10 Agent Host exception.
        // Keep its capability evidence Unknown instead of inventing harness
        // features for the runtime that PR10 removes.
        return Ok(api_types::HarnessCapabilities::unknown());
    }
    let capabilities_for = |registry: &executors::HarnessAdapterRegistry| {
        registry
            .get(kind)
            .map(|adapter| adapter.capabilities(normalized_config))
            .ok_or_else(|| {
                executors::ExecutorError::Other(format!(
                    "No HarnessAdapter registered for {kind}"
                ))
            })
    };
    match adapter_registry {
        Some(registry) => capabilities_for(registry),
        None => capabilities_for(&cli_adapters::default_registry()),
    }
}

fn effective_harness_policy(
    kind: &ExecutorKind,
    normalized_config: &Value,
    adapter_registry: Option<&executors::HarnessAdapterRegistry>,
) -> std::result::Result<Option<api_types::EffectiveExecutionPolicy>, executors::ExecutorError> {
    if kind == &ExecutorKind::Embedded {
        // Embedded policy interpretation remains bounded Agent Host/PR10 debt.
        return Ok(None);
    }
    let policy_for = |registry: &executors::HarnessAdapterRegistry| {
        registry
            .get(kind)
            .map(|adapter| adapter.effective_execution_policy(normalized_config, None, None))
            .ok_or_else(|| {
                executors::ExecutorError::Other(format!(
                    "No HarnessAdapter registered for {kind}"
                ))
            })
    };
    match adapter_registry {
        Some(registry) => policy_for(registry).map(Some),
        None => policy_for(&cli_adapters::default_registry()).map(Some),
    }
}

pub(super) async fn create_failed_execution_record(
    db: &SqliteDb,
    task_id: &str,
    agent: &Agent,
    workspace: &Workspace,
    execution_id: &str,
    role: &str,
    purpose: ExecutionPurpose,
    error: String,
) -> Result<()> {
    let now = now_rfc3339();
    ExecutionRepo::create(
        db,
        CreateExecution {
            id: execution_id.to_owned(),
            task_id: task_id.to_owned(),
            agent_id: Some(agent.id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent.id.clone())),
            purpose: Some(purpose),
            harness_session_id: None,
            role: role.to_owned(),
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
    fn extract_fallbacks_removes_key_and_returns_entries() {
        let mut config = serde_json::json!({
            "profile": "acct-1",
            "fallbacks": [
                {"executor_type": "smith", "config": {"profile": "acct-2"}}
            ]
        });
        let fallbacks = extract_fallbacks(&mut config).expect("fallbacks extract");
        assert_eq!(fallbacks.len(), 1);
        assert!(config.get("fallbacks").is_none());
        assert_eq!(config["profile"], "acct-1");

        let mut without = serde_json::json!({"profile": "acct-1"});
        assert!(extract_fallbacks(&mut without)
            .expect("no fallbacks is fine")
            .is_empty());

        let mut invalid = serde_json::json!({"fallbacks": "acct-2"});
        assert!(extract_fallbacks(&mut invalid).is_err());
    }

    #[test]
    fn routing_snapshot_value_builds_only_with_fallbacks() {
        let primary = normalize_candidate_config(
            &ExecutorKind::Smith,
            &serde_json::json!({"profile": "acct-1"}),
            None,
        )
        .expect("primary normalizes");

        assert!(routing_snapshot_value(ExecutorKind::Smith, &primary, &[], None)
            .expect("legacy path succeeds")
            .is_none());

        let routing = routing_snapshot_value(
            ExecutorKind::Smith,
            &primary,
            &[serde_json::json!({"executor_type": "claude_code", "config": {}})],
            None,
        )
        .expect("routing builds")
        .expect("routing present");
        assert_eq!(routing["policy"], "ordered_fallback_v1");
        assert_eq!(routing["candidates"].as_array().unwrap().len(), 2);
        assert_eq!(routing["candidates"][1]["executor_type"], "claude_code");
    }

    fn smith_snapshot(profile: &str, with_routing: bool) -> String {
        let config = normalize_candidate_config(
            &ExecutorKind::Smith,
            &serde_json::json!({"profile": profile}),
            None,
        )
        .expect("config resolves");
        let registry = cli_adapters::default_registry();
        let capabilities = effective_harness_capabilities(
            &ExecutorKind::Smith,
            &config,
            Some(&registry),
        )
        .expect("capabilities resolve");
        let effective_policy = effective_harness_policy(
            &ExecutorKind::Smith,
            &config,
            Some(&registry),
        )
        .expect("policy resolves");
        let mut snapshot = serde_json::json!({
            "executor_type": "smith",
            "config": config,
            "harness_capabilities": capabilities,
            "effective_execution_policy": effective_policy,
        });
        if with_routing {
            let routing = routing_snapshot_value(
                ExecutorKind::Smith,
                &snapshot["config"],
                &[serde_json::json!({"executor_type": "smith", "config": {"profile": "acct-2"}})],
                None,
            )
            .expect("routing builds")
            .expect("routing present");
            snapshot["routing"] = routing;
        }
        snapshot.to_string()
    }

    #[test]
    fn sticky_resume_matches_parent_winner_at_top_level() {
        let fresh = smith_snapshot("acct-1", true);
        let parent = smith_snapshot("acct-1", true);

        let resumed = executor_snapshot_with_sticky_candidate(&fresh, &parent, None)
            .expect("sticky resume succeeds");
        let snapshot: Value = serde_json::from_str(&resumed).unwrap();
        assert_eq!(snapshot["dispatch_metadata"]["execution_policy"], "explicit_harness_session");
        assert_eq!(snapshot["config"]["profile"], "acct-1");
    }

    #[test]
    fn sticky_resume_promotes_parent_winner_from_route() {
        let fresh = smith_snapshot("acct-1", true);
        // Parent ran on the fallback candidate acct-2 (its winner was
        // persisted at top level).
        let parent = smith_snapshot("acct-2", false);

        let resumed = executor_snapshot_with_sticky_candidate(&fresh, &parent, None)
            .expect("sticky resume succeeds");
        let snapshot: Value = serde_json::from_str(&resumed).unwrap();
        assert_eq!(snapshot["config"]["profile"], "acct-2");
        assert_eq!(snapshot["dispatch_metadata"]["execution_policy"], "explicit_harness_session");
        // The full route stays available for fallback.
        assert_eq!(
            snapshot["routing"]["candidates"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn sticky_resume_promotes_cross_harness_capabilities_and_policy() {
        let registry = cli_adapters::default_registry();
        let mut fresh: Value = serde_json::from_str(&smith_snapshot("acct-1", false)).unwrap();
        let fallback = normalize_candidate_config(
            &ExecutorKind::ClaudeCode,
            &json!({}),
            Some(&registry),
        )
        .expect("fallback config normalizes");
        let routing = routing_snapshot_value(
            ExecutorKind::Smith,
            &fresh["config"],
            &[json!({
                "executor_type": "claude_code",
                "config": fallback,
            })],
            Some(&registry),
        )
        .expect("route validates")
        .expect("fallback route exists");
        fresh[executors::ROUTING_SNAPSHOT_KEY] = routing;
        let parent = json!({
            "executor_type": "claude_code",
            "config": normalize_candidate_config(
                &ExecutorKind::ClaudeCode,
                &json!({}),
                Some(&registry),
            )
            .expect("parent config normalizes"),
        })
        .to_string();

        let resumed = executor_snapshot_with_sticky_candidate(
            &fresh.to_string(),
            &parent,
            Some(&registry),
        )
        .expect("cross-harness sticky resume succeeds");
        let resumed: Value = serde_json::from_str(&resumed).unwrap();
        let claude = registry
            .get(&ExecutorKind::ClaudeCode)
            .expect("Claude Code adapter registered");
        let expected_capabilities = claude.capabilities(&resumed["config"]);
        let expected_policy =
            claude.effective_execution_policy(&resumed["config"], None, None);

        assert_eq!(resumed["executor_type"], "claude_code");
        assert_eq!(
            resumed["harness_capabilities"],
            serde_json::to_value(expected_capabilities).unwrap()
        );
        assert_eq!(
            resumed["effective_execution_policy"],
            serde_json::to_value(expected_policy).unwrap()
        );
        assert_eq!(resumed["harness_capabilities"]["planning"], "native");
        assert_eq!(resumed["dispatch_metadata"]["execution_policy"], "explicit_harness_session");
    }

    #[test]
    fn sticky_resume_fails_when_exact_candidate_left_route() {
        let fresh = smith_snapshot("acct-1", true);
        let parent = smith_snapshot("acct-9", false);

        assert!(executor_snapshot_with_sticky_candidate(&fresh, &parent, None).is_err());
    }

    #[test]
    fn sticky_resume_without_routing_requires_exact_candidate() {
        let fresh = smith_snapshot("acct-1", false);
        let matching_parent = smith_snapshot("acct-1", false);
        let other_parent = smith_snapshot("acct-2", false);

        let resumed = executor_snapshot_with_sticky_candidate(&fresh, &matching_parent, None)
            .expect("sticky resume succeeds");
        let snapshot: Value = serde_json::from_str(&resumed).unwrap();
        assert_eq!(snapshot["dispatch_metadata"]["execution_policy"], "explicit_harness_session");

        assert!(executor_snapshot_with_sticky_candidate(&fresh, &other_parent, None).is_err());
    }

    #[test]
    fn initial_harness_policy_comes_from_the_registered_adapter() {
        let registry = cli_adapters::default_registry();
        let policy = effective_harness_policy(
            &ExecutorKind::Codex,
            &json!({"sandbox": "danger-full-access"}),
            Some(&registry),
        )
        .expect("adapter interprets policy")
        .expect("external harness policy is captured");
        assert_eq!(policy.isolation_posture, "danger-full-access");
        assert!(policy.is_high_risk);

        assert!(effective_harness_policy(
            &ExecutorKind::Embedded,
            &json!({}),
            Some(&registry),
        )
        .expect("embedded remains on the legacy path")
        .is_none());
    }

    #[test]
    fn apply_route_outcome_records_winner_attempts_and_disposition() {
        let snapshot = smith_snapshot("acct-1", true);
        let outcome = RouteOutcome {
            selected: Some((
                "smith:profile=acct-2#test".to_owned(),
                "smith".to_owned(),
                serde_json::json!({"profile": "acct-2"}),
                serde_json::json!({"resume": "native"}),
                Some(serde_json::json!({"isolation_posture": "not_applicable"})),
            )),
            attempts: vec![
                (
                    "smith:profile=acct-1#test".to_owned(),
                    "usage_exhausted".to_owned(),
                ),
                (
                    "smith:profile=acct-2#test".to_owned(),
                    "completed".to_owned(),
                ),
            ],
            unavailable_retry_at: None,
        };

        let updated = apply_route_outcome_to_snapshot(&snapshot, &outcome)
            .expect("outcome applies")
            .expect("routed snapshot updates");
        let value: Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(value["config"]["profile"], "acct-2");
        assert_eq!(
            value["routing"]["selected_candidate_key"],
            "smith:profile=acct-2#test"
        );
        assert_eq!(
            value["routing"]["attempts"][0]["outcome"],
            "usage_exhausted"
        );
        // Provenance: the configured route is retained.
        assert_eq!(value["routing"]["candidates"].as_array().unwrap().len(), 2);

        // Single-candidate snapshots also record the actual invocation winner.
        let legacy = smith_snapshot("acct-1", false);
        let updated = apply_route_outcome_to_snapshot(&legacy, &outcome)
            .expect("legacy path succeeds")
            .expect("winner capability snapshot is added");
        let updated: Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(updated["harness_capabilities"]["resume"], "native");
    }

    #[test]
    fn executor_snapshot_for_harness_resume_keeps_resume_intent_generic() {
        let snapshot_json = r#"{"executor_type":"codex","dispatch":{"execution_policy":"new_execution","target_role":"coder"},"config":{"model":"gpt-5-codex","resume_fallback_prompt":"full prompt should not be reused"}}"#;

        let updated = executor_snapshot_for_harness_resume(snapshot_json)
            .expect("snapshot updates");
        let snapshot: Value = serde_json::from_str(&updated).expect("snapshot is valid json");

        assert_eq!(snapshot["config"]["resume_fallback_prompt"], "full prompt should not be reused");
        assert_eq!(snapshot["dispatch"]["execution_policy"], "explicit_harness_session");
        assert_eq!(snapshot["dispatch"]["target_role"], "coder");
        assert_eq!(
            snapshot["dispatch_metadata"]["execution_policy"],
            "explicit_harness_session"
        );
    }

    #[test]
    fn fresh_start_removes_dispatch_marker_but_adapter_owns_legacy_resume_keys() {
        let snapshot_json = r#"{
            "executor_type":"cursor",
            "dispatch":{"execution_policy":"resume_latest_target_role_thread","target_role":"reviewer"},
            "dispatch_metadata":{"execution_policy":"resume_latest_target_role_thread"},
            "config":{"resume_session_id":"top","fallbacks":[
                {"config":{"resume_thread_id":"nested","resume_thread_in_place":true}},
                {"config":{"resume_session_id":"fallback","resume_fallback_prompt":"old"}}
            ]}
        }"#;

        let cleaned = executor_snapshot_for_fresh_start(snapshot_json)
            .expect("reviewer snapshot is valid");
        let snapshot: Value = serde_json::from_str(&cleaned).expect("cleaned snapshot is json");
        let rendered = snapshot.to_string();

        assert!(rendered.contains("resume_session_id"));
        assert!(rendered.contains("resume_thread_id"));
        assert!(snapshot.get("dispatch_metadata").is_none());
        assert!(snapshot["dispatch"].get("execution_policy").is_none());
        assert_eq!(snapshot["dispatch"]["target_role"], "reviewer");
    }
}
