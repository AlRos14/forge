use super::*;

pub async fn is_root_task(db: &SqliteDb, task_id: &str) -> Result<bool> {
    let task = TaskRepo::get_by_id(db, task_id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
    Ok(task.parent_task_id.is_none())
}

pub async fn is_subtask(db: &SqliteDb, task_id: &str) -> Result<bool> {
    Ok(!is_root_task(db, task_id).await?)
}

pub async fn root_for(db: &SqliteDb, task_id: &str) -> Result<Task> {
    let task = TaskRepo::get_by_id(db, task_id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
    let Some(parent_task_id) = task.parent_task_id.as_deref() else {
        return Ok(task);
    };
    TaskRepo::get_by_id(db, parent_task_id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", parent_task_id.to_owned()))
}
