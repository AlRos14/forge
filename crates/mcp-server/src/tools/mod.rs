mod descriptors;
mod handlers;

use serde_json::Value;

use crate::{error::McpToolError, protocol::McpContext, state::AppState};

pub(crate) use descriptors::tool_descriptors;

pub(crate) async fn dispatch_tool(
    state: &AppState,
    name: &str,
    arguments: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    match name {
        "forge_create_task" => handlers::forge_create_task(state, arguments).await,
        "forge_create_sub_tasks" => handlers::forge_create_sub_tasks(state, arguments).await,
        "forge_add_task_dependency" => handlers::forge_add_task_dependency(state, arguments).await,
        "forge_remove_task_dependency" => {
            handlers::forge_remove_task_dependency(state, arguments).await
        }
        "forge_list_task_dependencies" => {
            handlers::forge_list_task_dependencies(state, arguments).await
        }
        "forge_list_tasks" => handlers::forge_list_tasks(state, arguments).await,
        "forge_get_task" => handlers::forge_get_task(state, arguments).await,
        "forge_get_task_diff" => handlers::forge_get_task_diff(state, arguments).await,
        "forge_list_executions" => handlers::forge_list_executions(state, arguments).await,
        "forge_start_execution" => handlers::forge_start_execution(state, arguments, context).await,
        "forge_update_task" => handlers::forge_update_task(state, arguments).await,
        "forge_register_agent" => handlers::forge_register_agent(state, arguments).await,
        "forge_list_agents" => handlers::forge_list_agents(state, arguments).await,
        "forge_list_projects" => handlers::forge_list_projects(state, arguments).await,
        "forge_get_project" => handlers::forge_get_project(state, arguments).await,
        "forge_create_project" => handlers::forge_create_project(state, arguments).await,
        "forge_update_project" => handlers::forge_update_project(state, arguments).await,
        "forge_update_project_hooks" => {
            handlers::forge_update_project_hooks(state, arguments).await
        }
        "forge_follow_up_execution" => handlers::forge_follow_up_execution(state, arguments).await,
        "forge_list_agent_profiles" => {
            handlers::forge_list_agent_profiles(state, arguments, context).await
        }
        "forge_get_task_lifecycle" => handlers::forge_get_task_lifecycle(state, arguments).await,
        "forge_list_task_lifecycle_transitions" => {
            handlers::forge_list_task_lifecycle_transitions(state, arguments).await
        }
        "forge_transition_task_lifecycle" => {
            handlers::forge_transition_task_lifecycle(state, arguments, context).await
        }
        "forge_list_task_roles" => handlers::forge_list_task_roles(state, arguments).await,
        "forge_create_task_role" => {
            handlers::forge_create_task_role(state, arguments, context).await
        }
        "forge_add_task_role_member" => {
            handlers::forge_add_task_role_member(state, arguments, context).await
        }
        "forge_create_task_gate" => handlers::forge_create_task_gate(state, arguments).await,
        "forge_get_gate" => handlers::forge_get_gate(state, arguments).await,
        "forge_revise_gate_policy" => handlers::forge_revise_gate_policy(state, arguments).await,
        "forge_evaluate_gate" => handlers::forge_evaluate_gate(state, arguments).await,
        "forge_get_gate_evaluation" => handlers::forge_get_gate_evaluation(state, arguments).await,
        "forge_list_task_review_executions" => {
            handlers::forge_list_task_review_executions(state, arguments).await
        }
        "forge_get_review_execution" => {
            handlers::forge_get_review_execution(state, arguments).await
        }
        "forge_list_validation_runs" => {
            handlers::forge_list_validation_runs(state, arguments).await
        }
        "forge_get_validation_run" => handlers::forge_get_validation_run(state, arguments).await,
        "forge_get_evidence" => handlers::forge_get_evidence(state, arguments).await,
        "forge_list_task_artifacts" => handlers::forge_list_task_artifacts(state, arguments).await,
        "forge_list_task_messages" => handlers::forge_list_task_messages(state, arguments).await,
        "forge_list_task_handoffs" => handlers::forge_list_task_handoffs(state, arguments).await,
        "forge_list_task_proposals" => handlers::forge_list_task_proposals(state, arguments).await,
        "forge_list_task_decisions" => handlers::forge_list_task_decisions(state, arguments).await,
        _ => Err(McpToolError::new(-32601, "method not found")),
    }
}
