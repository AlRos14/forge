use api_types::{
    Actor, ActorRef, TaskDispatchActionRequest, TaskDispatchActionResponse,
    TaskLaunchPreflightResponse, TaskLaunchResponse, UserActionSource,
};
use axum::{
    extract::{Path, State},
    Json,
};
use db::{TaskLifecycleRepo, TaskRepo};
use services::task_lifecycle::TaskLifecycleService;
use services::ServiceError;

use crate::{
    errors::{ApiError, ApiResult},
    routes::{auth::AuthenticatedUser, task_response},
    state::AppState,
};

fn lifecycle_service(state: &AppState) -> TaskLifecycleService {
    TaskLifecycleService::new(
        std::sync::Arc::clone(&state.db),
        std::sync::Arc::clone(&state.event_bus),
    )
}

pub async fn get_launch_preflight(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<TaskLaunchPreflightResponse>> {
    crate::routes::tasks::require_task_visible(&state, &task_id, &user).await?;
    Ok(Json(
        lifecycle_service(&state)
            .launch_preflight(&task_id)
            .await
            .map_err(ApiError::from)?,
    ))
}

pub async fn launch_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<TaskDispatchActionRequest>,
) -> ApiResult<Json<TaskLaunchResponse>> {
    crate::routes::tasks::require_task_mutable(&state, &task_id, &user).await?;
    let requested_by = ActorRef::Human(user.user_id.clone());
    let write = lifecycle_service(&state)
        .launch_task(
            &task_id,
            request.expected_task_version,
            request.expected_lifecycle_version,
            &requested_by,
            &request.idempotency_key,
        )
        .await
        .map_err(ApiError::from)?;
    let outcome = if write.replayed {
        services::DispatchOutcome::default()
    } else if let Some(dispatcher) = state.task_dispatcher.as_ref() {
        dispatcher
            .dispatch_task_once(&task_id)
            .await
            .map_err(ApiError::from)?
    } else {
        services::DispatchOutcome::default()
    };
    let preflight = lifecycle_service(&state)
        .launch_preflight(&task_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(TaskLaunchResponse {
        admitted: true,
        dispatch: api_types::TaskDispatchIntentResponse {
            task_id: write.dispatch.task_id,
            state: match write.dispatch.state {
                db::TaskDispatchState::Idle => api_types::TaskDispatchState::Idle,
                db::TaskDispatchState::Running => api_types::TaskDispatchState::Running,
                db::TaskDispatchState::Paused => api_types::TaskDispatchState::Paused,
                db::TaskDispatchState::Cancelled => api_types::TaskDispatchState::Cancelled,
            },
            version: write.dispatch.version,
            requested_by: match (
                write.dispatch.requested_by_kind.as_str(),
                write.dispatch.requested_by_id.as_deref(),
            ) {
                ("human", Some(id)) => Some(ActorRef::Human(id.to_owned())),
                ("agent", Some(id)) => Some(ActorRef::Agent(id.to_owned())),
                _ => None,
            },
            reason: write.dispatch.reason,
            created_at: write.dispatch.created_at,
            updated_at: write.dispatch.updated_at,
        },
        preflight,
        admitted_execution_ids: outcome.admitted_execution_ids,
        started_execution_ids: outcome.started_execution_ids,
    }))
}

pub async fn pause_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<TaskDispatchActionRequest>,
) -> ApiResult<Json<TaskDispatchActionResponse>> {
    crate::routes::tasks::require_task_mutable(&state, &task_id, &user).await?;
    let write = lifecycle_service(&state)
        .pause_task(
            &task_id,
            request.expected_task_version,
            request.expected_lifecycle_version,
            &ActorRef::Human(user.user_id.clone()),
            request.reason,
            &request.idempotency_key,
        )
        .await
        .map_err(ApiError::from)?;
    action_response(&state, &task_id, write).await
}

pub async fn resume_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<TaskDispatchActionRequest>,
) -> ApiResult<Json<TaskDispatchActionResponse>> {
    crate::routes::tasks::require_task_mutable(&state, &task_id, &user).await?;
    let write = lifecycle_service(&state)
        .resume_task(
            &task_id,
            request.expected_task_version,
            request.expected_lifecycle_version,
            &ActorRef::Human(user.user_id.clone()),
            &request.idempotency_key,
        )
        .await
        .map_err(ApiError::from)?;
    if !write.replayed {
        if let Some(dispatcher) = state.task_dispatcher.as_ref() {
            match dispatcher.dispatch_task_once(&task_id).await {
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(task_id = %task_id, %error, "resume dispatch failed");
                }
            }
        }
    }
    action_response(&state, &task_id, write).await
}

pub async fn cancel_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<TaskDispatchActionRequest>,
) -> ApiResult<Json<TaskDispatchActionResponse>> {
    crate::routes::tasks::require_task_mutable(&state, &task_id, &user).await?;
    let task = TaskRepo::get_by_id(&*state.db, &task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.clone()))?;
    if let Some(receipt) =
        TaskLifecycleRepo::get_task_dispatch_receipt(&*state.db, &task_id, &request.idempotency_key)
            .await?
    {
        if receipt.operation != db::TaskDispatchOperation::Cancel {
            return Err(ApiError::conflict_with_code(
                "task_dispatch.idempotency_conflict",
                "Task dispatch idempotency key belongs to another operation",
            ));
        }
        let write = lifecycle_service(&state)
            .record_cancelled_dispatch_after_cancel(
                &task_id,
                request.expected_task_version,
                request.expected_lifecycle_version,
                &ActorRef::Human(user.user_id.clone()),
                request.reason,
                &request.idempotency_key,
            )
            .await
            .map_err(ApiError::from)?;
        return action_response(&state, &task_id, write).await;
    }
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*state.db, &task_id)
        .await?
        .ok_or_else(|| ApiError::not_found("task lifecycle", task_id.clone()))?;
    if task.version != request.expected_task_version {
        return Err(ApiError::from(ServiceError::Db(
            db::DbError::TaskVersionConflict {
                expected: request.expected_task_version,
                actual: task.version,
            },
        )));
    }
    if lifecycle.version != request.expected_lifecycle_version {
        return Err(ApiError::from(ServiceError::Db(
            db::DbError::VersionConflict,
        )));
    }
    let actor = Actor::User {
        user_id: Some(user.user_id.clone()),
        source: UserActionSource::Api,
    };
    state
        .task_service
        .cancel_task_as(task_id.clone(), actor)
        .await
        .map_err(ApiError::from)?;
    let write = lifecycle_service(&state)
        .record_cancelled_dispatch_after_cancel(
            &task_id,
            task.version,
            lifecycle.version,
            &ActorRef::Human(user.user_id.clone()),
            request.reason,
            &request.idempotency_key,
        )
        .await
        .map_err(ApiError::from)?;
    action_response(&state, &task_id, write).await
}

async fn action_response(
    state: &AppState,
    task_id: &str,
    write: db::TaskDispatchWrite,
) -> ApiResult<Json<TaskDispatchActionResponse>> {
    let task = TaskRepo::get_by_id(&*state.db, task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.to_owned()))?;
    let task = task_response(&state.db, task).await?;
    let preflight = lifecycle_service(state)
        .launch_preflight(task_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(TaskDispatchActionResponse {
        task,
        dispatch: api_types::TaskDispatchIntentResponse {
            task_id: write.dispatch.task_id,
            state: match write.dispatch.state {
                db::TaskDispatchState::Idle => api_types::TaskDispatchState::Idle,
                db::TaskDispatchState::Running => api_types::TaskDispatchState::Running,
                db::TaskDispatchState::Paused => api_types::TaskDispatchState::Paused,
                db::TaskDispatchState::Cancelled => api_types::TaskDispatchState::Cancelled,
            },
            version: write.dispatch.version,
            requested_by: match (
                write.dispatch.requested_by_kind.as_str(),
                write.dispatch.requested_by_id.as_deref(),
            ) {
                ("human", Some(id)) => Some(ActorRef::Human(id.to_owned())),
                ("agent", Some(id)) => Some(ActorRef::Agent(id.to_owned())),
                _ => None,
            },
            reason: write.dispatch.reason,
            created_at: write.dispatch.created_at,
            updated_at: write.dispatch.updated_at,
        },
        preflight,
    }))
}
