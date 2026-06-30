use super::*;

pub async fn create_comment(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CreateCommentRequest>,
) -> ApiResult<(StatusCode, Json<CommentResponse>)> {
    let content = request.content.trim();
    if content.is_empty() {
        return Err(ApiError::bad_request("content must not be empty"));
    }
    let author_name = request.author_name.trim();
    if author_name.is_empty() {
        return Err(ApiError::bad_request("author_name must not be empty"));
    }

    let comment = state
        .task_service
        .add_user_comment(&id, author_name.to_owned(), content.to_owned())
        .await?;

    Ok((StatusCode::CREATED, Json(comment_response(comment))))
}

pub async fn list_comments(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<PaginatedResponse<CommentResponse>>> {
    let comments = TaskCommentRepo::list_comments(
        &*state.db,
        &id,
        PageRequest {
            cursor: params.cursor,
            limit: params.limit.unwrap_or(50).clamp(1, 100),
            include_total: params.include_total.unwrap_or(false),
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Asc,
        },
    )
    .await?;
    Ok(Json(paginated(comments, comment_response)))
}

pub async fn delete_comment(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let comment = TaskCommentRepo::get_comment_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("comment", id.clone()))?;
    if comment.author_type != CommentAuthorType::User {
        return Err(ApiError::forbidden_with_code(
            "comment.delete_forbidden",
            "only user comments can be deleted",
        ));
    }
    TaskCommentRepo::delete_comment(&*state.db, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
