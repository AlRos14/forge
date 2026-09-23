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

/// Runtime writers choose purpose from the semantic operation. This helper is
/// intentionally only a small compatibility mapping for existing role-driven
/// dispatch paths; callers with a stronger semantic signal pass the purpose
/// directly.
pub(crate) fn execution_purpose_for_role(role: &str) -> ExecutionPurpose {
    match role.trim().to_ascii_lowercase().as_str() {
        "planner" => ExecutionPurpose::Plan,
        "reviewer" | "auditor" => ExecutionPurpose::Review,
        "coder" | "worker" | "implementer" | "executor" | "merge_fixer" => {
            ExecutionPurpose::Implement
        }
        "orchestrator" => ExecutionPurpose::Orchestrate,
        "interactive" | "system" => ExecutionPurpose::General,
        _ => ExecutionPurpose::General,
    }
}

/// Resolve the purpose at a semantic task-dispatch boundary. Role remains the
/// fallback for the legacy role-driven implementation path, but task types
/// with a stronger domain meaning win so validation is not recorded as
/// implementation merely because its workflow role is `worker`.
pub(crate) fn execution_purpose_for_task_type(
    task_type: &str,
    role: &str,
) -> ExecutionPurpose {
    match task_type.trim().to_ascii_lowercase().as_str() {
        "planning" => ExecutionPurpose::Plan,
        "review" => ExecutionPurpose::Review,
        "validation" => ExecutionPurpose::Validate,
        "discovery" | "investigation" | "investigate" => ExecutionPurpose::Investigate,
        _ => execution_purpose_for_role(role),
    }
}

/// Resolve explicit generic continuity. A referenced HarnessSession is the
/// authority for new rows; the legacy execution.agent_session_id fallback is
/// only available to historical rows that have not yet been materialized.
pub async fn resumable_external_session(
    db: &SqliteDb,
    execution: &Execution,
    expected_agent_id: Option<&str>,
    workspace_id: Option<&str>,
) -> Result<Option<String>> {
    if ExecutionRepo::has_historical_session_ambiguity(db, &execution.id).await? {
        // Contradictory historical evidence is never resumable, even if a
        // generic reference was attached by an earlier/incomplete rollout.
        // Keep this guard in the common authority path so all consumers fail
        // closed consistently.
        return Ok(None);
    }

    if let Some(harness_session_id) = execution.harness_session_id.as_deref() {
        let Some(session) = HarnessSessionRepo::get_by_id(db, harness_session_id).await? else {
            return Ok(None);
        };
        let Some(agent) = AgentRepo::get_by_id(db, &session.agent_id).await? else {
            return Ok(None);
        };
        if !harness_session_identity_matches_snapshot(
            &agent.executor_type,
            &agent.config_json,
            agent.credential_ref.as_deref(),
            &session.harness_kind,
            &session.profile_snapshot_json,
        ) || !harness_identity_matches_snapshot(
            &agent.executor_type,
            &agent.config_json,
            agent.credential_ref.as_deref(),
            execution.executor_config_snapshot_json.as_deref(),
        ) {
            // Historical PR2 fallbacks could have written a session under an
            // Agent whose primary harness/account identity did not match the
            // actual candidate. Keep that history, but do not advertise or
            // resume it as continuity for the current Agent identity.
            return Ok(None);
        }
        let execution_harness_kind = execution
            .executor_config_snapshot_json
            .as_deref()
            .and_then(|snapshot| serde_json::from_str::<Value>(snapshot).ok())
            .and_then(|snapshot| {
                snapshot
                    .get("executor_type")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            });
        if !matches!(&session.status, HarnessSessionStatus::Active)
            || !session
                .external_session_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || !matches!(
                execution.actor_ref(),
                Some(db::ActorRef::Agent(ref agent_id)) if agent_id == &session.agent_id
            )
            || expected_agent_id.is_some_and(|agent_id| session.agent_id != agent_id)
            || execution_harness_kind
                .as_deref()
                .is_some_and(|harness_kind| harness_kind != session.harness_kind)
            || (session.workspace_id.is_some() && session.workspace_id.as_deref() != workspace_id)
            || execution
                .agent_session_id
                .as_deref()
                .is_some_and(|legacy_id| Some(legacy_id) != session.external_session_id.as_deref())
        {
            return Ok(None);
        }
        if !harness_session_resume_is_available(&session) {
            return Ok(None);
        }
        return Ok(session.external_session_id);
    }

    // Bounded PR13 cleanup fallback: old rows have no generic reference, so
    // their legacy external identity is usable only when the persisted Agent
    // still matches the caller. Never use this path when a generic reference
    // exists, even if the compatibility projection is populated.
    let Some(db::ActorRef::Agent(actor_id)) = execution.actor_ref() else {
        return Ok(None);
    };
    if execution.agent_id.as_deref() == Some(actor_id.as_str())
        && !actor_id.eq_ignore_ascii_case("human")
        && execution
            .agent_session_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && (execution.workspace_id.is_none()
            || execution.workspace_id.as_deref() == workspace_id)
        && expected_agent_id.is_none_or(|agent_id| actor_id == agent_id)
    {
        let Some(snapshot_value) = execution
            .executor_config_snapshot_json
            .as_deref()
            .and_then(|snapshot| serde_json::from_str::<Value>(snapshot).ok())
        else {
            return Ok(None);
        };
        let Some(snapshot_kind) = snapshot_value
            .get("executor_type")
            .and_then(Value::as_str)
            .filter(|kind| is_known_pr2_resume_harness(kind))
        else {
            return Ok(None);
        };
        if snapshot_value
            .get("agent_id")
            .and_then(Value::as_str)
            .is_some_and(|snapshot_agent_id| snapshot_agent_id != actor_id)
        {
            return Ok(None);
        }
        let Some(agent) = AgentRepo::get_by_id(db, &actor_id).await? else {
            return Ok(None);
        };
        if !agent.executor_type.eq_ignore_ascii_case(snapshot_kind)
            || !harness_identity_matches_value(
                &agent.executor_type,
                &agent.config_json,
                agent.credential_ref.as_deref(),
                &snapshot_value,
            )
        {
            return Ok(None);
        }
        return Ok(execution.agent_session_id.clone());
    }
    Ok(None)
}

fn harness_session_identity_matches_snapshot(
    agent_harness_kind: &str,
    agent_config_json: &str,
    agent_credential_ref: Option<&str>,
    session_harness_kind: &str,
    profile_snapshot_json: &str,
) -> bool {
    let (Ok(agent_kind), Ok(session_kind), Ok(snapshot)) = (
        agent_harness_kind.parse::<executors::ExecutorKind>(),
        session_harness_kind.parse::<executors::ExecutorKind>(),
        serde_json::from_str::<Value>(profile_snapshot_json),
    ) else {
        return false;
    };
    let Some(snapshot_kind) = snapshot
        .get("executor_type")
        .and_then(Value::as_str)
        .and_then(|kind| kind.parse::<executors::ExecutorKind>().ok())
    else {
        return false;
    };
    if agent_kind != session_kind || snapshot_kind != session_kind {
        return false;
    }
    harness_identity_matches_value(
        agent_harness_kind,
        agent_config_json,
        agent_credential_ref,
        &snapshot,
    )
}

fn harness_identity_matches_snapshot(
    agent_harness_kind: &str,
    agent_config_json: &str,
    agent_credential_ref: Option<&str>,
    snapshot_json: Option<&str>,
) -> bool {
    let Some(snapshot) = snapshot_json
        .and_then(|snapshot| serde_json::from_str::<Value>(snapshot).ok())
    else {
        return false;
    };
    harness_identity_matches_value(
        agent_harness_kind,
        agent_config_json,
        agent_credential_ref,
        &snapshot,
    )
}

fn harness_identity_matches_value(
    agent_harness_kind: &str,
    agent_config_json: &str,
    agent_credential_ref: Option<&str>,
    snapshot: &Value,
) -> bool {
    let (Ok(agent_kind), Ok(agent_config)) = (
        agent_harness_kind.parse::<executors::ExecutorKind>(),
        serde_json::from_str::<Value>(agent_config_json),
    ) else {
        return false;
    };
    let Some(snapshot_kind) = snapshot
        .get("executor_type")
        .and_then(Value::as_str)
        .and_then(|kind| kind.parse::<executors::ExecutorKind>().ok())
    else {
        return false;
    };
    let Some(snapshot_config) = snapshot.get("config") else {
        return false;
    };
    if agent_kind != snapshot_kind {
        return false;
    }
    let current_credential_ref = agent_credential_ref
        .map(str::trim)
        .filter(|reference| !reference.is_empty());
    let historical_credential_ref = snapshot
        .get("credential_ref")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reference| !reference.is_empty());
    current_credential_ref == historical_credential_ref
        && executors::validate_same_agent_candidate(
            &agent_kind,
            &agent_config,
            &snapshot_kind,
            snapshot_config,
        )
        .is_ok()
}

/// Build the runtime-only invocation from the durable HarnessSession link.
/// Absence means Start. A present but unusable link is an explicit failure and
/// can never be rewritten into a fresh run.
pub async fn harness_invocation_for_execution(
    db: &SqliteDb,
    execution: &Execution,
    workspace_id: Option<&str>,
) -> Result<api_types::HarnessInvocation> {
    if execution.harness_session_id.is_none() {
        return Ok(api_types::HarnessInvocation::Start);
    }
    let external_session_id = resumable_external_session(
        db,
        execution,
        execution.agent_id.as_deref(),
        workspace_id,
    )
    .await?
    .ok_or_else(|| {
        ServiceError::invalid_operation(
            "explicit HarnessSession is not resumable under its historical capability evidence",
        )
    })?;
    Ok(api_types::HarnessInvocation::Resume {
        external_session_id,
    })
}

/// Validate a parent-provided HarnessSession for child attachment. Actor
/// selection happens before this function; role/task/model similarity is not a
/// continuity condition.
pub(crate) async fn reusable_harness_session_for_agent(
    db: &SqliteDb,
    execution: &Execution,
    agent_id: &str,
    workspace_id: Option<&str>,
) -> Result<Option<HarnessSession>> {
    let Some(harness_session_id) = execution.harness_session_id.as_deref() else {
        return Ok(None);
    };
    let Some(session) = HarnessSessionRepo::get_by_id(db, harness_session_id).await? else {
        return Ok(None);
    };
    let Some(agent) = AgentRepo::get_by_id(db, agent_id).await? else {
        return Ok(None);
    };
    let execution_harness_kind = execution
        .executor_config_snapshot_json
        .as_deref()
        .and_then(|snapshot| serde_json::from_str::<Value>(snapshot).ok())
        .and_then(|snapshot| {
            snapshot
                .get("executor_type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        });
    if session.agent_id != agent_id
        || !matches!(
            execution.actor_ref(),
            Some(db::ActorRef::Agent(ref actor_agent_id)) if actor_agent_id == agent_id
        )
        || !matches!(&session.status, HarnessSessionStatus::Active)
        || !session
            .external_session_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || execution_harness_kind
            .as_deref()
            .is_some_and(|harness_kind| harness_kind != session.harness_kind)
        || !harness_session_identity_matches_snapshot(
            &agent.executor_type,
            &agent.config_json,
            agent.credential_ref.as_deref(),
            &session.harness_kind,
            &session.profile_snapshot_json,
        )
        || !harness_identity_matches_snapshot(
            &agent.executor_type,
            &agent.config_json,
            agent.credential_ref.as_deref(),
            execution.executor_config_snapshot_json.as_deref(),
        )
        || (session.workspace_id.is_some() && session.workspace_id.as_deref() != workspace_id)
        || execution
            .agent_session_id
            .as_deref()
            .is_some_and(|legacy_id| {
                session.external_session_id.as_deref() != Some(legacy_id)
            })
    {
        return Ok(None);
    }
    if !harness_session_resume_is_available(&session) {
        return Ok(None);
    }
    Ok(Some(session))
}

/// PR2 wrote Agent tag JSON into this column. New snapshots use the typed
/// HarnessCapabilities shape; that evidence wins. For untyped PR2 history,
/// only adapter kinds whose concrete PR2 integration explicitly implemented
/// resume receive this bounded compatibility allowance. PR13 removes it.
fn harness_session_resume_is_available(session: &HarnessSession) -> bool {
    let Ok(snapshot) = serde_json::from_str::<Value>(&session.capabilities_snapshot_json) else {
        return false;
    };
    match api_types::HarnessCapabilitiesSnapshot::from_value(&snapshot) {
        api_types::HarnessCapabilitiesSnapshotRead::VersionedV1(capabilities) => {
            return capabilities.resume.is_available();
        }
        api_types::HarnessCapabilitiesSnapshotRead::UnsupportedVersion(_)
        | api_types::HarnessCapabilitiesSnapshotRead::Malformed => return false,
        api_types::HarnessCapabilitiesSnapshotRead::Unversioned => {}
    }
    // Bounded PR2 compatibility: only its exact tag-array/empty-object shapes
    // and integrations with an implemented exact resume protocol are trusted.
    // Partial capability-looking objects and malformed/future versioned data
    // never enter this path. PR13 removes this allowlist.
    let legacy_shape = snapshot.as_array().is_some_and(|tags| {
        tags.iter().all(Value::is_string)
    }) || snapshot
        .as_object()
        .is_some_and(serde_json::Map::is_empty);
    if !legacy_shape {
        return false;
    }
    is_known_pr2_resume_harness(&session.harness_kind)
}

fn is_known_pr2_resume_harness(harness_kind: &str) -> bool {
    matches!(
        harness_kind,
        "codex" | "claude_code" | "cursor" | "opencode" | "smith"
    )
}

/// Reconcile a pre-PR2 Execution's exact legacy external identity into the
/// generic authority before a new resume child is created. This is only for
/// historical rows that were backfilled to an Agent ActorRef; an unresolved
/// agentless row must fail closed instead of minting continuity.
pub(crate) async fn materialize_historical_harness_session(
    db: &SqliteDb,
    execution: &Execution,
    external_session_id: &str,
) -> Result<Option<Execution>> {
    if execution.harness_session_id.is_some() {
        return Ok(Some(execution.clone()));
    }
    let Some(db::ActorRef::Agent(actor_id)) = execution.actor_ref() else {
        return Ok(None);
    };
    if execution.agent_id.as_deref() != Some(actor_id.as_str()) {
        return Ok(None);
    }
    let reconciled = ExecutionRepo::record_harness_session_result(
        db,
        &execution.id,
        external_session_id,
        &db::now_rfc3339(),
    )
    .await?;
    if reconciled.harness_session_id.is_some() {
        Ok(Some(reconciled))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod purpose_tests {
    use super::*;

    #[test]
    fn historical_session_identity_allows_run_config_but_rejects_harness_account_and_credential_changes() {
        let current_config = r#"{"model":"gpt-5","env":{"CODEX_HOME":"/accounts/one"}}"#;
        let same_identity_new_model = r#"{"executor_type":"codex","credential_ref":"cred-one","config":{"model":"gpt-5.1","model_reasoning_effort":"high","env":{"CODEX_HOME":"/accounts/one"}}}"#;
        assert!(harness_session_identity_matches_snapshot(
            "codex",
            current_config,
            Some("cred-one"),
            "codex",
            same_identity_new_model,
        ));

        let other_harness = r#"{"executor_type":"cursor","credential_ref":"cred-one","config":{"model":"gpt-5.1"}}"#;
        assert!(!harness_session_identity_matches_snapshot(
            "codex",
            current_config,
            Some("cred-one"),
            "cursor",
            other_harness,
        ));

        let other_account = r#"{"executor_type":"codex","credential_ref":"cred-one","config":{"model":"gpt-5","env":{"CODEX_HOME":"/accounts/two"}}}"#;
        assert!(!harness_session_identity_matches_snapshot(
            "codex",
            current_config,
            Some("cred-one"),
            "codex",
            other_account,
        ));

        assert!(!harness_session_identity_matches_snapshot(
            "codex",
            current_config,
            Some("cred-two"),
            "codex",
            same_identity_new_model,
        ));
        assert!(!harness_session_identity_matches_snapshot(
            "codex",
            current_config,
            Some("cred-one"),
            "codex",
            r#"{"executor_type":"codex","config":{"model":"gpt-5"}}"#,
        ));
        assert!(!harness_session_identity_matches_snapshot(
            "codex",
            current_config,
            Some("cred-one"),
            "codex",
            "{malformed",
        ));
    }

    #[test]
    fn role_mapping_is_only_a_compatibility_default() {
        assert_eq!(
            execution_purpose_for_role("planner"),
            ExecutionPurpose::Plan
        );
        assert_eq!(
            execution_purpose_for_role("reviewer"),
            ExecutionPurpose::Review
        );
        assert_eq!(
            execution_purpose_for_role("implementer"),
            ExecutionPurpose::Implement
        );
        assert_eq!(
            execution_purpose_for_role("orchestrator"),
            ExecutionPurpose::Orchestrate
        );
        assert_eq!(
            execution_purpose_for_role("custom"),
            ExecutionPurpose::General
        );
        assert_eq!(
            execution_purpose_for_role("interactive"),
            ExecutionPurpose::General
        );
    }

    #[test]
    fn task_dispatch_mapping_keeps_domain_purpose_independent_from_role() {
        assert_eq!(
            execution_purpose_for_task_type("validation", "worker"),
            ExecutionPurpose::Validate
        );
        assert_eq!(
            execution_purpose_for_task_type("investigation", "implementer"),
            ExecutionPurpose::Investigate
        );
        assert_eq!(
            execution_purpose_for_task_type("implementation", "implementer"),
            ExecutionPurpose::Implement
        );
    }
}

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

pub(super) fn spawn_account_usage_probe(
    db: Arc<SqliteDb>,
    snapshot: Option<String>,
    execution_id: String,
    executor: Arc<dyn executors::TaskExecutor>,
) -> Option<CursorUsageProbe> {
    let snapshot = snapshot?;
    let value = serde_json::from_str::<Value>(&snapshot).ok()?;
    let kind = value
        .get("executor_type")
        .and_then(Value::as_str)?
        .parse::<executors::ExecutorKind>()
        .ok()?;
    // PR0A's periodic account quota poll is Cursor-specific. Other adapters
    // can expose an explicit observation API without changing that policy.
    if kind != executors::ExecutorKind::Cursor {
        return None;
    }
    let cancel = tokio_util::sync::CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        loop {
            if task_cancel.is_cancelled() {
                break;
            }
            persist_account_usage_probe(
                &db,
                &snapshot,
                kind.clone(),
                value.get("config").unwrap_or(&Value::Null).clone(),
                Arc::clone(&executor),
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

async fn persist_account_usage_probe(
    db: &SqliteDb,
    snapshot: &str,
    kind: executors::ExecutorKind,
    config: Value,
    executor: Arc<dyn executors::TaskExecutor>,
    execution_id: &str,
    cancel: tokio_util::sync::CancellationToken,
) {
    match executor.observe_usage(kind, &config, cancel.clone()).await {
        Ok(Some(observation)) => {
            if let Err(error) = persist_account_usage_snapshot_with_source(
                db,
                Some(snapshot),
                execution_id,
                &observation.value,
                observation.source.as_deref().unwrap_or("harness_account_usage"),
            )
            .await
            {
                tracing::warn!(
                    execution_id = %execution_id,
                    %error,
                    "failed to persist harness account usage snapshot"
                );
            }
        }
        Err(error) if !cancel.is_cancelled() => {
            tracing::warn!(
                execution_id = %execution_id,
                %error,
                "failed to observe harness account usage"
            );
        }
        Ok(None) | Err(_) => {}
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
        snapshot_usage_account_key, harness_session_resume_is_available,
    };
    use db::{HarnessSession, HarnessSessionStatus};
    use executors::{LogEntry, LogKind, LogStream};
    use serde_json::json;

    fn harness_session(harness_kind: &str, capabilities: serde_json::Value) -> HarnessSession {
        HarnessSession {
            id: "session-row".to_owned(),
            agent_id: "agent-1".to_owned(),
            harness_kind: harness_kind.to_owned(),
            external_session_id: Some("external-session".to_owned()),
            profile_id: None,
            profile_snapshot_json: "{}".to_owned(),
            capabilities_snapshot_json: capabilities.to_string(),
            workspace_id: Some("workspace-1".to_owned()),
            status: HarnessSessionStatus::Active,
            predecessor_session_id: None,
            created_at: "2026-09-23T00:00:00Z".to_owned(),
            updated_at: "2026-09-23T00:00:00Z".to_owned(),
            last_activity_at: None,
        }
    }

    #[test]
    fn historical_resume_support_is_typed_or_narrow_pr2_compatibility() {
        let mut typed = api_types::HarnessCapabilities::unknown();
        typed.resume = api_types::CapabilitySupport::Native;
        assert!(harness_session_resume_is_available(&harness_session(
            "custom_harness",
            serde_json::to_value(typed.snapshot()).unwrap(),
        )));

        assert!(!harness_session_resume_is_available(&harness_session(
            "custom_harness",
            serde_json::to_value(api_types::HarnessCapabilities::unknown().snapshot()).unwrap(),
        )));
        assert!(!harness_session_resume_is_available(&harness_session(
            "codex",
            json!({"resume":"unsupported"}),
        )));
        assert!(!harness_session_resume_is_available(&harness_session(
            "codex",
            json!({"resume":"native"}),
        )));
        assert!(!harness_session_resume_is_available(&harness_session(
            "codex",
            json!(["legacy-tag", 7]),
        )));
        assert!(!harness_session_resume_is_available(&harness_session(
            "codex",
            json!({"schema_version":"one", "capabilities":{"resume":"native"}}),
        )));
        assert!(!harness_session_resume_is_available(&harness_session(
            "codex",
            json!({"schema_version":1, "capabilities":{"resume":"future_native"}}),
        )));
        assert!(!harness_session_resume_is_available(&harness_session(
            "gemini",
            json!(["legacy-agent-tag"]),
        )));
        assert!(harness_session_resume_is_available(&harness_session(
            "codex",
            json!(["legacy-agent-tag"]),
        )));
        assert!(harness_session_resume_is_available(&harness_session("codex", json!({}))));
        assert!(!harness_session_resume_is_available(&harness_session(
            "codex",
            json!({"schema_version": 2, "capabilities": {"resume":"native"}}),
        )));
    }

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
