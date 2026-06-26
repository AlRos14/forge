use api_types::{MemoryGetQuery, MemorySearchQuery, MemorySearchResponse, MemorySearchResultDto};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::{MemoryItem, MemoryRepository, ProjectMemberRepo, ProjectRepo};
use services::MemorySearchResult;
use uuid::Uuid;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};

pub async fn search_project_memory(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(project_id): Path<String>,
    Query(params): Query<MemorySearchQuery>,
) -> ApiResult<Json<MemorySearchResponse>> {
    if params.query.trim().is_empty() {
        return Err(ApiError::bad_request("query must not be empty"));
    }
    let project_uuid = parse_uuid(&project_id, "project_id")?;
    let normalized_project_id = project_uuid.to_string();
    require_project_visible(&state, &normalized_project_id, &user).await?;
    let layer = response_layer(params.layer, params.token_budget)?;
    let limit = params.limit.unwrap_or(20);
    let (results, has_more, next_cursor) = state
        .memory_service
        .search(
            project_uuid,
            params.query,
            params.layer,
            params.token_budget,
            limit,
            params.cursor,
        )
        .await?;

    let mut items = Vec::with_capacity(results.len());
    for (index, result) in results.into_iter().enumerate() {
        let raw = memory_item_for_result(&state, &result).await?;
        if raw.project_id != normalized_project_id {
            return Err(ApiError::not_found("memory_item", result.id.to_string()));
        }
        items.push(memory_result_dto(
            result,
            raw,
            layer,
            relevance_score(index),
        ));
    }

    Ok(Json(MemorySearchResponse {
        items,
        has_more,
        next_cursor,
    }))
}

pub async fn get_memory_item(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
    Query(params): Query<MemoryGetQuery>,
) -> ApiResult<Json<MemorySearchResultDto>> {
    let item_uuid = parse_uuid(&id, "id")?;
    let layer = response_layer(params.layer, None)?;
    let result = state.memory_service.get(item_uuid, params.layer).await?;
    let raw = memory_item_for_result(&state, &result).await?;
    require_project_visible(&state, &raw.project_id, &user).await?;
    Ok(Json(memory_result_dto(result, raw, layer, 1.0)))
}

async fn memory_item_for_result(
    state: &AppState,
    result: &MemorySearchResult,
) -> ApiResult<MemoryItem> {
    let id = result.id.to_string();
    MemoryRepository::get_memory_item(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("memory_item", id))
}

fn memory_result_dto(
    result: MemorySearchResult,
    raw: MemoryItem,
    layer: u8,
    score: f32,
) -> MemorySearchResultDto {
    let source_id = source_ref_from_metadata(&raw.metadata_json).unwrap_or_else(|| raw.id.clone());
    let creator = creator_from_item(&raw);
    MemorySearchResultDto {
        id: result.id.to_string(),
        layer,
        content: result.body.or(result.summary).unwrap_or(result.title),
        score,
        source_type: result.kind.to_string(),
        source_id,
        project_id: raw.project_id,
        task_id: raw.task_id,
        created_at: raw.created_at,
        creator,
    }
}

fn response_layer(layer: Option<u8>, token_budget: Option<u32>) -> ApiResult<u8> {
    match layer {
        Some(value @ 1..=3) => Ok(value),
        Some(other) => Err(ApiError::bad_request(format!(
            "invalid memory layer {other}; expected 1, 2, or 3"
        ))),
        None => Ok(match token_budget {
            Some(budget) if budget < 200 => 1,
            Some(budget) if budget <= 1000 => 2,
            _ => 3,
        }),
    }
}

fn relevance_score(index: usize) -> f32 {
    1.0 / (index as f32 + 1.0)
}

fn source_ref_from_metadata(metadata_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(metadata_json)
        .ok()
        .and_then(|value| {
            value
                .get("source_ref")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
}

fn creator_from_item(item: &MemoryItem) -> Option<String> {
    item.created_by_id
        .clone()
        .or_else(|| item.created_by_type.clone())
}

fn parse_uuid(value: &str, field: &'static str) -> ApiResult<Uuid> {
    Uuid::parse_str(value)
        .map_err(|error| ApiError::bad_request(format!("invalid {field} UUID: {error}")))
}

async fn require_project_visible(
    state: &AppState,
    project_id: &str,
    user: &AuthenticatedUser,
) -> ApiResult<()> {
    let project = ProjectRepo::get_by_id(&*state.db, project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", project_id.to_owned()))?;
    if project.owner_id.is_none() {
        return Ok(());
    }
    let member = ProjectMemberRepo::get_member(&*state.db, project_id, &user.user_id).await?;
    if member.is_none() {
        return Err(ApiError::not_found("project", project_id.to_owned()));
    }
    Ok(())
}
