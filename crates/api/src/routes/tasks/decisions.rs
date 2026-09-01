use super::*;
use api_types::{AnswerTaskDecisionRequest, TaskDecisionRequestResponse};
use sqlx::Row;

pub async fn list_task_decisions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<TaskDecisionRequestResponse>>> {
    TaskRepo::get_by_id(&*state.db, &id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", id.clone()))?;
    let rows = sqlx::query(
        "SELECT id, task_id, execution_id, role, authority_scope, questions_json,
                context, status, created_at
         FROM task_decision_request WHERE task_id = ? ORDER BY created_at DESC",
    )
    .bind(&id)
    .fetch_all(state.db.pool())
    .await?;
    Ok(Json(rows.into_iter().map(decision_response).collect()))
}

pub async fn answer_task_decision(
    State(state): State<AppState>,
    user: crate::routes::auth::AuthenticatedUser,
    Path((task_id, request_id)): Path<(String, String)>,
    Json(request): Json<AnswerTaskDecisionRequest>,
) -> ApiResult<Json<TaskDecisionRequestResponse>> {
    let row = sqlx::query(
        "SELECT id, task_id, execution_id, role, authority_scope, questions_json,
                context, status, created_at
         FROM task_decision_request WHERE id = ? AND task_id = ?",
    )
    .bind(&request_id)
    .bind(&task_id)
    .fetch_optional(state.db.pool())
    .await?
    .ok_or_else(|| ApiError::not_found("task_decision_request", request_id.clone()))?;
    if row.get::<String, _>("status") != "pending" {
        return Err(ApiError::invalid_operation_conflict(
            "decision request is not pending",
        ));
    }
    if row.get::<String, _>("authority_scope") != "task" {
        return Err(ApiError::invalid_operation_conflict(
            "this answer requires a Project Decision and baseline reconciliation; Task prose cannot change Project authority",
        ));
    }
    let questions: Value = serde_json::from_str(&row.get::<String, _>("questions_json"))
        .map_err(|_| ApiError::bad_request("stored decision questions are invalid"))?;
    let answers_complete = request.answers.as_object().is_some_and(|answers| {
        questions.as_array().is_some_and(|questions| {
            !questions.is_empty()
                && questions.iter().enumerate().all(|(index, question)| {
                    let key = question
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| index.to_string());
                    answers.get(&key).is_some_and(answer_has_content)
                })
        })
    });
    if !answers_complete {
        return Err(ApiError::bad_request(
            "answers must be a non-empty object with a value for every question",
        ));
    }
    let execution_id: String = row.get("execution_id");
    let now = now_rfc3339();
    let mut tx = state.db.pool().begin().await?;
    let claimed = sqlx::query(
        "UPDATE task_decision_request SET status = 'answered' WHERE id = ? AND status = 'pending'",
    )
    .bind(&request_id)
    .execute(&mut *tx)
    .await?;
    if claimed.rows_affected() != 1 {
        return Err(ApiError::invalid_operation_conflict(
            "decision request is not pending",
        ));
    }
    sqlx::query(
        "INSERT INTO task_decision_answer
         (id, request_id, principal_type, principal_id, answers_json, answered_at)
         VALUES (?, ?, 'user', ?, ?, ?)",
    )
    .bind(db::new_uuid_v4())
    .bind(&request_id)
    .bind(&user.user_id)
    .bind(request.answers.to_string())
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    state.task_service.dispatch_role_follow_up(
        &task_id,
        services::workflow::default_roles::PLANNER,
        execution_id,
        format!("The authorized user answered the pending questions:\n{}\n\nRevise the plan and emit a new FORGE_RESULT.", request.answers),
        "decision_answered",
    ).await?;
    let updated = sqlx::query(
        "SELECT id, task_id, execution_id, role, authority_scope, questions_json,
                context, status, created_at FROM task_decision_request WHERE id = ?",
    )
    .bind(&request_id)
    .fetch_one(state.db.pool())
    .await?;
    Ok(Json(decision_response(updated)))
}

fn answer_has_content(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::String(value) => !value.trim().is_empty(),
        _ => true,
    }
}

fn decision_response(row: sqlx::sqlite::SqliteRow) -> TaskDecisionRequestResponse {
    let raw: String = row.get("questions_json");
    TaskDecisionRequestResponse {
        id: row.get("id"),
        task_id: row.get("task_id"),
        execution_id: row.get("execution_id"),
        role: row.get("role"),
        authority_scope: row.get("authority_scope"),
        questions: serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!([])),
        context: row.get("context"),
        status: row.get("status"),
        created_at: row.get("created_at"),
    }
}
