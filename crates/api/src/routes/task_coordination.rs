use std::sync::Arc;

use api_types::{
    ActorRef, ApproveTaskCoordinationPlanRequest, AuthorizeTaskCoordinationRequest,
    CreateTaskCoordinationTurnRequest, CreateTaskCoordinationTurnResponse,
    CreateTaskInterventionRequest, CreateTaskQualityRequest,
    MaterializeTaskCoordinationPlanRequest, MaterializeTaskCoordinationPlanResponse,
    ResolveTaskQualityRequest, RevokeTaskCoordinationRequest, TaskCoordinationBlockerResponse,
    TaskCoordinationBriefResponse, TaskCoordinationCoordinatorResponse,
    TaskCoordinationMaterializationResponse, TaskCoordinationMode,
    TaskCoordinationOverviewResponse, TaskCoordinationPlanDecision, TaskCoordinationPlanResponse,
    TaskCoordinationPlanState, TaskCoordinationPlanWorkUnitResponse,
    TaskCoordinationRequestResponse, TaskCoordinationRequestState,
    TaskCoordinationTeamMemberResponse, TaskCoordinationTeamRoleResponse,
    TaskDispatchIntentResponse, TaskDispatchState, TaskInterventionKind, TaskInterventionResponse,
    TaskInterventionState, TaskInterventionsResponse, TaskLifecycleResponse, TaskLifecycleState,
    TaskQualityCategory, TaskQualityEscalationResponse, TaskQualityRequestResponse,
    TaskQualityRequestsResponse, TaskQualityResolution, TaskQualityRouteResponse, TaskQualityState,
    TaskQualityTargetKind,
};
use axum::{
    extract::{Path, State},
    Json,
};
use db::{
    TaskCoordinationPlanState as DbPlanState, TaskCoordinationRequestState as DbRequestState,
    TaskDispatchState as DbDispatchState, TaskLifecycleRepo,
    TaskLifecycleState as DbLifecycleState,
};
use services::task_coordination::{
    CoordinationBlocker, CoordinationBrief, CoordinatorView, InterventionInput, MaterializeInput,
    MaterializeOutcome, Overview, PlanDecisionInput, PlanView, QualityRequestInput,
    TaskCoordinationService, TeamRoleView, TurnRequestInput,
};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{auth::AuthenticatedUser, tasks},
    state::AppState,
};

fn service(state: &AppState) -> TaskCoordinationService {
    TaskCoordinationService::new(Arc::clone(&state.db), Arc::clone(&state.event_bus))
}

fn db_mode(mode: TaskCoordinationMode) -> db::TaskCoordinationMode {
    match mode {
        TaskCoordinationMode::Manual => db::TaskCoordinationMode::Manual,
        TaskCoordinationMode::Proposal => db::TaskCoordinationMode::Proposal,
        TaskCoordinationMode::PrepareAndExecute => db::TaskCoordinationMode::PrepareAndExecute,
    }
}

fn api_mode(mode: db::TaskCoordinationMode) -> TaskCoordinationMode {
    match mode {
        db::TaskCoordinationMode::Manual => TaskCoordinationMode::Manual,
        db::TaskCoordinationMode::Proposal => TaskCoordinationMode::Proposal,
        db::TaskCoordinationMode::PrepareAndExecute => TaskCoordinationMode::PrepareAndExecute,
    }
}

fn api_request_state(state: DbRequestState) -> TaskCoordinationRequestState {
    match state {
        DbRequestState::Requested => TaskCoordinationRequestState::Requested,
        DbRequestState::Running => TaskCoordinationRequestState::Running,
        DbRequestState::PlanReady => TaskCoordinationRequestState::PlanReady,
        DbRequestState::AwaitingApproval => TaskCoordinationRequestState::AwaitingApproval,
        DbRequestState::Materialized => TaskCoordinationRequestState::Materialized,
        DbRequestState::Failed => TaskCoordinationRequestState::Failed,
        DbRequestState::Cancelled => TaskCoordinationRequestState::Cancelled,
        DbRequestState::Uncertain => TaskCoordinationRequestState::Uncertain,
    }
}

fn api_plan_state(state: DbPlanState) -> TaskCoordinationPlanState {
    match state {
        DbPlanState::Proposed => TaskCoordinationPlanState::Proposed,
        DbPlanState::Approved => TaskCoordinationPlanState::Approved,
        DbPlanState::Rejected => TaskCoordinationPlanState::Rejected,
        DbPlanState::Superseded => TaskCoordinationPlanState::Superseded,
        DbPlanState::Materialized => TaskCoordinationPlanState::Materialized,
    }
}

async fn lifecycle_response(state: &AppState, task_id: &str) -> ApiResult<TaskLifecycleResponse> {
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*state.db, task_id)
        .await?
        .ok_or_else(|| ApiError::not_found("task lifecycle", task_id.to_owned()))?;
    Ok(TaskLifecycleResponse {
        task_id: lifecycle.task_id,
        state: match lifecycle.state {
            DbLifecycleState::Backlog => TaskLifecycleState::Backlog,
            DbLifecycleState::Ready => TaskLifecycleState::Ready,
            DbLifecycleState::Active => TaskLifecycleState::Active,
            DbLifecycleState::Blocked => TaskLifecycleState::Blocked,
            DbLifecycleState::ReadyToMerge => TaskLifecycleState::ReadyToMerge,
            DbLifecycleState::Merging => TaskLifecycleState::Merging,
            DbLifecycleState::Done => TaskLifecycleState::Done,
            DbLifecycleState::Cancelled => TaskLifecycleState::Cancelled,
        },
        version: lifecycle.version,
        reason_kind: lifecycle.reason_kind,
        reason_ref: lifecycle.reason_ref,
        created_at: lifecycle.created_at,
        updated_at: lifecycle.updated_at,
    })
}

async fn dispatch_response(
    state: &AppState,
    task_id: &str,
) -> ApiResult<TaskDispatchIntentResponse> {
    let dispatch = TaskLifecycleRepo::get_task_dispatch(&*state.db, task_id)
        .await?
        .ok_or_else(|| ApiError::not_found("task dispatch", task_id.to_owned()))?;
    Ok(TaskDispatchIntentResponse {
        task_id: dispatch.task_id,
        state: match dispatch.state {
            DbDispatchState::Idle => TaskDispatchState::Idle,
            DbDispatchState::Running => TaskDispatchState::Running,
            DbDispatchState::Paused => TaskDispatchState::Paused,
            DbDispatchState::Cancelled => TaskDispatchState::Cancelled,
        },
        version: dispatch.version,
        requested_by: match (
            dispatch.requested_by_kind.as_str(),
            dispatch.requested_by_id.as_deref(),
        ) {
            ("human", Some(id)) => Some(ActorRef::Human(id.to_owned())),
            ("agent", Some(id)) => Some(ActorRef::Agent(id.to_owned())),
            _ => None,
        },
        reason: dispatch.reason,
        created_at: dispatch.created_at,
        updated_at: dispatch.updated_at,
    })
}

fn coordinator_response(coordinator: CoordinatorView) -> TaskCoordinationCoordinatorResponse {
    TaskCoordinationCoordinatorResponse {
        task_role_id: coordinator.task_role_id,
        role: coordinator.role,
        actor_ref: api_actor(coordinator.actor_ref),
        display_name: coordinator.display_name,
        harness: coordinator.harness,
    }
}

fn api_actor(actor: db::ActorRef) -> ActorRef {
    match actor {
        db::ActorRef::Human(id) => ActorRef::Human(id),
        db::ActorRef::Agent(id) => ActorRef::Agent(id),
    }
}

fn db_actor(actor: ActorRef) -> db::ActorRef {
    match actor {
        ActorRef::Human(id) => db::ActorRef::Human(id),
        ActorRef::Agent(id) => db::ActorRef::Agent(id),
    }
}

fn team_response(role: TeamRoleView) -> TaskCoordinationTeamRoleResponse {
    TaskCoordinationTeamRoleResponse {
        id: role.id,
        role: role.role,
        display_label: role.display_label,
        coordination_mode: role.coordination_mode.map(|mode| match mode {
            db::CoordinationMode::Partitioned => api_types::CoordinationMode::Partitioned,
            db::CoordinationMode::Collaborative => api_types::CoordinationMode::Collaborative,
            db::CoordinationMode::Independent => api_types::CoordinationMode::Independent,
        }),
        capabilities: role.capabilities,
        configuration: role.configuration,
        version: role.version,
        members: role
            .members
            .into_iter()
            .map(|member| TaskCoordinationTeamMemberResponse {
                id: member.id,
                actor_ref: api_actor(member.actor_ref),
                display_name: member.display_name,
                harness: member.harness,
                status: member.status,
                version: member.version,
            })
            .collect(),
    }
}

fn blocker_response(blocker: CoordinationBlocker) -> TaskCoordinationBlockerResponse {
    TaskCoordinationBlockerResponse {
        code: blocker.code,
        message: blocker.message,
        task_role_id: blocker.task_role_id,
        actor_ref: blocker.actor_ref.map(api_actor),
    }
}

fn brief_response(brief: CoordinationBrief) -> TaskCoordinationBriefResponse {
    TaskCoordinationBriefResponse {
        objective: brief.objective,
        constraints: brief.constraints,
        acceptance_criteria: brief.acceptance_criteria,
        context: brief.context,
    }
}

fn request_response(
    request: db::TaskCoordinationRequest,
    brief: CoordinationBrief,
) -> TaskCoordinationRequestResponse {
    TaskCoordinationRequestResponse {
        id: request.id,
        task_id: request.task_id,
        mode: api_mode(request.mode),
        state: api_request_state(request.state),
        authorization_version: request.authorization_version,
        objective_digest: request.objective_digest,
        request_digest: request.request_digest,
        brief: brief_response(brief),
        wake_id: request.wake_id,
        execution_id: request.execution_id,
        plan_artifact_id: request.plan_artifact_id,
        failure_code: request.failure_code,
        failure_message: request.failure_message,
        version: request.version,
        created_at: request.created_at,
        updated_at: request.updated_at,
        completed_at: request.completed_at,
    }
}

fn materialization_response(
    materialization: db::TaskCoordinationMaterialization,
) -> ApiResult<TaskCoordinationMaterializationResponse> {
    let work_unit_ids = serde_json::from_str::<Vec<String>>(&materialization.work_unit_ids_json)
        .map_err(|error| {
            ApiError::internal(format!("stored WorkUnit receipt is invalid: {error}"))
        })?;
    Ok(TaskCoordinationMaterializationResponse {
        id: materialization.id,
        plan_artifact_id: materialization.plan_artifact_id,
        plan_digest: materialization.plan_digest,
        work_unit_ids,
        receipt_digest: materialization.receipt_digest,
        created_by: materialization.created_by,
        created_at: materialization.created_at,
    })
}

fn plan_response(view: PlanView) -> ApiResult<TaskCoordinationPlanResponse> {
    let work_units = view
        .parsed
        .work_units
        .into_iter()
        .map(|unit| TaskCoordinationPlanWorkUnitResponse {
            key: unit.key,
            title: unit.title,
            scope: unit.scope,
            role: unit.role,
            assigned_actor: unit.assigned_actor,
            parent_key: unit.parent_key,
            depends_on: unit.depends_on,
            requires_integration: unit.requires_integration,
            execution_instructions: unit.execution_instructions,
            review_subject_key: unit.review_subject_key,
            acceptance: unit.acceptance,
            artifact_inputs: unit.artifact_inputs,
        })
        .collect();
    Ok(TaskCoordinationPlanResponse {
        artifact_id: view.plan.artifact_id,
        task_id: view.plan.task_id,
        request_id: view.plan.request_id,
        mode: api_mode(view.mode),
        state: api_plan_state(view.plan.state),
        authorization_version: view.plan.authorization_version,
        task_version: view.parsed.task_version,
        digest: view.plan.plan_digest,
        team_digest: view.parsed.team_digest,
        envelope_digest: view.parsed.envelope_digest,
        objective_digest: view.parsed.objective_digest,
        work_units,
        proposal_id: view.plan.proposal_id,
        decision_id: view.plan.decision_id,
        materialization: view
            .materialization
            .map(materialization_response)
            .transpose()?,
        version: view.plan.version,
        created_at: view.plan.created_at,
        updated_at: view.plan.updated_at,
    })
}

async fn overview_response(
    state: &AppState,
    overview: Overview,
) -> ApiResult<TaskCoordinationOverviewResponse> {
    let lifecycle = lifecycle_response(state, &overview.task.id).await?;
    let dispatch = dispatch_response(state, &overview.task.id).await?;
    let team = overview.team.into_iter().map(team_response).collect();
    let latest_request = overview
        .latest_request
        .map(|(request, brief)| request_response(request, brief));
    let latest_plan = match overview.latest_plan {
        Some(plan) => Some(plan_response(plan)?),
        None => None,
    };
    Ok(TaskCoordinationOverviewResponse {
        task_id: overview.task.id,
        task_version: overview.task.version,
        lifecycle,
        dispatch,
        mode: api_mode(overview.coordination.mode),
        authorization_version: overview.coordination.authorization_version,
        revoked_at: overview.coordination.revoked_at,
        revoke_reason: overview.coordination.revoke_reason,
        requested_by: overview.coordination.requested_by.map(ActorRef::Human),
        coordinator: overview.coordinator.map(coordinator_response),
        team,
        team_digest: overview.coordination.team_digest,
        envelope: serde_json::to_value(&overview.envelope).map_err(|error| {
            ApiError::internal(format!(
                "coordination envelope could not serialize: {error}"
            ))
        })?,
        envelope_digest: overview.coordination.envelope_digest,
        turn_count: overview.coordination.turn_count,
        max_turns: overview.coordination.max_turns,
        quality_request_count: overview.coordination.quality_request_count,
        max_quality_requests: overview.coordination.max_quality_requests,
        can_authorize: overview.can_authorize,
        can_request: overview.can_request,
        blockers: overview
            .blockers
            .into_iter()
            .map(blocker_response)
            .collect(),
        latest_request,
        latest_plan,
    })
}

pub async fn get_coordination(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<TaskCoordinationOverviewResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let overview = service(&state)
        .overview(&task_id, &user.user_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(overview_response(&state, overview).await?))
}

pub async fn authorize(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<AuthorizeTaskCoordinationRequest>,
) -> ApiResult<Json<TaskCoordinationOverviewResponse>> {
    tasks::require_task_mutable(&state, &task_id, &user).await?;
    let overview = service(&state)
        .authorize(
            &task_id,
            &user.user_id,
            services::task_coordination::AuthorizationInput {
                expected_task_version: request.expected_task_version,
                expected_coordination_version: request.expected_coordination_version,
                mode: db_mode(request.mode),
                coordinator_task_role_id: request.coordinator_task_role_id,
                coordinator_actor: request.coordinator_actor.map(db_actor),
                envelope: request.envelope,
                idempotency_key: request.idempotency_key,
            },
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(overview_response(&state, overview).await?))
}

pub async fn revoke(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<RevokeTaskCoordinationRequest>,
) -> ApiResult<Json<TaskCoordinationOverviewResponse>> {
    tasks::require_task_mutable(&state, &task_id, &user).await?;
    let overview = service(&state)
        .revoke(
            &task_id,
            &user.user_id,
            request.expected_task_version,
            request.expected_coordination_version,
            request.reason,
            &request.idempotency_key,
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(overview_response(&state, overview).await?))
}

pub async fn request_turn(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<CreateTaskCoordinationTurnRequest>,
) -> ApiResult<Json<CreateTaskCoordinationTurnResponse>> {
    tasks::require_task_mutable(&state, &task_id, &user).await?;
    let (turn, accepted) = service(&state)
        .request_turn(
            &task_id,
            &user.user_id,
            TurnRequestInput {
                expected_task_version: request.expected_task_version,
                expected_coordination_version: request.expected_coordination_version,
                brief: CoordinationBrief {
                    objective: request.objective,
                    constraints: request.constraints,
                    acceptance_criteria: request.acceptance_criteria,
                    context: request.context,
                },
                idempotency_key: request.idempotency_key,
            },
        )
        .await
        .map_err(ApiError::from)?;
    let (request, brief) = service(&state)
        .get_request(&task_id, &turn.id, &user.user_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(CreateTaskCoordinationTurnResponse {
        request: request_response(request, brief),
        accepted,
        blockers: Vec::new(),
    }))
}

pub async fn get_request(
    State(state): State<AppState>,
    Path((task_id, request_id)): Path<(String, String)>,
    user: AuthenticatedUser,
) -> ApiResult<Json<TaskCoordinationRequestResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let (request, brief) = service(&state)
        .get_request(&task_id, &request_id, &user.user_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(request_response(request, brief)))
}

pub async fn get_plan(
    State(state): State<AppState>,
    Path((task_id, artifact_id)): Path<(String, String)>,
    user: AuthenticatedUser,
) -> ApiResult<Json<TaskCoordinationPlanResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let plan = service(&state)
        .get_plan(&task_id, &artifact_id, &user.user_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(plan_response(plan)?))
}

pub async fn approve_plan(
    State(state): State<AppState>,
    Path((task_id, artifact_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(request): Json<ApproveTaskCoordinationPlanRequest>,
) -> ApiResult<Json<TaskCoordinationPlanResponse>> {
    tasks::require_task_mutable(&state, &task_id, &user).await?;
    let plan = service(&state)
        .approve_plan(
            &task_id,
            &artifact_id,
            &user.user_id,
            PlanDecisionInput {
                expected_coordination_version: request.expected_coordination_version,
                expected_plan_digest: request.expected_plan_digest,
                approve: request.decision == TaskCoordinationPlanDecision::Approve,
                rationale: request.rationale,
                idempotency_key: request.idempotency_key,
            },
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(plan_response(plan)?))
}

pub async fn materialize_plan(
    State(state): State<AppState>,
    Path((task_id, artifact_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(request): Json<MaterializeTaskCoordinationPlanRequest>,
) -> ApiResult<Json<MaterializeTaskCoordinationPlanResponse>> {
    tasks::require_task_mutable(&state, &task_id, &user).await?;
    let outcome: MaterializeOutcome = service(&state)
        .materialize(
            &task_id,
            &artifact_id,
            &user.user_id,
            MaterializeInput {
                expected_task_version: request.expected_task_version,
                expected_coordination_version: request.expected_coordination_version,
                expected_plan_digest: request.expected_plan_digest,
                expected_plan_version: request.expected_plan_version,
                expected_request_version: request.expected_request_version,
                idempotency_key: request.idempotency_key,
            },
        )
        .await
        .map_err(ApiError::from)?;
    let (request, brief) = service(&state)
        .get_request(&task_id, &outcome.request.id, &user.user_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(MaterializeTaskCoordinationPlanResponse {
        plan: plan_response(outcome.plan)?,
        request: request_response(request, brief),
        materialization: materialization_response(outcome.materialization)?,
        replayed: outcome.replayed,
        launch_intent_accepted: outcome.launch_intent_accepted,
        launch_blockers: outcome.launch_blockers,
    }))
}

fn quality_response(request: db::TaskQualityRequest) -> ApiResult<TaskQualityRequestResponse> {
    let findings =
        serde_json::from_str::<Vec<String>>(&request.findings_json).map_err(|error| {
            ApiError::internal(format!("stored quality findings are invalid: {error}"))
        })?;
    let cycle_request_ids = serde_json::from_str::<Vec<String>>(&request.cycle_request_ids_json)
        .map_err(|error| {
            ApiError::internal(format!(
                "stored quality route request cycle is invalid: {error}"
            ))
        })?;
    let cycle_task_role_ids = serde_json::from_str::<Vec<String>>(
        &request.cycle_task_role_ids_json,
    )
    .map_err(|error| {
        ApiError::internal(format!(
            "stored quality route role cycle is invalid: {error}"
        ))
    })?;
    Ok(TaskQualityRequestResponse {
        id: request.id,
        task_id: request.task_id,
        source_kind: request.source_kind,
        source_id: request.source_id,
        category: match request.category {
            db::TaskQualityCategory::DeliveryCorrection => TaskQualityCategory::DeliveryCorrection,
            db::TaskQualityCategory::PlanRevision => TaskQualityCategory::PlanRevision,
            db::TaskQualityCategory::Specialist => TaskQualityCategory::Specialist,
            db::TaskQualityCategory::HumanDecision => TaskQualityCategory::HumanDecision,
        },
        target_kind: match request.target_kind {
            db::TaskQualityTargetKind::Implementation => TaskQualityTargetKind::Implementation,
            db::TaskQualityTargetKind::Orchestrator => TaskQualityTargetKind::Orchestrator,
            db::TaskQualityTargetKind::Specialist => TaskQualityTargetKind::Specialist,
            db::TaskQualityTargetKind::Human => TaskQualityTargetKind::Human,
        },
        target_task_role_id: request.target_task_role_id,
        target_actor: request.target_actor.map(api_actor),
        summary: request.summary,
        findings,
        resolution_condition: request.resolution_condition,
        state: match request.state {
            db::TaskQualityState::Requested => TaskQualityState::Requested,
            db::TaskQualityState::Notified => TaskQualityState::Notified,
            db::TaskQualityState::WorkCreated => TaskQualityState::WorkCreated,
            db::TaskQualityState::Running => TaskQualityState::Running,
            db::TaskQualityState::Resolved => TaskQualityState::Resolved,
            db::TaskQualityState::RequiresHuman => TaskQualityState::RequiresHuman,
            db::TaskQualityState::Cancelled => TaskQualityState::Cancelled,
        },
        handoff_id: request.handoff_id,
        work_unit_id: request.work_unit_id,
        coordination_request_id: request.coordination_request_id,
        budget_round: request.budget_round,
        version: request.version,
        created_at: request.created_at,
        updated_at: request.updated_at,
        resolved_at: request.resolved_at,
        resolution_kind: request.resolution_kind,
        resolution_ref: request.resolution_ref,
        resolution_digest: request.resolution_digest,
        quality_route: TaskQualityRouteResponse {
            id: request.route_id,
            round: request.route_round,
            limit: request.route_limit,
            source_task_role_id: request.source_task_role_id,
            escalation: match (
                request.requires_human_reason_code,
                request.requires_human_reason,
            ) {
                (Some(reason_code), Some(reason)) => Some(TaskQualityEscalationResponse {
                    reason_code,
                    reason,
                    cycle_request_ids,
                    cycle_task_role_ids,
                }),
                _ => None,
            },
        },
    })
}

pub async fn list_quality_requests(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<TaskQualityRequestsResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let items = service(&state)
        .list_quality_requests(&task_id, &user.user_id)
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .map(quality_response)
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(TaskQualityRequestsResponse { items }))
}

pub async fn create_quality_request(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<CreateTaskQualityRequest>,
) -> ApiResult<Json<TaskQualityRequestResponse>> {
    tasks::require_task_mutable(&state, &task_id, &user).await?;
    let source = match request.source_execution_id.as_deref() {
        Some(execution_id) => {
            services::CollaborationActorSource::Execution(execution_id.to_owned())
        }
        None => services::CollaborationActorSource::Human(user.user_id.clone()),
    };
    let request = service(&state)
        .create_quality_request_as(
            &task_id,
            source,
            &user.user_id,
            QualityRequestInput {
                source_artifact_id: request.source_artifact_id,
                category: match request.category {
                    TaskQualityCategory::DeliveryCorrection => {
                        db::TaskQualityCategory::DeliveryCorrection
                    }
                    TaskQualityCategory::PlanRevision => db::TaskQualityCategory::PlanRevision,
                    TaskQualityCategory::Specialist => db::TaskQualityCategory::Specialist,
                    TaskQualityCategory::HumanDecision => db::TaskQualityCategory::HumanDecision,
                },
                target_kind: match request.target_kind {
                    TaskQualityTargetKind::Implementation => {
                        db::TaskQualityTargetKind::Implementation
                    }
                    TaskQualityTargetKind::Orchestrator => db::TaskQualityTargetKind::Orchestrator,
                    TaskQualityTargetKind::Specialist => db::TaskQualityTargetKind::Specialist,
                    TaskQualityTargetKind::Human => db::TaskQualityTargetKind::Human,
                },
                target_task_role_id: request.target_task_role_id,
                target_actor: request.target_actor.map(db_actor),
                summary: request.summary,
                findings: request.findings,
                resolution_condition: request.resolution_condition,
                idempotency_key: request.idempotency_key,
            },
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(quality_response(request)?))
}

pub async fn resolve_quality_request(
    State(state): State<AppState>,
    Path((task_id, request_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(request): Json<ResolveTaskQualityRequest>,
) -> ApiResult<Json<TaskQualityRequestResponse>> {
    tasks::require_task_mutable(&state, &task_id, &user).await?;
    let db_state = match request.resolution {
        TaskQualityResolution::Resolved => db::TaskQualityState::Resolved,
        TaskQualityResolution::Cancelled => db::TaskQualityState::Cancelled,
        TaskQualityResolution::RequiresHuman => db::TaskQualityState::RequiresHuman,
    };
    let request = service(&state)
        .resolve_quality_request(
            &task_id,
            &request_id,
            &user.user_id,
            request.expected_version,
            db_state,
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(quality_response(request)?))
}

fn intervention_response(intervention: db::TaskIntervention) -> TaskInterventionResponse {
    TaskInterventionResponse {
        id: intervention.id,
        task_id: intervention.task_id,
        kind: match intervention.kind {
            db::TaskInterventionKind::Instruction => TaskInterventionKind::Instruction,
            db::TaskInterventionKind::Decision => TaskInterventionKind::Decision,
            db::TaskInterventionKind::Pause => TaskInterventionKind::Pause,
            db::TaskInterventionKind::Resume => TaskInterventionKind::Resume,
            db::TaskInterventionKind::Cancel => TaskInterventionKind::Cancel,
            db::TaskInterventionKind::RevokeAuthorization => {
                TaskInterventionKind::RevokeAuthorization
            }
            db::TaskInterventionKind::RequestReplan => TaskInterventionKind::RequestReplan,
        },
        state: match intervention.state {
            db::TaskInterventionState::Recorded => TaskInterventionState::Recorded,
            db::TaskInterventionState::Applied => TaskInterventionState::Applied,
            db::TaskInterventionState::RequiresHuman => TaskInterventionState::RequiresHuman,
            db::TaskInterventionState::Rejected => TaskInterventionState::Rejected,
        },
        source_kind: intervention.source_kind,
        source_id: intervention.source_id,
        expected_task_version: intervention.expected_task_version,
        expected_coordination_version: intervention.expected_coordination_version,
        proposal_id: intervention.proposal_id,
        decision_id: intervention.decision_id,
        coordination_request_id: intervention.coordination_request_id,
        failure_code: intervention.failure_code,
        failure_message: intervention.failure_message,
        version: intervention.version,
        created_at: intervention.created_at,
        updated_at: intervention.updated_at,
    }
}

pub async fn list_interventions(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<TaskInterventionsResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let items = service(&state)
        .list_interventions(&task_id, &user.user_id)
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .map(intervention_response)
        .collect();
    Ok(Json(TaskInterventionsResponse { items }))
}

pub async fn create_intervention(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<CreateTaskInterventionRequest>,
) -> ApiResult<Json<TaskInterventionResponse>> {
    tasks::require_task_mutable(&state, &task_id, &user).await?;
    let kind = match request.kind {
        TaskInterventionKind::Instruction => db::TaskInterventionKind::Instruction,
        TaskInterventionKind::Decision => db::TaskInterventionKind::Decision,
        TaskInterventionKind::Pause => db::TaskInterventionKind::Pause,
        TaskInterventionKind::Resume => db::TaskInterventionKind::Resume,
        TaskInterventionKind::Cancel => db::TaskInterventionKind::Cancel,
        TaskInterventionKind::RevokeAuthorization => db::TaskInterventionKind::RevokeAuthorization,
        TaskInterventionKind::RequestReplan => db::TaskInterventionKind::RequestReplan,
    };
    let brief = request.brief.map(|brief| TurnRequestInput {
        expected_task_version: brief.expected_task_version,
        expected_coordination_version: brief.expected_coordination_version,
        brief: CoordinationBrief {
            objective: brief.objective,
            constraints: brief.constraints,
            acceptance_criteria: brief.acceptance_criteria,
            context: brief.context,
        },
        idempotency_key: brief.idempotency_key,
    });
    let intervention = service(&state)
        .create_intervention(
            &task_id,
            &user.user_id,
            InterventionInput {
                kind,
                expected_task_version: request.expected_task_version,
                expected_coordination_version: request.expected_coordination_version,
                expected_lifecycle_version: request.expected_lifecycle_version,
                instruction: request.instruction,
                plan_artifact_id: request.plan_artifact_id,
                approve: request
                    .decision
                    .map(|decision| decision == TaskCoordinationPlanDecision::Approve),
                rationale: request.rationale,
                reason: request.reason,
                brief,
                idempotency_key: request.idempotency_key,
            },
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(intervention_response(intervention)))
}
