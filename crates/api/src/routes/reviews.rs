use api_types::ReviewResponse;
use axum::{
    extract::{Path, State},
    Json,
};
use db::ReviewRepo;

use crate::{
    errors::{ApiError, ApiResult},
    routes::review_response_strict,
    state::AppState,
};

pub async fn get_review(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<ReviewResponse>> {
    let review = ReviewRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("review", id))?;
    Ok(Json(review_response_strict(review)?))
}
