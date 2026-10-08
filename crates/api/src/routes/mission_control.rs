use api_types::{
    AgentDetailResponse, AttentionItem, AttentionListResponse, AttentionMutationRequest,
    AttentionSnoozeRequest, MissionControlHomeResponse, MissionControlQuery,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};

/// Mission Control handlers are intentionally kept in a separate module so
/// the router can be registered as one bounded read/mutation surface by the
/// application owner.  The service is cheap and stateless; constructing it
/// per request also keeps this slice independent of AppState wiring.
pub async fn home(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Query(_query): Query<MissionControlQuery>,
) -> ApiResult<Json<MissionControlHomeResponse>> {
    Err(retired_attention_error())
}

pub async fn list_attention(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    Query(_query): Query<MissionControlQuery>,
) -> ApiResult<Json<AttentionListResponse>> {
    Err(retired_attention_error())
}

pub async fn acknowledge(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    Json(_body): Json<AttentionMutationRequest>,
) -> ApiResult<Json<AttentionItem>> {
    Err(retired_attention_error())
}

pub async fn snooze(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    Json(_body): Json<AttentionSnoozeRequest>,
) -> ApiResult<Json<AttentionItem>> {
    Err(retired_attention_error())
}

pub async fn resolve(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    _id: Path<String>,
    Json(_body): Json<AttentionMutationRequest>,
) -> ApiResult<Json<AttentionItem>> {
    Err(retired_attention_error())
}

pub async fn agent_detail(
    State(_state): State<AppState>,
    _user: AuthenticatedUser,
    _identity_id: Path<String>,
    Query(_query): Query<MissionControlQuery>,
) -> ApiResult<Json<AgentDetailResponse>> {
    Err(retired_attention_error())
}

fn retired_attention_error() -> ApiError {
    ApiError::gone_with_code(
        "operation_retired",
        "Main/Project Agent Attention was retired in Plan PR11",
    )
}
