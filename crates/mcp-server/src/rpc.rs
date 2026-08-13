use db::{ExecutionRepo, ProjectMemberRepo, ProjectRepo, TaskRepo};
use serde_json::{json, Value};

use crate::{
    error::McpToolError,
    params::{parse_params, ToolCallParams},
    protocol::McpContext,
    state::AppState,
    tools::{dispatch_tool, tool_descriptors},
};

#[cfg(test)]
pub(crate) async fn dispatch(
    state: &AppState,
    method: &str,
    params: Value,
) -> Result<Value, McpToolError> {
    dispatch_with_context(state, &McpContext::default(), method, params).await
}

pub(crate) async fn dispatch_with_context(
    state: &AppState,
    context: &McpContext,
    method: &str,
    params: Value,
) -> Result<Value, McpToolError> {
    match method {
        "initialize" => handle_initialize(),
        "notifications/initialized" => Ok(Value::Null),
        "tools/list" => handle_tools_list(context),
        "tools/call" => {
            let params: ToolCallParams = parse_params(params)?;
            let arguments = match params.arguments {
                Value::Null => json!({}),
                arguments => arguments,
            };
            let arguments = apply_project_scope(state, &params.name, arguments, context).await?;
            let result = dispatch_tool(state, &params.name, arguments, context).await?;
            Ok(tool_call_result(result))
        }
        _ => Err(McpToolError::new(-32601, "method not found")),
    }
}

async fn apply_project_scope(
    state: &AppState,
    tool_name: &str,
    mut arguments: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let Some(project_id) = context.project_id.as_deref() else {
        return Ok(arguments);
    };

    if let Some(user_id) = context.user_id.as_deref() {
        assert_project_membership(state, project_id, user_id).await?;
    }

    let object = arguments
        .as_object_mut()
        .ok_or_else(|| McpToolError::new(-32602, "tool arguments must be an object"))?;

    if tool_accepts_project_id(tool_name) {
        match object.get("project_id").and_then(Value::as_str) {
            Some(existing) if existing == project_id => {}
            Some(_) => {
                return Err(McpToolError::new(
                    -32602,
                    "project_id does not match scoped MCP project",
                )
                .with_data(json!({ "project_id": project_id })));
            }
            None => {
                object.insert(
                    "project_id".to_owned(),
                    Value::String(project_id.to_owned()),
                );
            }
        }
        return Ok(arguments);
    }

    if let Some(field_name) = task_scope_field(tool_name) {
        let task_id = required_string_arg(object, field_name)?;
        assert_task_in_scope(state, project_id, task_id).await?;
        return Ok(arguments);
    }

    if tool_name == "forge_follow_up_execution" {
        let execution_id = required_string_arg(object, "execution_id")?;
        assert_execution_in_scope(state, project_id, execution_id).await?;
    }

    Ok(arguments)
}

fn tool_accepts_project_id(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "forge_create_task"
            | "forge_list_tasks"
            | "forge_get_project"
            | "forge_update_project"
            | "forge_update_project_lifecycle_hooks"
            | "forge_memory_search"
            | "forge_get_project_agent"
            | "forge_set_project_agent"
            | "forge_list_agent_handoffs"
            | "forge_get_agent_handoff"
            | "forge_create_agent_handoff"
    )
}

fn task_scope_field(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "forge_get_task"
        | "forge_preview_prompt"
        | "forge_assign_agent"
        | "forge_cancel_task"
        | "forge_get_task_diff"
        | "forge_list_executions"
        | "forge_update_task"
        | "forge_transition_task" => Some("task_id"),
        "forge_create_sub_tasks" => Some("parent_task_id"),
        _ => None,
    }
}

fn required_string_arg<'a>(
    arguments: &'a serde_json::Map<String, Value>,
    field_name: &str,
) -> Result<&'a str, McpToolError> {
    arguments
        .get(field_name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| McpToolError::new(-32602, format!("missing required field `{field_name}`")))
}

async fn assert_task_in_scope(
    state: &AppState,
    project_id: &str,
    task_id: &str,
) -> Result<(), McpToolError> {
    let task = TaskRepo::get_by_id(&*state.db, task_id, false)
        .await?
        .ok_or_else(|| McpToolError::not_found("task", task_id.to_owned()))?;
    if task.project_id != project_id {
        return Err(
            McpToolError::new(-32602, "task does not belong to scoped MCP project").with_data(
                json!({
                    "project_id": project_id,
                    "task_id": task_id,
                }),
            ),
        );
    }
    Ok(())
}

async fn assert_execution_in_scope(
    state: &AppState,
    project_id: &str,
    execution_id: &str,
) -> Result<(), McpToolError> {
    let execution = ExecutionRepo::get_by_id(&*state.db, execution_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("execution", execution_id.to_owned()))?;
    assert_task_in_scope(state, project_id, &execution.task_id).await
}

async fn assert_project_membership(
    state: &AppState,
    project_id: &str,
    user_id: &str,
) -> Result<(), McpToolError> {
    let project = ProjectRepo::get_by_id(&*state.db, project_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("project", project_id.to_owned()))?;

    if project.owner_id.as_deref() == Some(user_id) {
        return Ok(());
    }

    let member = ProjectMemberRepo::get_member(&*state.db, project_id, user_id).await?;
    if member.is_some() {
        return Ok(());
    }

    Err(McpToolError::new(-32001, "project not accessible"))
}

fn handle_initialize() -> Result<Value, McpToolError> {
    Ok(json!({
        "protocolVersion": "2025-03-26",
        "serverInfo": {
            "name": "forge-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": {
            "tools": {},
        },
    }))
}

fn handle_tools_list(context: &McpContext) -> Result<Value, McpToolError> {
    Ok(json!({ "tools": tool_descriptors(context.project_id.is_some()) }))
}

fn tool_call_result(result: Value) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string(&result).unwrap_or_else(|_| result.to_string()),
            }
        ],
    })
}
