use api_types::{
    AddWorkUnitDependencyRequest, AllocateWorkUnitRequest, CreateWorkUnitRequest,
    TransitionWorkUnitRequest, UpdateWorkUnitRequest, WorkUnitDependencyResponse,
    WorkUnitIntegrationOutcome, WorkUnitIntegrationRequest, WorkUnitIntegrationResponse,
    WorkUnitProvenance, WorkUnitProvenanceKind, WorkUnitReadinessResponse, WorkUnitResponse,
    WorkUnitStatus as ApiWorkUnitStatus,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use db::{ActorRef, WorkUnit, WorkUnitDependency, WorkUnitIntegration};
use services::CollaborationActorSource;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};

fn actor_to_db(actor: api_types::ActorRef) -> ActorRef {
    match actor {
        api_types::ActorRef::Human(id) => ActorRef::Human(id),
        api_types::ActorRef::Agent(id) => ActorRef::Agent(id),
    }
}

fn actor_to_api(actor: ActorRef) -> api_types::ActorRef {
    match actor {
        ActorRef::Human(id) => api_types::ActorRef::Human(id),
        ActorRef::Agent(id) => api_types::ActorRef::Agent(id),
    }
}

fn status_to_api(status: db::WorkUnitStatus) -> ApiWorkUnitStatus {
    match status {
        db::WorkUnitStatus::Open => ApiWorkUnitStatus::Open,
        db::WorkUnitStatus::Completed => ApiWorkUnitStatus::Completed,
        db::WorkUnitStatus::Cancelled => ApiWorkUnitStatus::Cancelled,
    }
}

fn status_to_db(status: ApiWorkUnitStatus) -> db::WorkUnitStatus {
    match status {
        ApiWorkUnitStatus::Open => db::WorkUnitStatus::Open,
        ApiWorkUnitStatus::Completed => db::WorkUnitStatus::Completed,
        ApiWorkUnitStatus::Cancelled => db::WorkUnitStatus::Cancelled,
    }
}

fn readiness_to_api(readiness: services::WorkUnitReadiness) -> WorkUnitReadinessResponse {
    WorkUnitReadinessResponse {
        runnable: readiness.runnable,
        ready_for_allocation: readiness.ready_for_allocation,
        active_execution_ids: readiness.active_execution_ids,
        unsatisfied_dependency_ids: readiness.unsatisfied_dependency_ids,
        awaiting_integration: readiness.awaiting_integration,
    }
}

fn work_unit_response(unit: WorkUnit, readiness: services::WorkUnitReadiness) -> WorkUnitResponse {
    WorkUnitResponse {
        id: unit.id,
        task_id: unit.task_id,
        parent_work_unit_id: unit.parent_work_unit_id,
        title: unit.title,
        scope: unit.scope,
        status: status_to_api(unit.status),
        role: unit.role,
        assigned_actor: unit.assigned_actor.map(actor_to_api),
        requires_integration: unit.requires_integration,
        provenance: unit
            .provenance_kind
            .zip(unit.provenance_id)
            .map(|(kind, id)| WorkUnitProvenance {
                kind: match kind.as_str() {
                    "actor" => WorkUnitProvenanceKind::Actor,
                    "work_unit" => WorkUnitProvenanceKind::WorkUnit,
                    "artifact" => WorkUnitProvenanceKind::Artifact,
                    _ => WorkUnitProvenanceKind::External,
                },
                id,
            }),
        created_by: actor_to_api(unit.created_by),
        version: unit.version,
        created_at: unit.created_at,
        updated_at: unit.updated_at,
        readiness: readiness_to_api(readiness),
    }
}

async fn response_for(
    state: &AppState,
    user: &AuthenticatedUser,
    unit: WorkUnit,
) -> ApiResult<WorkUnitResponse> {
    let readiness = state
        .work_unit_service
        .readiness(
            CollaborationActorSource::Human(user.user_id.clone()),
            &unit.id,
        )
        .await
        .map_err(ApiError::from)?;
    Ok(work_unit_response(unit, readiness))
}

pub async fn create_work_unit(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<CreateWorkUnitRequest>,
) -> ApiResult<(StatusCode, Json<WorkUnitResponse>)> {
    let provenance = body.provenance.map(|value| {
        (
            match value.kind {
                WorkUnitProvenanceKind::Actor => "actor",
                WorkUnitProvenanceKind::WorkUnit => "work_unit",
                WorkUnitProvenanceKind::Artifact => "artifact",
                WorkUnitProvenanceKind::External => "external",
            }
            .to_owned(),
            value.id,
        )
    });
    let unit = state
        .work_unit_service
        .create(
            CollaborationActorSource::Human(user.user_id.clone()),
            services::CreateWorkUnitInput {
                task_id,
                title: body.title,
                scope: body.scope,
                role: body.role,
                parent_work_unit_id: body.parent_work_unit_id,
                assigned_actor: body.assigned_actor.map(actor_to_db),
                requires_integration: body.requires_integration,
                provenance,
            },
        )
        .await
        .map_err(ApiError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(response_for(&state, &user, unit).await?),
    ))
}

pub async fn list_work_units(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<Vec<WorkUnitResponse>>> {
    let units = state
        .work_unit_service
        .list(
            CollaborationActorSource::Human(user.user_id.clone()),
            &task_id,
        )
        .await
        .map_err(ApiError::from)?;
    let mut result = Vec::with_capacity(units.len());
    for unit in units {
        result.push(response_for(&state, &user, unit).await?);
    }
    Ok(Json(result))
}

pub async fn get_work_unit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<WorkUnitResponse>> {
    let unit = state
        .work_unit_service
        .get(CollaborationActorSource::Human(user.user_id.clone()), &id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(response_for(&state, &user, unit).await?))
}

pub async fn update_work_unit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<UpdateWorkUnitRequest>,
) -> ApiResult<Json<WorkUnitResponse>> {
    let unit = state
        .work_unit_service
        .update(
            CollaborationActorSource::Human(user.user_id.clone()),
            &id,
            body.expected_version,
            body.title,
            body.scope,
            body.requires_integration,
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(response_for(&state, &user, unit).await?))
}

pub async fn allocate_work_unit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<AllocateWorkUnitRequest>,
) -> ApiResult<Json<WorkUnitResponse>> {
    let unit = state
        .work_unit_service
        .allocate(
            CollaborationActorSource::Human(user.user_id.clone()),
            &id,
            body.expected_version,
            body.role,
            body.assigned_actor.map(actor_to_db),
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(response_for(&state, &user, unit).await?))
}

pub async fn transition_work_unit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<TransitionWorkUnitRequest>,
) -> ApiResult<Json<WorkUnitResponse>> {
    let unit = state
        .work_unit_service
        .transition(
            CollaborationActorSource::Human(user.user_id.clone()),
            &id,
            body.expected_version,
            status_to_db(body.status),
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(response_for(&state, &user, unit).await?))
}

pub async fn add_dependency(
    State(state): State<AppState>,
    Path((id, prerequisite_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(body): Json<AddWorkUnitDependencyRequest>,
) -> ApiResult<(StatusCode, Json<WorkUnitDependencyResponse>)> {
    let dependency = state
        .work_unit_service
        .add_dependency(
            CollaborationActorSource::Human(user.user_id.clone()),
            &id,
            &prerequisite_id,
            body.expected_version,
        )
        .await
        .map_err(ApiError::from)?;
    Ok((StatusCode::CREATED, Json(dependency_response(dependency))))
}

pub async fn remove_dependency(
    State(state): State<AppState>,
    Path((id, prerequisite_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(body): Json<AddWorkUnitDependencyRequest>,
) -> ApiResult<StatusCode> {
    state
        .work_unit_service
        .remove_dependency(
            CollaborationActorSource::Human(user.user_id.clone()),
            &id,
            &prerequisite_id,
            body.expected_version,
        )
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_dependencies(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<Vec<WorkUnitDependencyResponse>>> {
    let dependencies = state
        .work_unit_service
        .dependencies(CollaborationActorSource::Human(user.user_id.clone()), &id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(
        dependencies.into_iter().map(dependency_response).collect(),
    ))
}

pub async fn readiness(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<WorkUnitReadinessResponse>> {
    let readiness = state
        .work_unit_service
        .readiness(CollaborationActorSource::Human(user.user_id.clone()), &id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(readiness_to_api(readiness)))
}

pub async fn integrate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<WorkUnitIntegrationRequest>,
) -> ApiResult<Json<WorkUnitIntegrationResponse>> {
    let integration = state
        .work_unit_service
        .integrate(
            CollaborationActorSource::Human(user.user_id.clone()),
            &id,
            &body.execution_id,
            &body.idempotency_key,
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(integration_response(integration)))
}

fn dependency_response(dependency: WorkUnitDependency) -> WorkUnitDependencyResponse {
    WorkUnitDependencyResponse {
        work_unit_id: dependency.work_unit_id,
        depends_on_work_unit_id: dependency.depends_on_work_unit_id,
        satisfied: dependency.satisfied,
        created_at: dependency.created_at,
    }
}

fn integration_response(integration: WorkUnitIntegration) -> WorkUnitIntegrationResponse {
    WorkUnitIntegrationResponse {
        id: integration.id,
        task_id: integration.task_id,
        work_unit_id: integration.work_unit_id,
        execution_id: integration.execution_id,
        source_workspace_id: integration.source_workspace_id,
        source_sha: integration.source_sha,
        target_workspace_id: integration.target_workspace_id,
        target_before_sha: integration.target_before_sha,
        target_after_sha: integration.target_after_sha,
        outcome: match integration.outcome {
            db::WorkUnitIntegrationOutcome::Running => WorkUnitIntegrationOutcome::Running,
            db::WorkUnitIntegrationOutcome::Success => WorkUnitIntegrationOutcome::Success,
            db::WorkUnitIntegrationOutcome::Conflict => WorkUnitIntegrationOutcome::Conflict,
            db::WorkUnitIntegrationOutcome::Failed => WorkUnitIntegrationOutcome::Failed,
            db::WorkUnitIntegrationOutcome::Rejected => WorkUnitIntegrationOutcome::Rejected,
        },
        version: integration.version,
        started_at: integration.started_at,
        finished_at: integration.finished_at,
    }
}
