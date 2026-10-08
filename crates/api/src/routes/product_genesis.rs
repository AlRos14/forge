//! Product Genesis routes on the account's existing Main Agent Chat.
//!
//! Product Genesis is retained as a historical read surface until PR12.
//! Mutations return the PR11 retired-operation error.

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};
use api_types::{
    CancelProductGenesisRequest, ProductGenesisActiveResponse, ProductGenesisStartResponse,
    StartProductGenesisRequest,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

/// Start Product Genesis in the existing global Main Agent Chat.
pub async fn start_product_genesis(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Json(_request): Json<StartProductGenesisRequest>,
) -> ApiResult<(StatusCode, Json<ProductGenesisStartResponse>)> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "Product Genesis was retired in Plan PR11; create a Project directly",
    ))
}

/// Return the authenticated account's active Genesis session, if any.
pub async fn get_active_product_genesis(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> ApiResult<Json<ProductGenesisActiveResponse>> {
    Ok(Json(ProductGenesisActiveResponse {
        session: state.product_genesis_history.active(&user.user_id).await?,
    }))
}

/// Read one Genesis session from the authenticated account's history.
///
/// A session identifier is only a lookup key: ownership is checked against
/// the authenticated account before the durable record is returned.
pub async fn get_product_genesis(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(session_id): Path<String>,
) -> ApiResult<Json<api_types::ProductGenesisSession>> {
    let session = state.product_genesis_history.get(&session_id).await?;
    if session.account_id != user.user_id {
        return Err(ApiError::not_found("product_genesis_session", session_id));
    }
    Ok(Json(session))
}

/// Cancel an active Genesis session with optimistic concurrency.
pub async fn cancel_product_genesis(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Path(_session_id): Path<String>,
    Json(_request): Json<CancelProductGenesisRequest>,
) -> ApiResult<Json<api_types::ProductGenesisSession>> {
    Err(ApiError::gone_with_code(
        "operation_retired",
        "Product Genesis sessions are historical and cannot be changed after PR11",
    ))
}
