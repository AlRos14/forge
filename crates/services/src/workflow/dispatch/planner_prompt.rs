use crate::plan_artifact::PLAN_ARTIFACT_AGENT_INSTRUCTION;
use crate::workflow::{
    default_roles,
    dispatch::{
        default_tool_names, AgentDispatchContext, AgentPrompt, PromptBuilder,
        BUILDER_ID_PLANNER_DEFAULT_V1,
    },
};

pub struct PlannerPromptBuilder;

impl PromptBuilder for PlannerPromptBuilder {
    fn id(&self) -> &'static str {
        BUILDER_ID_PLANNER_DEFAULT_V1
    }

    fn build(&self, ctx: &AgentDispatchContext) -> AgentPrompt {
        let mut user = format!("Plan task: {}\n", ctx.task.title);

        if let Some(description) = ctx.task.description.as_deref() {
            user.push_str("\nTask description:\n");
            user.push_str(description);
            user.push('\n');
        }

        if let Some(parent) = &ctx.parent_task {
            user.push_str("\nParent task:\n");
            user.push_str(&parent.title);
            if let Some(description) = parent.description.as_deref() {
                user.push('\n');
                user.push_str(description);
            }
            user.push('\n');
        }

        if !ctx.sub_tasks.is_empty() {
            user.push_str("\nExisting sub-tasks:\n");
            for task in &ctx.sub_tasks {
                user.push_str("- ");
                user.push_str(&task.title);
                user.push('\n');
            }
        }

        user.push_str("\nPlanning output:\n");
        user.push_str(PLAN_ARTIFACT_AGENT_INSTRUCTION);
        user.push('\n');

        AgentPrompt {
            system: "You are the planner for this Forge workflow task. Your job is to investigate the codebase, design an approach, and produce a concrete implementation plan with clear verification steps. You do not implement the code — the coder agent will receive your plan and execute it. Leave implementation work unchecked unless it is already complete.".to_string(),
            user,
            tools: default_tool_names(default_roles::PLANNER),
        }
    }
}
