use super::*;

const MIN_BOARD_POSITION_GAP: f64 = 1e-9;

impl TaskService {
    pub async fn reorder_task(
        &self,
        task_id: String,
        before_id: Option<String>,
        after_id: Option<String>,
    ) -> Result<Task> {
        validate_required("task_id", &task_id)?;
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;

        if before_id.is_none() && after_id.is_none() {
            return Err(ServiceError::invalid_operation(
                "before_id and after_id cannot both be null",
            ));
        }
        if before_id.is_some() && before_id == after_id {
            return Err(ServiceError::invalid_operation(
                "before_id and after_id cannot be the same task",
            ));
        }
        if before_id.as_deref() == Some(task.id.as_str())
            || after_id.as_deref() == Some(task.id.as_str())
        {
            return Err(ServiceError::invalid_operation(
                "neighbour ID cannot equal task_id",
            ));
        }

        let mut before = load_neighbour(&self.db, &task, before_id.as_deref()).await?;
        let mut after = load_neighbour(&self.db, &task, after_id.as_deref()).await?;

        if let (Some(before_task), Some(after_task)) = (&before, &after) {
            if (after_task.board_position - before_task.board_position).abs()
                < MIN_BOARD_POSITION_GAP
            {
                renormalize_board_positions(&self.db, &task.project_id).await?;
                before = load_neighbour(&self.db, &task, before_id.as_deref()).await?;
                after = load_neighbour(&self.db, &task, after_id.as_deref()).await?;
            }
        }

        let new_position = match (&before, &after) {
            (Some(before_task), Some(after_task)) => {
                (before_task.board_position + after_task.board_position) / 2.0
            }
            (Some(before_task), None) => before_task.board_position + 1.0,
            (None, Some(after_task)) => after_task.board_position - 1.0,
            (None, None) => unreachable!("both null rejected above"),
        };

        Ok(TaskRepo::reorder_task(&*self.db, &task.id, new_position, &now_rfc3339()).await?)
    }
}

async fn load_neighbour(
    db: &SqliteDb,
    task: &Task,
    neighbour_id: Option<&str>,
) -> Result<Option<Task>> {
    let Some(neighbour_id) = neighbour_id else {
        return Ok(None);
    };
    let neighbour = TaskRepo::get_by_id(db, neighbour_id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", neighbour_id.to_owned()))?;
    if neighbour.project_id != task.project_id {
        return Err(ServiceError::invalid_operation(format!(
            "neighbour task {neighbour_id} belongs to a different project"
        )));
    }
    Ok(Some(neighbour))
}

async fn renormalize_board_positions(db: &SqliteDb, project_id: &str) -> Result<()> {
    let mut transaction = db.pool().begin().await?;
    let task_ids = sqlx::query_scalar::<_, String>(
        "SELECT id FROM task WHERE project_id = ? AND deleted_at IS NULL ORDER BY board_position ASC, id ASC",
    )
    .bind(project_id)
    .fetch_all(&mut *transaction)
    .await?;
    let updated_at = now_rfc3339();

    for (index, task_id) in task_ids.iter().enumerate() {
        sqlx::query("UPDATE task SET board_position = ?, updated_at = ? WHERE id = ?")
            .bind(index as f64 + 1.0)
            .bind(&updated_at)
            .bind(task_id)
            .execute(&mut *transaction)
            .await?;
    }

    transaction.commit().await?;
    Ok(())
}
