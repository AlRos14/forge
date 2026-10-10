use super::*;

pub async fn create_task(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Json<TaskResponse>> {
    let request: CreateTaskRequest = serde_json::from_value(body)?;
    let task_type = request.task_type.map(|t| {
        match t {
            api_types::TaskType::Implementation => "implementation",
            api_types::TaskType::Planning => "planning",
            api_types::TaskType::Discovery => "discovery",
            api_types::TaskType::Review => "review",
            api_types::TaskType::Validation => "validation",
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
            None,
            None,
            None,
        )
        .await?;
    Ok(Json(task_response(&state.db, task).await?))
}

pub async fn list_tasks(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<TasksResponse>> {
    if params.status.is_some() {
        return Err(ApiError::bad_request(
            "Task status filters were retired; use lifecycle_state",
        ));
    }
    if params.include_cancelled.is_some() {
        return Err(ApiError::bad_request(
            "include_cancelled was retired; filter with lifecycle_state=cancelled",
        ));
    }
    if params.include_archived.is_some() {
        return Err(ApiError::bad_request(
            "Task archiving is not part of TaskLifecycle",
        ));
    }
    let lifecycle_states =
        parse_csv::<db::TaskLifecycleState>(params.lifecycle_state.as_ref(), "lifecycle_state")?;

    // Task decoration performs additional reads, so bracket the assembled page with
    // revision reads and retry if a board mutation races the response.
    for _ in 0..3 {
        let board_revision = TaskBoardRepo::board_revision(&*state.db, &project_id).await?;
        let page = TaskRepo::list(
            &*state.db,
            TaskListQuery {
                project_id: project_id.clone(),
                q: params.q.clone(),
                lifecycle_states: lifecycle_states.clone(),
                statuses: Vec::new(),
                agent_ids: Vec::new(),
                assignee_types: Vec::new(),
                assignee_ids: Vec::new(),
                priority: params.priority,
                include_archived: false,
                include_cancelled: true,
                include_deleted: false,
                page: task_page_request(&params)?,
            },
        )
        .await?;
        let has_more = page.next_cursor.is_some();
        let mut items = Vec::with_capacity(page.items.len());
        for task in page.items {
            items.push(task_response_light(&state.db, task).await?);
        }
        let current_revision = TaskBoardRepo::board_revision(&*state.db, &project_id).await?;
        if current_revision == board_revision {
            return Ok(Json(TasksResponse {
                items,
                next_cursor: page.next_cursor,
                has_more,
                total_count: page.total_count.and_then(|count| u64::try_from(count).ok()),
                board_revision,
            }));
        }
    }

    Err(ApiError::conflict_with_code(
        "board_snapshot_changed",
        "board changed while the task page was assembled; retry the request",
    ))
}

pub async fn get_task(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskResponse>> {
    let task = require_task_visible(&state, &id, &user).await?;
    let response = task_response(&state.db, task).await?;
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
            merge_config: None,
            error_annotation: None,
            blocked_json: None,
            failed_json: None,
            task_state_config: None,
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
