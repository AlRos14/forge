use db::{ExecutionRepo, GateRepo, ProjectRepo, ValidationRunRepo};
use serde_json::{json, Value};

use crate::{
    error::McpToolError,
    params::{parse_params, ToolCallParams},
    protocol::McpContext,
    state::AppState,
    tools::{dispatch_tool, require_project_access, require_task_access, tool_descriptors},
};

#[cfg(test)]
pub(crate) async fn dispatch(
    state: &AppState,
    method: &str,
    params: Value,
) -> Result<Value, McpToolError> {
    dispatch_with_context(
        state,
        &McpContext {
            project_id: None,
            user_id: Some("test-user".to_owned()),
            is_admin: false,
        },
        method,
        params,
    )
    .await
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
    let user_id = context
        .user_id
        .as_deref()
        .ok_or_else(|| McpToolError::new(-32003, "authenticated server identity is required"))?;
    let scoped_project_id = context.project_id.as_deref();
    if let Some(project_id) = scoped_project_id {
        require_project_access(state, project_id, user_id).await?;
    }

    let object = arguments
        .as_object_mut()
        .ok_or_else(|| McpToolError::new(-32602, "tool arguments must be an object"))?;

    if tool_accepts_project_id(tool_name) {
        let project_id = match scoped_project_id {
            Some(project_id) => {
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
                project_id
            }
            None => match object.get("project_id").and_then(Value::as_str) {
                Some(project_id) => project_id,
                None => return Ok(arguments),
            },
        };
        if tool_name == "forge_create_task"
            && ProjectRepo::get_by_id(&*state.db, project_id)
                .await?
                .is_none()
        {
            // Preserve the tool's field-specific invalid-parameter response;
            // it does not touch the missing Project.
            return Ok(arguments);
        }
        require_project_access(state, project_id, user_id).await?;
        if tool_name == "forge_create_task" {
            if let Some(parent_task_id) = object.get("parent_task_id").and_then(Value::as_str) {
                let parent = require_task_access(state, parent_task_id, user_id).await?;
                if parent.project_id != project_id {
                    return Err(task_project_mismatch(scoped_project_id, parent_task_id));
                }
            }
        }
        return Ok(arguments);
    }

    if let Some(field_name) = task_scope_field(tool_name) {
        let task_id = required_string_arg(object, field_name)?;
        let task = require_task_access(state, task_id, user_id).await?;
        assert_task_scope(&task, scoped_project_id, task_id)?;
        if matches!(
            tool_name,
            "forge_add_task_dependency" | "forge_remove_task_dependency"
        ) {
            let dependency_id = required_string_arg(object, "depends_on_id")?;
            let dependency = require_task_access(state, dependency_id, user_id).await?;
            if task.project_id != dependency.project_id {
                return Err(task_project_mismatch(scoped_project_id, dependency_id));
            }
        }
        if tool_name == "forge_transition_task_lifecycle" {
            if let Some(evaluation_id) = object.get("gate_evaluation_id").and_then(Value::as_str) {
                let evaluation = GateRepo::get_gate_evaluation(&*state.db, evaluation_id)
                    .await?
                    .ok_or_else(|| {
                        McpToolError::not_found("GateEvaluation", evaluation_id.to_owned())
                    })?;
                if evaluation.task_id != task.id {
                    return Err(task_project_mismatch(scoped_project_id, &task.id));
                }
            }
        }
        return Ok(arguments);
    }

    if matches!(
        tool_name,
        "forge_follow_up_execution" | "forge_get_review_execution"
    ) {
        let execution_id = required_string_arg(object, "execution_id")?;
        assert_execution_access(state, execution_id, user_id, scoped_project_id).await?;
    }
    if matches!(
        tool_name,
        "forge_get_gate" | "forge_revise_gate_policy" | "forge_evaluate_gate"
    ) {
        let id = required_string_arg(object, "gate_id")?;
        let gate = GateRepo::get_gate(&*state.db, id)
            .await?
            .ok_or_else(|| McpToolError::not_found("Gate", id.to_owned()))?;
        let task = require_task_access(state, &gate.task_id, user_id).await?;
        assert_task_scope(&task, scoped_project_id, &gate.task_id)?;
    }
    if tool_name == "forge_get_gate_evaluation" {
        let id = required_string_arg(object, "evaluation_id")?;
        let evaluation = GateRepo::get_gate_evaluation(&*state.db, id)
            .await?
            .ok_or_else(|| McpToolError::not_found("GateEvaluation", id.to_owned()))?;
        let task = require_task_access(state, &evaluation.task_id, user_id).await?;
        assert_task_scope(&task, scoped_project_id, &evaluation.task_id)?;
    }
    if tool_name == "forge_get_validation_run" {
        let id = required_string_arg(object, "validation_run_id")?;
        let run = ValidationRunRepo::get_validation_run(&*state.db, id)
            .await?
            .ok_or_else(|| McpToolError::not_found("ValidationRun", id.to_owned()))?;
        let task = require_task_access(state, &run.task_id, user_id).await?;
        assert_task_scope(&task, scoped_project_id, &run.task_id)?;
    }
    if tool_name == "forge_get_evidence" {
        let id = required_string_arg(object, "evidence_id")?;
        let evidence = ValidationRunRepo::get_evidence(&*state.db, id)
            .await?
            .ok_or_else(|| McpToolError::not_found("Evidence", id.to_owned()))?;
        let task = require_task_access(state, &evidence.task_id, user_id).await?;
        assert_task_scope(&task, scoped_project_id, &evidence.task_id)?;
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
            | "forge_update_project_hooks"
    )
}

fn task_scope_field(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "forge_create_sub_tasks" => Some("parent_task_id"),
        "forge_get_task"
        | "forge_update_task"
        | "forge_get_task_diff"
        | "forge_list_executions"
        | "forge_start_execution"
        | "forge_add_task_dependency"
        | "forge_remove_task_dependency"
        | "forge_list_task_dependencies"
        | "forge_get_task_lifecycle"
        | "forge_list_task_lifecycle_transitions"
        | "forge_transition_task_lifecycle"
        | "forge_list_task_roles"
        | "forge_create_task_role"
        | "forge_add_task_role_member"
        | "forge_create_task_gate"
        | "forge_list_task_review_executions"
        | "forge_list_validation_runs"
        | "forge_list_task_artifacts"
        | "forge_list_task_messages"
        | "forge_list_task_handoffs"
        | "forge_list_task_proposals"
        | "forge_list_task_decisions" => Some("task_id"),
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

fn assert_task_scope(
    task: &db::Task,
    project_id: Option<&str>,
    task_id: &str,
) -> Result<(), McpToolError> {
    if let Some(project_id) = project_id {
        if task.project_id != project_id {
            return Err(task_project_mismatch(Some(project_id), task_id));
        }
    }
    Ok(())
}

fn task_project_mismatch(project_id: Option<&str>, task_id: &str) -> McpToolError {
    McpToolError::new(
        -32602,
        if project_id.is_some() {
            "task does not belong to scoped MCP project"
        } else {
            "related task belongs to a different project"
        },
    )
    .with_data(json!({ "project_id": project_id, "task_id": task_id }))
}

async fn assert_execution_access(
    state: &AppState,
    execution_id: &str,
    user_id: &str,
    project_id: Option<&str>,
) -> Result<(), McpToolError> {
    let execution = ExecutionRepo::get_by_id(&*state.db, execution_id)
        .await?
        .ok_or_else(|| McpToolError::not_found("execution", execution_id.to_owned()))?;
    let task = require_task_access(state, &execution.task_id, user_id).await?;
    assert_task_scope(&task, project_id, &execution.task_id)
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
