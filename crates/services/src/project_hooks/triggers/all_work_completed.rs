use async_trait::async_trait;
use sqlx::Row;

use crate::{
    project_hooks::triggers::{HookTrigger, TriggerContext, TriggerMatch},
    Result,
};

pub const ALL_WORK_COMPLETED_TRIGGER_TYPE: &str = "project.all_work_completed";

pub struct AllWorkCompletedTrigger;

#[async_trait]
impl HookTrigger for AllWorkCompletedTrigger {
    async fn evaluate(&self, context: &TriggerContext<'_>) -> Result<Option<TriggerMatch>> {
        let rows = sqlx::query(
            "SELECT task.id, lifecycle.state AS lifecycle_state \
             FROM task \
             LEFT JOIN task_lifecycle AS lifecycle ON lifecycle.task_id = task.id \
             WHERE task.project_id = ? \
               AND task.is_automation = 0 \
               AND task.archived_at IS NULL \
               AND task.deleted_at IS NULL",
        )
        .bind(&context.project.id)
        .fetch_all(context.db.pool())
        .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        for row in rows {
            let lifecycle_state: Option<String> = row.try_get("lifecycle_state")?;
            if !matches!(lifecycle_state.as_deref(), Some("done" | "cancelled")) {
                return Ok(None);
            }
        }

        let epoch = context.project.project_work_epoch;
        Ok(Some(TriggerMatch {
            trigger_type: ALL_WORK_COMPLETED_TRIGGER_TYPE.to_owned(),
            dedupe_key: format!("{ALL_WORK_COMPLETED_TRIGGER_TYPE}:{epoch}"),
            source_task_id: context.cause.source_task_id().map(str::to_owned),
            source_execution_id: None,
            reason: Some(format!(
                "all visible project work completed at epoch {epoch}"
            )),
        }))
    }
}
