use serde_json::json;

use crate::workflow::dispatch::{
    AgentDispatchContext, AgentPrompt, PromptBuilder, BUILDER_ID_GENERIC_DEFAULT_V1,
};

pub struct GenericPromptBuilder;

impl PromptBuilder for GenericPromptBuilder {
    fn id(&self) -> &'static str {
        BUILDER_ID_GENERIC_DEFAULT_V1
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
                "You are the {} agent for this Forge workflow task.",
                ctx.role
            ),
            user: serde_json::to_string_pretty(&payload)
                .expect("generic prompt payload is serializable"),
            tools: Vec::new(),
        }
    }
}
