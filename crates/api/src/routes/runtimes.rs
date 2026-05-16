use api_types::{PaginatedResponse, RuntimeResponse};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::{RuntimeListQuery, RuntimeRepo};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{auth::RequireAdmin, page_request, paginated, runtime_response, ListParams},
    state::AppState,
};

pub async fn list_runtimes(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<PaginatedResponse<RuntimeResponse>>> {
    let pr = page_request(&params)?;
    let page = RuntimeRepo::list(
        &*state.db,
        RuntimeListQuery {
            daemon_id: params.daemon_id,
            page: pr,
        },
    )
    .await?;
    Ok(Json(paginated(page, runtime_response)))
}

pub async fn get_runtime(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
) -> ApiResult<Json<RuntimeResponse>> {
    let runtime = RuntimeRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("runtime", id))?;
    Ok(Json(runtime_response(runtime)))
}
