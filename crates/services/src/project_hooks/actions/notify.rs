use crate::{
    project_hooks::actions::{ActionContext, ActionOutcome, HookActionHandler},
    Result,
};

pub struct NotifyAction<'a> {
    pub title: &'a str,
    pub message: &'a str,
    pub severity: Option<&'a str>,
}

#[async_trait::async_trait]
impl HookActionHandler for NotifyAction<'_> {
    async fn execute(&self, context: &ActionContext<'_>) -> Result<ActionOutcome> {
        let body = format!(
            "{}\n\nTrigger: {}\nDedupe key: {}\nHook run: {}\nRule: {}{}",
            self.message,
            context.trigger_match.trigger_type,
            context.trigger_match.dedupe_key,
            context.run.id,
            context.rule_id,
            self.severity
                .map(|severity| format!("\nSeverity: {severity}"))
                .unwrap_or_default()
        );
        let notification = context
            .service
            .notification_service
            .create_project_hook_notification(
                context.project.id.clone(),
                context.trigger_match.source_task_id.clone(),
                self.title.to_owned(),
                Some(body),
            )
            .await?;
        Ok(ActionOutcome::completed(format!(
            "notification {} created",
            notification.id
        )))
    }
}
