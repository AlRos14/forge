use std::path::{Component, Path as StdPath, PathBuf};

use super::*;

/// Remove storage references when a Task is deleted. The historical rows and
/// shared media assets remain governed by the existing soft-delete and pin
/// checks; PR12 removes only the Task media HTTP surface.
pub(crate) async fn delete_task_media_for_task(state: &AppState, task_id: &str) -> ApiResult<()> {
    let media = TaskMediaRepo::list_active_media_for_task(&*state.db, task_id).await?;
    for item in media {
        let asset = SharedMediaRepo::get_media_asset_for_task_media(&*state.db, &item.id).await?;
        let deleted_at = now_rfc3339();
        let deleted =
            match TaskMediaRepo::soft_delete_media(&*state.db, &item.id, &deleted_at).await {
                Ok(deleted) => deleted,
                Err(db::DbError::NotFound) => continue,
                Err(error) => return Err(error.into()),
            };
        if let Some(asset) = asset {
            maybe_collect_media_asset(state, &asset.id, &asset.storage_key, &deleted_at).await?;
        } else {
            remove_media_file(state, &item.storage_key)?;
        }
        publish_media_deleted(state, &deleted);
    }
    Ok(())
}

/// Claim and remove one unreferenced shared asset. The claim and finalization
/// recheck active Task/Project attachments and immutable release pins.
async fn maybe_collect_media_asset(
    state: &AppState,
    asset_id: &str,
    storage_key: &str,
    now: &str,
) -> ApiResult<()> {
    let lease_owner = format!("task-media-delete:{}", db::new_uuid_v4());
    let lease_expires_at = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    let candidate = SharedMediaRepo::claim_media_gc_candidate(
        &*state.db,
        asset_id,
        now,
        &lease_owner,
        &lease_expires_at,
    )
    .await?;
    let Some(candidate) = candidate else {
        return Ok(());
    };
    if candidate.storage_key != storage_key {
        return Err(ApiError::internal("shared media storage metadata changed"));
    }

    let path = media_storage_path(state, &candidate.storage_key)?;
    if let Err(error) = remove_file_if_exists(&path) {
        let _ = SharedMediaRepo::reset_media_gc_candidate(
            &*state.db,
            asset_id,
            &lease_owner,
            candidate.version,
            now,
        )
        .await;
        return Err(error);
    }

    let _ = SharedMediaRepo::complete_media_gc(
        &*state.db,
        asset_id,
        &lease_owner,
        candidate.version,
        now,
    )
    .await?;
    Ok(())
}

fn media_root(state: &AppState) -> PathBuf {
    state.effective_config.forge.data_dir.join("media")
}

fn media_storage_path(state: &AppState, storage_key: &str) -> ApiResult<PathBuf> {
    let path = StdPath::new(storage_key);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(ApiError::internal("invalid media storage key"));
    }
    Ok(media_root(state).join(path))
}

fn remove_media_file(state: &AppState, storage_key: &str) -> ApiResult<()> {
    remove_file_if_exists(&media_storage_path(state, storage_key)?)
}

fn remove_file_if_exists(path: &StdPath) -> ApiResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn publish_media_deleted(state: &AppState, media: &db::TaskMedia) {
    state.event_bus.publish(events::ForgeEvent {
        event_type: "task.media.deleted".to_owned(),
        entity_id: media.id.clone(),
        timestamp: events::event_timestamp(),
        context: events::EventContext::TaskMediaDeleted {
            task_id: media.task_id.clone(),
            media_id: media.id.clone(),
        },
    });
}
