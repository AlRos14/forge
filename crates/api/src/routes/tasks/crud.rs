use super::*;

pub async fn create_task(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<TaskResponse>> {
    let request: CreateTaskRequest = serde_json::from_value(body)?;
    let review_config = match request.review_config {
        Some(review_config) => Some(review_config),
        None => project_default_review_config(&state.db, &project_id).await?,
    };
    let review_config = serialize_json(
        review_config.map(|review_config| serde_json::json!({ "review": review_config })),
    )?;
    let task_type = request.task_type.map(|t| {
        match t {
            api_types::TaskType::Task => "task",
            api_types::TaskType::PlanningTask => "planning_task",
            api_types::TaskType::SubTask => "sub_task",
        }
        .to_owned()
    });
    let task = state
        .task_service
        .create_task(
            project_id,
            request.title,
            request.description,
            request.parent_task_id,
            request.priority,
            task_type,
            review_config,
            request.merge_config,
            request.role_assignments,
        )
        .await?;
    Ok(Json(task_response(&state.db, task).await?))
}

pub async fn list_tasks(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<PaginatedResponse<TaskResponse>>> {
    let page = TaskRepo::list(
        &*state.db,
        TaskListQuery {
            project_id,
            q: params.q.clone(),
            statuses: parse_csv::<db::TaskStatus>(params.status.as_ref(), "status")?,
            agent_ids: parse_csv::<String>(params.agent_id.as_ref(), "agent_id")?,
            assignee_types: parse_csv::<db::AssigneeKind>(
                params.assignee_type.as_ref(),
                "assignee_type",
            )?
            .into_iter()
            .map(|kind| kind.to_string())
            .collect(),
            assignee_ids: parse_csv::<String>(params.assignee_id.as_ref(), "assignee_id")?,
            priority: params.priority,
            include_archived: params.include_archived.unwrap_or(false),
            include_cancelled: params.include_cancelled.unwrap_or(false),
            include_deleted: false,
            page: task_page_request(&params)?,
        },
    )
    .await?;
    let has_more = page.next_cursor.is_some();
    let task_ids = page
        .items
        .iter()
        .map(|task| task.id.as_str())
        .collect::<Vec<_>>();
    let latest_reviews = ReviewRepo::list_latest_reviews_for_tasks(&*state.db, &task_ids)
        .await?
        .into_iter()
        .map(|review| (review.task_id.clone(), review))
        .collect::<std::collections::HashMap<_, _>>();
    let latest_executions = ExecutionRepo::list_latest_executions_for_tasks(&*state.db, &task_ids)
        .await?
        .into_iter()
        .map(|execution| (execution.task_id.clone(), execution))
        .collect::<std::collections::HashMap<_, _>>();
    let mut items = Vec::with_capacity(page.items.len());
    for task in page.items {
        let latest_review = latest_reviews.get(&task.id).cloned();
        let latest_execution = latest_executions.get(&task.id).cloned();
        items.push(
            task_response_light_with_latest(&state.db, task, latest_review, latest_execution)
                .await?,
        );
    }
    Ok(Json(PaginatedResponse {
        items,
        next_cursor: page.next_cursor,
        has_more,
        total_count: page.total_count.and_then(|count| u64::try_from(count).ok()),
    }))
}

pub async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskResponse>> {
    let awaiting_human = state.task_service.is_awaiting_human(id.clone()).await?;
    let task = TaskRepo::get_by_id(&*state.db, &id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", id))?;
    let response = task_response_with_awaiting_human(&state.db, task, awaiting_human).await?;
    Ok(Json(response))
}

pub async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateTaskRequest>,
) -> ApiResult<Json<TaskResponse>> {
    TaskRepo::update(
        &*state.db,
        UpdateTask {
            id: id.clone(),
            expected_version: request.version,
            title: request.title,
            description: request.description.map(Some),
            priority: request.priority,
            merge_config: serialize_json(request.merge_config)?.map(Some),
            plan: request.plan.map(Some),
            error_annotation: None,
            blocked_json: None,
            failed_json: None,
            task_state_config: serialize_json(request.task_state_config)?.map(Some),
            parent_task_id: request.parent_task_id,
            updated_at: now_rfc3339(),
        },
    )
    .await?;

    let task = TaskRepo::get_by_id(&*state.db, &id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", id.clone()))?;
    Ok(Json(task_response(&state.db, task).await?))
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let task = state.task_service.soft_delete(id).await?;
    super::media::delete_task_media_for_task(&state, &task.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn reorder_subtasks(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<ReorderSubtasksRequest>,
) -> ApiResult<Json<TaskResponse>> {
    state
        .task_service
        .reorder_subtasks(task_id.clone(), request.ordered_ids)
        .await?;
    let task = TaskRepo::get_by_id(&*state.db, &task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.clone()))?;
    Ok(Json(task_response(&state.db, task).await?))
}

pub async fn reorder_task_position(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PositionRequest>,
) -> ApiResult<Json<PositionResponse>> {
    let task = state
        .task_service
        .reorder_task(id, request.before_id, request.after_id)
        .await?;
    Ok(Json(PositionResponse {
        task: task_response(&state.db, task).await?,
    }))
}

pub async fn cancel_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskResponse>> {
    let task = state.task_service.cancel_task(id).await?;
    Ok(Json(task_response(&state.db, task).await?))
}

pub async fn archive_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskResponse>> {
    let task = state.task_service.archive_task(id).await?;
    Ok(Json(task_response(&state.db, task).await?))
}

pub async fn advance_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskResponse>> {
    let task = state.task_service.advance_to_next_state(id).await?;
    Ok(Json(task_response(&state.db, task).await?))
}

pub async fn recover_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RecoverTaskRequest>,
) -> ApiResult<Json<TaskResponse>> {
    let task = state
        .task_service
        .recover_task(id, body.action, body.reason, body.context)
        .await
        .map_err(|error| match &error {
            ServiceError::InvalidOperation { message } if message.contains("terminal status") => {
                ApiError::conflict_with_code("task.terminal", message.clone())
            }
            _ => ApiError::from(error),
        })?;
    Ok(Json(task_response(&state.db, task).await?))
}

pub async fn duplicate_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskResponse>> {
    let task = state.task_service.duplicate_task(&id).await?;
    Ok(Json(task_response(&state.db, task).await?))
}
