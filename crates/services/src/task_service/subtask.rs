use super::*;

/// Validates one structural parent write while the caller holds SQLite's
/// `BEGIN IMMEDIATE` transaction, so all hierarchy reads share the writer's
/// serialized snapshot.
pub(super) async fn validate_parent_placement_in_tx(
    db: &SqliteDb,
    transaction: &mut sqlx::Transaction<'_, db::Sqlite>,
    task_id: Option<&str>,
    project_id: &str,
    parent_task_id: &str,
) -> Result<Task> {
    let parent = TaskRepo::get_by_id_in_tx(db, transaction, parent_task_id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", parent_task_id.to_owned()))?;
    if parent.project_id != project_id {
        return Err(ServiceError::not_found("task", parent_task_id.to_owned()));
    }
    if task_id == Some(parent_task_id) {
        return Err(ServiceError::invalid_operation(
            "a Task cannot be its own parent",
        ));
    }

    let mut seen = HashSet::new();
    let mut ancestor = parent.clone();
    loop {
        if task_id == Some(ancestor.id.as_str()) {
            return Err(ServiceError::invalid_operation(
                "Task parent change would create a cycle",
            ));
        }
        if !seen.insert(ancestor.id.clone()) {
            return Err(ServiceError::invalid_operation(
                "Task parent chain already contains a cycle",
            ));
        }
        let Some(ancestor_id) = ancestor.parent_task_id.as_deref() else {
            break;
        };
        ancestor = TaskRepo::get_by_id_in_tx(db, transaction, ancestor_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", ancestor_id.to_owned()))?;
        if ancestor.project_id != project_id {
            return Err(ServiceError::not_found("task", ancestor.id));
        }
    }

    if parent.parent_task_id.is_some() {
        return Err(ServiceError::nested_subtask_unsupported());
    }
    if let Some(task_id) = task_id {
        if TaskRepo::count_live_children_in_tx(db, transaction, task_id).await? != 0 {
            return Err(ServiceError::nested_subtask_unsupported());
        }
    }
    Ok(parent)
}

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
