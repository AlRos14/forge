use super::*;

impl TaskService {
    /// Ordinary content updates do not inspect or rewrite Task structure. An
    /// explicit parent field is handled in one immediate transaction so the
    /// structural reads and write are serialized with create/reparent calls.
    pub async fn update_task(&self, input: db::UpdateTask) -> Result<Task> {
        let Some(requested_parent) = input.parent_task_id.as_ref() else {
            return TaskRepo::update(&*self.db, input).await.map_err(Into::into);
        };

        let mut transaction = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let task = TaskRepo::get_by_id_in_tx(&*self.db, &mut transaction, &input.id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", input.id.clone()))?;
        if task.version != input.expected_version {
            return Err(db::DbError::VersionConflict.into());
        }

        let subtask_order = if let Some(parent_task_id) = requested_parent.as_deref() {
            validate_required("parent_task_id", parent_task_id)?;
            let _parent = super::subtask::validate_parent_placement_in_tx(
                &self.db,
                &mut transaction,
                Some(&task.id),
                &task.project_id,
                parent_task_id,
            )
            .await?;
            if task.parent_task_id.as_deref() == Some(parent_task_id)
                && task.subtask_order.is_some()
            {
                task.subtask_order
            } else {
                Some(
                    TaskRepo::next_subtask_order_in_tx(&*self.db, &mut transaction, parent_task_id)
                        .await?,
                )
            }
        } else {
            None
        };

        let updated = TaskRepo::update_parent_structure_in_tx(
            &*self.db,
            &mut transaction,
            input,
            subtask_order,
        )
        .await?;
        transaction.commit().await?;
        Ok(updated)
    }
}
