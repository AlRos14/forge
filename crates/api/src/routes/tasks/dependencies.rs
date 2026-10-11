use super::*;

pub async fn add_dependency(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<AddDependencyRequest>,
) -> ApiResult<StatusCode> {
    require_same_project_tasks(&state, &id, &request.depends_on_id, &user).await?;
    TaskDependencyRepo::add_dependency(&*state.db, &id, &request.depends_on_id, &now_rfc3339())
        .await?;
    Ok(StatusCode::CREATED)
}

pub async fn remove_dependency(
    State(state): State<AppState>,
    Path((id, dep_id)): Path<(String, String)>,
    user: AuthenticatedUser,
) -> ApiResult<StatusCode> {
    require_same_project_tasks(&state, &id, &dep_id, &user).await?;
    TaskDependencyRepo::remove_dependency(&*state.db, &id, &dep_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_dependencies(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<Vec<TaskDependency>>> {
    require_task_visible(&state, &id, &user).await?;
    let dependencies = TaskDependencyRepo::list_dependencies(&*state.db, &id).await?;
    for dependency_id in &dependencies {
        require_same_project_tasks(&state, &id, dependency_id, &user).await?;
    }
    Ok(Json(
        dependencies_with_created_at(&state.db, id, dependencies).await?,
    ))
}

pub async fn list_dependents(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<Vec<TaskDependency>>> {
    require_task_visible(&state, &id, &user).await?;
    let dependents = TaskDependencyRepo::list_dependents(&*state.db, &id).await?;
    for dependent_id in &dependents {
        require_same_project_tasks(&state, dependent_id, &id, &user).await?;
    }
    Ok(Json(
        dependents_with_created_at(&state.db, id, dependents).await?,
    ))
}

async fn require_same_project_tasks(
    state: &AppState,
    task_id: &str,
    related_task_id: &str,
    user: &AuthenticatedUser,
) -> ApiResult<()> {
    let task = require_task_visible(state, task_id, user).await?;
    let related = require_task_visible(state, related_task_id, user).await?;
    if task.project_id != related.project_id {
        return Err(ApiError::not_found("task", related_task_id.to_owned()));
    }
    Ok(())
}

async fn dependencies_with_created_at(
    db: &db::SqliteDb,
    task_id: String,
    dependencies: Vec<String>,
) -> ApiResult<Vec<TaskDependency>> {
    let mut response = Vec::with_capacity(dependencies.len());
    for depends_on_id in dependencies {
        let created_at = dependency_created_at(db, &task_id, &depends_on_id).await?;
        response.push(TaskDependency {
            task_id: task_id.clone(),
            depends_on_id,
            created_at,
        });
    }
    Ok(response)
}

async fn dependents_with_created_at(
    db: &db::SqliteDb,
    depends_on_id: String,
    dependents: Vec<String>,
) -> ApiResult<Vec<TaskDependency>> {
    let mut response = Vec::with_capacity(dependents.len());
    for task_id in dependents {
        let created_at = dependency_created_at(db, &task_id, &depends_on_id).await?;
        response.push(TaskDependency {
            task_id,
            depends_on_id: depends_on_id.clone(),
            created_at,
        });
    }
    Ok(response)
}

async fn dependency_created_at(
    db: &db::SqliteDb,
    task_id: &str,
    depends_on_id: &str,
) -> ApiResult<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT created_at FROM task_dependency WHERE task_id = ? AND depends_on_id = ?",
    )
    .bind(task_id)
    .bind(depends_on_id)
    .fetch_one(db.pool())
    .await
    .map_err(db::DbError::from)
    .map_err(ApiError::from)
}
