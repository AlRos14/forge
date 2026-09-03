use super::*;
use api_types::{TaskPlanHistoryResponse, TaskPlanRevisionSummary};
use sqlx::Row;

pub async fn get_task_plan(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskPlanHistoryResponse>> {
    require_task_visible(&state, &id, &user).await?;
    let current = services::plan_artifact::latest_plan_for_task(&state.db, &id)
        .await
        .map_err(|error| match error {
            services::plan_artifact::PlanArtifactError::DbError(error) => ApiError::from(error),
            other => ApiError::bad_request(other.to_string()),
        })?;
    let rows = sqlx::query(
        "SELECT id, revision, checkpoint, content_digest, created_at
         FROM task_plan_revision WHERE task_id = ? ORDER BY revision DESC",
    )
    .bind(&id)
    .fetch_all(state.db.pool())
    .await?;
    let revisions = rows
        .into_iter()
        .map(|row| TaskPlanRevisionSummary {
            id: row.get("id"),
            revision: row.get("revision"),
            checkpoint: row.get("checkpoint"),
            content_digest: row.get("content_digest"),
            created_at: row.get("created_at"),
        })
        .collect();
    Ok(Json(TaskPlanHistoryResponse { current, revisions }))
}
