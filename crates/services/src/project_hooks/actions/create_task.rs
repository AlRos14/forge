use crate::{
    project_hooks::actions::{
        task_type_to_string, ActionContext, ActionOutcome, HookActionHandler,
    },
    Result,
};

pub struct CreateTaskAction<'a> {
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub task_type: Option<api_types::TaskType>,
    pub priority: Option<i64>,
}

#[async_trait::async_trait]
impl HookActionHandler for CreateTaskAction<'_> {
    async fn execute(&self, context: &ActionContext<'_>) -> Result<ActionOutcome> {
        let task = context
            .service
            .task_service
            .create_task(
                context.project.id.clone(),
                self.title.to_owned(),
                Some(description_with_hook_context(context, self.description)),
                None,
                self.priority,
                self.task_type.map(task_type_to_string),
                None,
                None,
                None,
            )
            .await?;
        Ok(ActionOutcome::completed(format!(
            "created task {}",
            task.id
        )))
    }
}

fn description_with_hook_context(context: &ActionContext<'_>, description: Option<&str>) -> String {
    let mut parts = description
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(|description| vec![description.to_owned()])
        .unwrap_or_default();
    parts.push(format!("Project hook run: {}", context.run.id));
    parts.push(format!("Rule: {}", context.rule_id));
    parts.join("\n\n")
}
