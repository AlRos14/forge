use api_types::{NotificationResponse, PaginatedResponse, UnreadCountResponse};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use db::{NotificationListQuery, NotificationRepo, PageRequest, SortBy, SortOrder};
use serde::Deserialize;

use crate::{errors::ApiResult, routes::paginated, state::AppState};

#[derive(Debug, Clone, Deserialize)]
pub struct ListNotificationsParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub include_total: Option<bool>,
    pub project_id: Option<String>,
    pub read: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CountParams {
    pub project_id: Option<String>,
}

pub async fn list_notifications(
    State(state): State<AppState>,
    Query(params): Query<ListNotificationsParams>,
) -> ApiResult<Json<PaginatedResponse<NotificationResponse>>> {
    let page = NotificationRepo::list(
        &*state.db,
        NotificationListQuery {
            project_id: params.project_id,
            read: params.read,
            page: PageRequest {
                cursor: params.cursor,
                limit: params.limit.unwrap_or(20).clamp(1, 100),
                include_total: params.include_total.unwrap_or(false),
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        },
    )
    .await?;

    Ok(Json(paginated(page, notification_response)))
}

pub async fn get_unread_count(
    State(state): State<AppState>,
    Query(params): Query<CountParams>,
) -> ApiResult<Json<UnreadCountResponse>> {
    let count = NotificationRepo::unread_count(&*state.db, params.project_id.as_deref()).await?;
    Ok(Json(UnreadCountResponse { count }))
}

pub async fn mark_read(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<NotificationResponse>> {
    let item = NotificationRepo::mark_read(&*state.db, &id).await?;
    Ok(Json(notification_response(item)))
}

pub async fn mark_all_read(
    State(state): State<AppState>,
    Query(params): Query<CountParams>,
) -> ApiResult<StatusCode> {
    NotificationRepo::mark_all_read(&*state.db, params.project_id.as_deref()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_notification(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    NotificationRepo::delete(&*state.db, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn notification_response(item: db::Notification) -> NotificationResponse {
    NotificationResponse {
        id: item.id,
        project_id: item.project_id,
        task_id: item.task_id,
        event_type: item.event_type,
        title: item.title,
        body: item.body,
        read: item.read,
        created_at: item.created_at,
    }
}
