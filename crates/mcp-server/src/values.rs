use api_types::parse_project_hooks_json;
use db::{Agent, AgentProfile, Execution, Page, Project};
use serde_json::{json, Value};

pub(crate) fn execution_page_value(page: Page<Execution>) -> Value {
    let has_more = page.next_cursor.is_some();
    json!({
        "items": page.items.into_iter().map(execution_value).collect::<Vec<_>>(),
        "next_cursor": page.next_cursor,
        "has_more": has_more,
        "total_count": page.total_count,
    })
}

pub(crate) fn agent_page_value(page: Page<Agent>) -> Value {
    let has_more = page.next_cursor.is_some();
    json!({
        "items": page.items.into_iter().map(agent_value).collect::<Vec<_>>(),
        "next_cursor": page.next_cursor,
        "has_more": has_more,
        "total_count": page.total_count,
    })
}

pub(crate) fn project_page_value(page: Page<Project>) -> Result<Value, crate::error::McpToolError> {
    let has_more = page.next_cursor.is_some();
    let items = page
        .items
        .into_iter()
        .map(project_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "items": items,
        "next_cursor": page.next_cursor,
        "has_more": has_more,
        "total_count": page.total_count,
    }))
}

pub(crate) fn agent_value(agent: Agent) -> Value {
    json!({
        "id": agent.id,
        "name": agent.name,
        "description": agent.description,
        "profile_id": agent.profile_id,
        "executor_type": agent.executor_type,
        "provider": agent.provider,
        "model": agent.model,
        "reasoning_effort": agent.reasoning_effort,
        "permission_policy": agent.permission_policy,
        "capabilities": safe_json(&agent.capabilities_json),
        "config_json": safe_json(&agent.config_json),
        "credential_handle_id": agent.credential_ref,
        "daemon_id": agent.daemon_id,
        "max_concurrent_tasks": agent.max_concurrent_tasks,
        "heartbeat_interval_seconds": agent.heartbeat_interval_seconds,
        "max_missed_heartbeats": agent.max_missed_heartbeats,
        "status": agent.status.to_string(),
        "last_heartbeat_at": agent.last_heartbeat_at,
        "is_default": agent.is_default,
        "version": agent.version,
        "created_at": agent.created_at,
        "updated_at": agent.updated_at,
    })
}

pub(crate) fn project_value(project: Project) -> Result<Value, crate::error::McpToolError> {
    let paused = project.paused_at.is_some();
    let project_hooks = parse_project_hooks_json(&project.project_hooks_json)
        .map_err(|error| crate::error::McpToolError::new(-32603, error))?;
    Ok(json!({
        "id": project.id,
        "name": project.name,
        "project_hooks": project_hooks,
        "paused_at": project.paused_at,
        "paused": paused,
        "created_at": project.created_at,
        "updated_at": project.updated_at,
    }))
}

pub(crate) fn agent_profile_value(profile: AgentProfile) -> Value {
    json!({
        "id": profile.id,
        "identity_id": profile.identity_id,
        "executor_type": profile.executor_type,
        "provider": profile.provider,
        "model": profile.model,
        "reasoning_effort": profile.reasoning_effort,
        "permission_policy": profile.permission_policy,
        "system_prompt": profile.prompt_template,
        "capabilities": safe_json(&profile.capabilities_json),
        "tool_policy": safe_json(&profile.tool_policy_json),
        "config": safe_json(&profile.config_json),
        "credential_handle_id": profile.credential_ref,
        "version": profile.version,
        "created_at": profile.created_at,
    })
}

pub(crate) fn execution_value(execution: Execution) -> Value {
    json!({
        "id": execution.id,
        "task_id": execution.task_id,
        "actor_ref": execution.actor_ref().map(|actor| match actor {
            db::ActorRef::Human(id) => json!({"kind": "human", "id": id}),
            db::ActorRef::Agent(id) => json!({"kind": "agent", "id": id}),
        }),
        "role": execution.role.to_string(),
        "purpose": execution.purpose.map(|purpose| purpose.to_string()),
        "status": execution.status.to_string(),
        "parent_execution_id": execution.parent_execution_id,
        "harness_session_id": execution.harness_session_id,
        "prompt": execution.prompt,
        "summary": execution.summary,
        "logs_path": execution.logs_path,
        "before_sha": execution.before_sha,
        "after_sha": execution.after_sha,
        "error": execution.error,
        "created_at": execution.created_at,
        "updated_at": execution.updated_at,
    })
}

fn safe_json(value: &str) -> Value {
    let parsed = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()));
    redact_sensitive(parsed)
}

fn redact_sensitive(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .filter_map(|(key, value)| {
                    let normalized = key.to_ascii_lowercase();
                    if normalized.contains("credential")
                        || normalized.contains("secret")
                        || normalized.contains("password")
                        || normalized == "token"
                        || normalized.ends_with("_token")
                        || normalized.contains("api_key")
                    {
                        return None;
                    }
                    Some((key, redact_sensitive(value)))
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_sensitive).collect()),
        other => other,
    }
}
