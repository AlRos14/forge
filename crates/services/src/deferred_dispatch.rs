use db::{now_rfc3339, Task, TaskMetadata, TaskRepo};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Result, ServiceError};

const METADATA_KEY: &str = "deferred_dispatch";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct DeferredDispatch {
    pub not_before: String,
    pub reason: String,
    pub target_state: String,
}

pub(crate) async fn set(
    db: &db::SqliteDb,
    task: &Task,
    target_state: &str,
    not_before: &str,
    reason: &str,
) -> Result<()> {
    let mut metadata = parse_metadata(task)?;
    metadata.extra.insert(
        METADATA_KEY.to_owned(),
        json!({
            "not_before": not_before,
            "reason": reason,
            "target_state": target_state,
        }),
    );
    TaskRepo::set_metadata_json(db, &task.id, metadata.to_json(), &now_rfc3339()).await?;
    Ok(())
}

pub(crate) async fn clear(db: &db::SqliteDb, task: &Task) -> Result<()> {
    let mut metadata = parse_metadata(task)?;
    if metadata.extra.remove(METADATA_KEY).is_none() {
        return Ok(());
    }
    TaskRepo::set_metadata_json(db, &task.id, metadata.to_json(), &now_rfc3339()).await?;
    Ok(())
}

pub(crate) fn pending_until(task: &Task) -> Option<DeferredDispatch> {
    let metadata = TaskMetadata::parse(task.metadata_json.as_deref()).ok()?;
    let value = metadata.extra.get(METADATA_KEY)?.clone();
    serde_json::from_value(value).ok()
}

fn parse_metadata(task: &Task) -> Result<TaskMetadata> {
    TaskMetadata::parse(task.metadata_json.as_deref()).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid task metadata for {}: {error}", task.id))
    })
}
