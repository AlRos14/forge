use api_types::{BranchListResponse, FsBranchesParams, FsListParams, FsListResponse};
use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use services::ServiceError;

use crate::errors::{ApiError, ApiResult};
use crate::routes::auth::RequireAdmin;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct PathQuery {
    path: String,
    daemon_id: String,
}

pub async fn list_entries(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Query(params): Query<PathQuery>,
) -> ApiResult<Json<FsListResponse>> {
    let provider = services::daemon_transport::select_filesystem_provider(
        &params.daemon_id,
        &state.db,
        &state.daemon_connections,
    )
    .await?;
    let result = provider.list(FsListParams { path: params.path }).await?;
    Ok(Json(FsListResponse {
        path: result.path,
        entries: result.entries,
    }))
}

pub async fn list_branches(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Query(params): Query<PathQuery>,
) -> ApiResult<Json<BranchListResponse>> {
    let provider = services::daemon_transport::select_filesystem_provider(
        &params.daemon_id,
        &state.db,
        &state.daemon_connections,
    )
    .await?;
    let branches = provider
        .branches(FsBranchesParams { path: params.path })
        .await
        .map_err(branch_error)?;
    Ok(Json(BranchListResponse {
        branches: branches.branches,
        default_branch: branches.default_branch,
        origin_url: branches.origin_url,
    }))
}

fn branch_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::InvalidOperation { message } if message == "path is not a git repository" => {
            ApiError::bad_request_with_code("fs.not_a_git_repo", message)
        }
        ServiceError::InvalidOperation { message }
            if message.starts_with("failed to list branches:") =>
        {
            ApiError::bad_request_with_code("fs.branch_list_failed", message)
        }
        other => other.into(),
    }
}
