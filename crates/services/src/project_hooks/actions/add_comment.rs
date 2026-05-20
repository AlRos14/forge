use db::TaskRepo;

use crate::{
    project_hooks::actions::{ActionContext, ActionOutcome, HookActionHandler},
    Result, ServiceError,
};

pub struct AddCommentAction<'a> {
    pub target_task_id: Option<&'a str>,
    pub content: &'a str,
}

#[async_trait::async_trait]
impl HookActionHandler for AddCommentAction<'_> {
    async fn execute(&self, context: &ActionContext<'_>) -> Result<ActionOutcome> {
        let task_id = self
            .target_task_id
            .or(context.trigger_match.source_task_id.as_deref())
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "add_comment requires target_task_id or trigger source task",
                )
            })?;
        let task = TaskRepo::get_by_id(&*context.service.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        if task.project_id != context.project.id {
            return Err(ServiceError::invalid_operation(format!(
                "task {task_id} does not belong to project {}",
                context.project.id
            )));
        }

        let content = format!(
            "{}\n\nProject hook run: {}\nRule: {}",
            self.content, context.run.id, context.rule_id
        );
        context
            .service
            .task_service
            .create_system_comment(task_id, content)
            .await?;
        Ok(ActionOutcome::completed(format!(
            "comment added to task {task_id}"
        )))
    }
}
