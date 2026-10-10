use api_types::{ProjectRelease, ProjectReleaseListResponse};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::{ProjectMemberRepo, ProjectRepo};
use serde::Deserialize;
use services::MilestoneRuntime;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ReleaseListQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

/// Historical immutable snapshot read. It never evaluates current readiness
/// or creates a new release.
pub async fn get_release(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((project_id, release_id)): Path<(String, String)>,
) -> ApiResult<Json<ProjectRelease>> {
    ensure_project_access(&state, &project_id, &user.user_id).await?;
    let release = MilestoneRuntime::new(state.db.clone())
        .get_release(&project_id, &release_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project_release", release_id))?;
    Ok(Json(release))
}

/// List immutable historical snapshots for the exact Project/Milestone pair.
pub async fn list_releases(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((project_id, milestone_id)): Path<(String, String)>,
    Query(query): Query<ReleaseListQuery>,
) -> ApiResult<Json<ProjectReleaseListResponse>> {
    ensure_project_access(&state, &project_id, &user.user_id).await?;
    let mut items = MilestoneRuntime::new(state.db.clone())
        .list_releases(&project_id, &milestone_id)
        .await?;
    if let Some((revision, id)) = decode_cursor(query.cursor.as_deref())? {
        let revision = revision
            .parse::<i64>()
            .map_err(|_| ApiError::bad_request("invalid cursor"))?;
        items.retain(|item| item.version > revision || (item.version == revision && item.id > id));
    }
    let limit = query.limit.unwrap_or(20).clamp(1, 100) as usize;
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = items
        .last()
        .map(|item| encode_cursor(&item.version.to_string(), &item.id));
    Ok(Json(ProjectReleaseListResponse {
        items,
        next_cursor: next_cursor.filter(|_| has_more),
        has_more,
    }))
}

async fn ensure_project_access(state: &AppState, project_id: &str, user_id: &str) -> ApiResult<()> {
    let project = ProjectRepo::get_by_id(&*state.db, project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", project_id.to_owned()))?;
    if project.owner_id.as_deref() != Some(user_id)
        && ProjectMemberRepo::get_member(&*state.db, project_id, user_id)
            .await?
            .is_none()
    {
        return Err(ApiError::not_found("project", project_id.to_owned()));
    }
    Ok(())
}

fn encode_cursor(revision: &str, id: &str) -> String {
    hex::encode(format!("{revision}\0{id}"))
}

fn decode_cursor(value: Option<&str>) -> ApiResult<Option<(String, String)>> {
    let Some(value) = value else { return Ok(None) };
    let bytes = hex::decode(value).map_err(|_| ApiError::bad_request("invalid cursor"))?;
    let decoded = String::from_utf8(bytes).map_err(|_| ApiError::bad_request("invalid cursor"))?;
    let (revision, id) = decoded
        .split_once('\0')
        .ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
    if revision.is_empty() || id.is_empty() {
        return Err(ApiError::bad_request("invalid cursor"));
    }
    Ok(Some((revision.to_owned(), id.to_owned())))
}
