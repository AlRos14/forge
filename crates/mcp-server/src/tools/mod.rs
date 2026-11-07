mod descriptors;
mod handlers;

use serde_json::Value;

use crate::{error::McpToolError, protocol::McpContext, state::AppState};

pub(crate) use descriptors::tool_descriptors;
pub(crate) use handlers::{
    require_project_access, require_project_admin_access, require_project_write_access,
    require_task_access,
};

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
        "forge_get_task_dispatch" => {
            handlers::forge_get_task_dispatch(state, arguments, context).await
        }
        "forge_launch_task" => handlers::forge_launch_task(state, arguments, context).await,
        "forge_pause_task" => handlers::forge_pause_task(state, arguments, context).await,
        "forge_resume_task" => handlers::forge_resume_task(state, arguments, context).await,
        "forge_cancel_task" => handlers::forge_cancel_task(state, arguments, context).await,
        "forge_get_task_coordination" => {
            handlers::forge_get_task_coordination(state, arguments, context).await
        }
        "forge_authorize_task_coordination" => {
            handlers::forge_authorize_task_coordination(state, arguments, context).await
        }
        "forge_revoke_task_coordination" => {
            handlers::forge_revoke_task_coordination(state, arguments, context).await
        }
        "forge_request_task_coordination" => {
            handlers::forge_request_task_coordination(state, arguments, context).await
        }
        "forge_approve_task_coordination_plan" => {
            handlers::forge_approve_task_coordination_plan(state, arguments, context).await
        }
        "forge_materialize_task_coordination_plan" => {
            handlers::forge_materialize_task_coordination_plan(state, arguments, context).await
        }
        "forge_list_task_quality_requests" => {
            handlers::forge_list_task_quality_requests(state, arguments, context).await
        }
        "forge_create_task_quality_request" => {
            handlers::forge_create_task_quality_request(state, arguments, context).await
        }
        "forge_resolve_task_quality_request" => {
            handlers::forge_resolve_task_quality_request(state, arguments, context).await
        }
        "forge_list_task_interventions" => {
            handlers::forge_list_task_interventions(state, arguments, context).await
        }
        "forge_create_task_intervention" => {
            handlers::forge_create_task_intervention(state, arguments, context).await
        }
        "forge_request_rework" => handlers::forge_request_rework(state, arguments, context).await,
        "forge_update_task" => handlers::forge_update_task(state, arguments).await,
        "forge_create_work_unit" => {
            handlers::forge_create_work_unit(state, arguments, context).await
        }
        "forge_list_work_units" => handlers::forge_list_work_units(state, arguments, context).await,
        "forge_submit_work_unit_review" => {
            handlers::forge_submit_work_unit_review(state, arguments, context).await
        }
        "forge_get_work_unit" => handlers::forge_get_work_unit(state, arguments, context).await,
        "forge_get_work_unit_artifact_package" => {
            handlers::forge_get_work_unit_artifact_package(state, arguments, context).await
        }
        "forge_list_task_review_lines" => {
            handlers::forge_list_task_review_lines(state, arguments, context).await
        }
        "forge_update_work_unit" => {
            handlers::forge_update_work_unit(state, arguments, context).await
        }
        "forge_allocate_work_unit" => {
            handlers::forge_allocate_work_unit(state, arguments, context).await
        }
        "forge_list_work_unit_dependencies" => {
            handlers::forge_list_work_unit_dependencies(state, arguments, context).await
        }
        "forge_add_work_unit_dependency" => {
            handlers::forge_add_work_unit_dependency(state, arguments, context).await
        }
        "forge_remove_work_unit_dependency" => {
            handlers::forge_remove_work_unit_dependency(state, arguments, context).await
        }
        "forge_transition_work_unit" => {
            handlers::forge_transition_work_unit(state, arguments, context).await
        }
        "forge_register_agent" => handlers::forge_register_agent(state, arguments, context).await,
        "forge_list_agents" => handlers::forge_list_agents(state, arguments, context).await,
        "forge_list_projects" => handlers::forge_list_projects(state, arguments, context).await,
        "forge_get_project" => handlers::forge_get_project(state, arguments).await,
        "forge_create_project" => handlers::forge_create_project(state, arguments, context).await,
        "forge_update_project" => handlers::forge_update_project(state, arguments).await,
        "forge_update_project_hooks" => {
            handlers::forge_update_project_hooks(state, arguments).await
        }
        "forge_follow_up_execution" => {
            handlers::forge_follow_up_execution(state, arguments, context).await
        }
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
        "forge_update_task_role" => {
            handlers::forge_update_task_role(state, arguments, context).await
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
