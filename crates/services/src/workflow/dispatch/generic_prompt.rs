use serde_json::json;

use crate::workflow::dispatch::{
    AgentDispatchContext, AgentPrompt, PromptBuilder, BUILDER_ID_GENERIC_DEFAULT_V2,
    MANAGED_EXECUTION_CONTRACT,
};

pub struct GenericPromptBuilder;

const GENERIC_ROLE_BOUNDARY: &str = "\
Generic boundary:
- Must follow the assigned role from the dispatch context and state any assumptions.
- Must not modify code unless the assigned role explicitly requires implementation work.
- Red flags: acting outside the named role, broad unrelated changes, hidden verification failures.";

impl PromptBuilder for GenericPromptBuilder {
    fn id(&self) -> &'static str {
        BUILDER_ID_GENERIC_DEFAULT_V2
    }

    fn build(&self, ctx: &AgentDispatchContext) -> AgentPrompt {
        let payload = json!({
            "task_id": ctx.task.id,
            "title": ctx.task.title,
            "description": ctx.task.description,
            "role": ctx.role,
            "state": ctx.state_name,
            "state_config": ctx.state_config,
            "last_manual_bounce_reason": ctx.last_manual_bounce_reason,
        });

        AgentPrompt {
            system: format!(
                "You are the {} agent for this Forge workflow task.\n\n{}\n\n{}",
                ctx.role, MANAGED_EXECUTION_CONTRACT, GENERIC_ROLE_BOUNDARY
            ),
            user: serde_json::to_string_pretty(&payload)
                .expect("generic prompt payload is serializable"),
            tools: Vec::new(),
        }
    }
}
