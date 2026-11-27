use api_types::{
    AddWorkUnitDependencyRequest, AllocateWorkUnitRequest, CreateWorkUnitProvenance,
    CreateWorkUnitRequest, TransitionWorkUnitRequest, UpdateWorkUnitRequest,
    WorkUnitDependencyResponse, WorkUnitIntegrationOutcome, WorkUnitIntegrationRequest,
    WorkUnitIntegrationResponse, WorkUnitProvenance, WorkUnitReadinessResponse,
    WorkUnitRepositoryIntegrationResponse, WorkUnitResponse, WorkUnitStatus as ApiWorkUnitStatus,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use db::{
    ActorRef, TaskRepositorySetRepo, WorkUnit, WorkUnitDependency, WorkUnitIntegration,
    WorkUnitRepo, WorkUnitRepositoryIntegration, WorkUnitRepositoryIntegrationRepo,
    WorkUnitWorkspaceRepo, WorkspaceRepo,
};
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
        awaiting_acceptance: readiness.awaiting_acceptance,
        awaiting_review_approval: readiness.awaiting_review_approval,
    }
}

fn work_unit_response(
    unit: WorkUnit,
    readiness: services::WorkUnitReadiness,
    review_subject: Option<api_types::ReviewSubjectResponse>,
    repository_ids: Vec<String>,
    integrations: Vec<WorkUnitRepositoryIntegration>,
) -> WorkUnitResponse {
    WorkUnitResponse {
        id: unit.id,
        task_id: unit.task_id,
        parent_work_unit_id: unit.parent_work_unit_id,
        title: unit.title,
        scope: unit.scope,
        execution_instructions: unit.execution_instructions,
        status: status_to_api(unit.status),
        role: unit.role,
        assigned_actor: unit.assigned_actor.map(actor_to_api),
        requires_integration: unit.requires_integration,
        provenance: unit.provenance.map(|provenance| match provenance {
            db::WorkUnitProvenance::Actor(actor) => WorkUnitProvenance::Actor {
                actor: actor_to_api(actor),
            },
            db::WorkUnitProvenance::WorkUnit(id) => WorkUnitProvenance::WorkUnit { id },
            db::WorkUnitProvenance::Artifact(id) => WorkUnitProvenance::Artifact { id },
            db::WorkUnitProvenance::External(id) => WorkUnitProvenance::External { id },
            db::WorkUnitProvenance::LegacyActor(id) => WorkUnitProvenance::LegacyActor { id },
        }),
        review_approval_gate_id: unit.review_approval_gate_id,
        review_approval_line_id: unit.review_approval_line_id,
        created_by: actor_to_api(unit.created_by),
        version: unit.version,
        created_at: unit.created_at,
        updated_at: unit.updated_at,
        readiness: readiness_to_api(readiness),
        review_subject,
        review_range: None,
        repository_ids,
        integrations: integrations
            .into_iter()
            .map(repository_integration_response)
            .collect(),
    }
}

pub(crate) async fn response_for(
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
    let review_subject = services::task_rework::get_review_subject(&state.db, &unit.id)
        .await
        .map_err(ApiError::from)?
        .as_ref()
        .map(services::task_rework::review_subject_response);
    let repository_ids = db::WorkUnitRepo::list_repository_ids(&*state.db, &unit.id).await?;
    let integrations =
        WorkUnitRepositoryIntegrationRepo::list_by_work_unit(&*state.db, &unit.id).await?;
    Ok(work_unit_response(
        unit,
        readiness,
        review_subject,
        repository_ids,
        integrations,
    ))
}

pub async fn create_work_unit(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<CreateWorkUnitRequest>,
) -> ApiResult<(StatusCode, Json<WorkUnitResponse>)> {
    let provenance = body.provenance.map(|value| match value {
        CreateWorkUnitProvenance::Actor { actor } => {
            db::WorkUnitProvenance::Actor(actor_to_db(actor))
        }
        CreateWorkUnitProvenance::WorkUnit { id } => db::WorkUnitProvenance::WorkUnit(id),
        CreateWorkUnitProvenance::Artifact { id } => db::WorkUnitProvenance::Artifact(id),
        CreateWorkUnitProvenance::External { id } => db::WorkUnitProvenance::External(id),
    });
    let unit = state
        .work_unit_service
        .create_with_repository_ids(
            CollaborationActorSource::Human(user.user_id.clone()),
            services::CreateWorkUnitInput {
                task_id,
                title: body.title,
                scope: body.scope,
                execution_instructions: body.execution_instructions,
                role: body.role,
                parent_work_unit_id: body.parent_work_unit_id,
                assigned_actor: body.assigned_actor.map(actor_to_db),
                requires_integration: body.requires_integration,
                provenance,
                artifact_inputs: body.artifact_inputs.unwrap_or_default(),
                review_approval: body.review_approval,
                review_subject: body
                    .review_subject
                    .map(|subject| subject.reviewed_work_unit_id),
            },
            body.repository_ids.unwrap_or_default(),
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

pub async fn get_artifact_package(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<api_types::WorkUnitArtifactPackageResponse>> {
    let package = state
        .work_unit_service
        .artifact_package(CollaborationActorSource::Human(user.user_id.clone()), &id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(package))
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
            body.execution_instructions,
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

pub async fn prepare_workspace(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<api_types::WorkspaceResponse>> {
    state
        .work_unit_service
        .prepare_workspace_group(CollaborationActorSource::Human(user.user_id.clone()), &id)
        .await
        .map_err(ApiError::from)?;
    let workspace = WorkUnitWorkspaceRepo::list_by_work_unit_id(&*state.db, &id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("workspace", id.clone()))?;
    Ok(Json(crate::routes::workspace_response(workspace)))
}

pub async fn integrate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<WorkUnitIntegrationRequest>,
) -> ApiResult<Json<WorkUnitIntegrationResponse>> {
    let batch = state
        .work_unit_service
        .integrate(
            CollaborationActorSource::Human(user.user_id.clone()),
            &id,
            &body.execution_id,
            &body.idempotency_key,
        )
        .await
        .map_err(ApiError::from)?;
    let integration = WorkUnitRepo::get_integration_by_idempotency(
        &*state.db,
        &batch.task_id,
        &body.idempotency_key,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("work unit integration", body.idempotency_key.clone()))?;
    let workspace = WorkspaceRepo::get_by_id(&*state.db, &integration.source_workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace", integration.source_workspace_id.clone()))?;
    let mut response = integration_response(integration.clone(), workspace.repo_id.clone());
    let children =
        WorkUnitRepositoryIntegrationRepo::list_by_work_unit(&*state.db, &batch.work_unit_id)
            .await?
            .into_iter()
            .filter(|row| row.work_unit_integration_id == response.id)
            .collect::<Vec<_>>();
    response.repositories = if children.is_empty() {
        let pinned_revision = TaskRepositorySetRepo::get_for_work_unit(
            &*state.db,
            &batch.work_unit_id,
            &batch.task_id,
        )
        .await?;
        match pinned_revision.filter(|revision| {
            let writable = revision
                .repositories
                .iter()
                .filter(|repository| {
                    matches!(
                        repository.role,
                        db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                    )
                })
                .collect::<Vec<_>>();
            writable.len() == 1
                && writable[0].role == db::TaskRepositoryRole::Primary
                && writable[0].repo_id == workspace.repo_id
        }) {
            Some(revision) => vec![legacy_single_repository_integration_response(
                &integration,
                workspace.repo_id,
                revision.revision,
            )],
            None => Vec::new(),
        }
    } else {
        children
            .into_iter()
            .map(repository_integration_response)
            .collect()
    };
    Ok(Json(response))
}

fn dependency_response(dependency: WorkUnitDependency) -> WorkUnitDependencyResponse {
    WorkUnitDependencyResponse {
        work_unit_id: dependency.work_unit_id,
        depends_on_work_unit_id: dependency.depends_on_work_unit_id,
        satisfied: dependency.satisfied,
        created_at: dependency.created_at,
    }
}

fn integration_response(
    integration: WorkUnitIntegration,
    repo_id: String,
) -> WorkUnitIntegrationResponse {
    WorkUnitIntegrationResponse {
        id: integration.id,
        task_id: integration.task_id,
        work_unit_id: integration.work_unit_id,
        repo_id,
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
        repositories: Vec::new(),
    }
}

fn repository_integration_response(
    integration: WorkUnitRepositoryIntegration,
) -> WorkUnitRepositoryIntegrationResponse {
    WorkUnitRepositoryIntegrationResponse {
        integration_id: integration.id,
        parent_integration_id: integration.work_unit_integration_id,
        task_id: integration.task_id,
        work_unit_id: integration.work_unit_id,
        execution_id: integration.execution_id,
        repository_set_revision: integration.repository_set_revision,
        repo_id: integration.repo_id,
        source_workspace_id: integration.source_workspace_id,
        source_branch: integration.source_branch,
        source_sha: integration.source_sha,
        target_workspace_id: integration.target_workspace_id,
        target_branch: integration.target_branch,
        target_before_sha: integration.target_before_sha,
        target_after_sha: integration.target_after_sha,
        outcome: match integration.outcome {
            db::WorkUnitRepositoryIntegrationOutcome::Running => {
                api_types::WorkUnitIntegrationOutcome::Running
            }
            db::WorkUnitRepositoryIntegrationOutcome::Success => {
                api_types::WorkUnitIntegrationOutcome::Success
            }
            db::WorkUnitRepositoryIntegrationOutcome::Unchanged => {
                api_types::WorkUnitIntegrationOutcome::Unchanged
            }
            db::WorkUnitRepositoryIntegrationOutcome::Conflict => {
                api_types::WorkUnitIntegrationOutcome::Conflict
            }
            db::WorkUnitRepositoryIntegrationOutcome::Failed => {
                api_types::WorkUnitIntegrationOutcome::Failed
            }
            db::WorkUnitRepositoryIntegrationOutcome::Rejected => {
                api_types::WorkUnitIntegrationOutcome::Rejected
            }
        },
        version: integration.version,
        started_at: integration.started_at,
        finished_at: integration.finished_at,
    }
}

fn legacy_single_repository_integration_response(
    integration: &WorkUnitIntegration,
    repo_id: String,
    repository_set_revision: i64,
) -> WorkUnitRepositoryIntegrationResponse {
    WorkUnitRepositoryIntegrationResponse {
        integration_id: integration.id.clone(),
        parent_integration_id: integration.id.clone(),
        task_id: integration.task_id.clone(),
        work_unit_id: integration.work_unit_id.clone(),
        execution_id: integration.execution_id.clone(),
        repository_set_revision,
        repo_id,
        source_workspace_id: integration.source_workspace_id.clone(),
        source_branch: integration.source_branch.clone(),
        source_sha: integration.source_sha.clone(),
        target_workspace_id: integration.target_workspace_id.clone(),
        target_branch: integration.target_branch.clone(),
        target_before_sha: integration.target_before_sha.clone(),
        target_after_sha: integration.target_after_sha.clone(),
        outcome: match integration.outcome {
            db::WorkUnitIntegrationOutcome::Running => {
                api_types::WorkUnitIntegrationOutcome::Running
            }
            db::WorkUnitIntegrationOutcome::Success => {
                api_types::WorkUnitIntegrationOutcome::Success
            }
            db::WorkUnitIntegrationOutcome::Conflict => {
                api_types::WorkUnitIntegrationOutcome::Conflict
            }
            db::WorkUnitIntegrationOutcome::Failed => api_types::WorkUnitIntegrationOutcome::Failed,
            db::WorkUnitIntegrationOutcome::Rejected => {
                api_types::WorkUnitIntegrationOutcome::Rejected
            }
        },
        version: integration.version,
        started_at: integration.started_at.clone(),
        finished_at: integration.finished_at.clone(),
    }
}
