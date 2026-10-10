use super::*;
use api_types::{
    CreateTaskGateRequest, GateEvaluationInputResponse, GateEvaluationResponse,
    GatePolicyRevisionResponse, GateResponse, MergeAfterGateRequest, MergeAfterGateResponse,
    ReviseGatePolicyRequest, TaskGateResponse, TaskLifecycleResponse,
    TaskLifecycleTransitionFactResponse, TaskLifecycleTransitionResponse,
    TransitionTaskLifecycleRequest,
};
use db::{
    Gate, GateEvaluation, GateEvaluationInput, GatePolicyRevision, GateRepo, GateScopeKind,
    TaskLifecycleRepo, TaskLifecycleState as DbTaskLifecycleState, TaskRepo,
};
use services::gate_engine::GateEngine;
use std::sync::Arc;

fn gate_response(gate: Gate) -> TaskGateResponse {
    TaskGateResponse {
        id: gate.id,
        task_id: gate.task_id,
        gate_kind: gate.gate_kind,
        scope_kind: gate.scope_kind.to_string(),
        scope_id: gate.scope_id,
        active_policy_revision: gate.active_policy_revision,
        created_at: gate.created_at,
    }
}

fn lifecycle_response(lifecycle: db::TaskLifecycle) -> TaskLifecycleResponse {
    TaskLifecycleResponse {
        task_id: lifecycle.task_id,
        state: lifecycle_state_response(lifecycle.state),
        version: lifecycle.version,
        reason_kind: lifecycle.reason_kind,
        reason_ref: lifecycle.reason_ref,
        created_at: lifecycle.created_at,
        updated_at: lifecycle.updated_at,
    }
}

pub async fn get_task_lifecycle(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
) -> ApiResult<Json<TaskLifecycleResponse>> {
    require_task_visible(&state, &task_id, &user).await?;
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*state.db, &task_id)
        .await?
        .ok_or_else(|| ApiError::not_found("task lifecycle", task_id))?;
    Ok(Json(lifecycle_response(lifecycle)))
}

pub async fn transition_task_lifecycle(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
    Json(request): Json<TransitionTaskLifecycleRequest>,
) -> ApiResult<Json<TaskLifecycleTransitionResponse>> {
    super::require_task_visible(&state, &task_id, &user).await?;
    let task = TaskRepo::get_by_id(&*state.db, &task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.clone()))?;
    let to_state = match request.to_state {
        api_types::TaskLifecycleState::Backlog => DbTaskLifecycleState::Backlog,
        api_types::TaskLifecycleState::Ready => DbTaskLifecycleState::Ready,
        api_types::TaskLifecycleState::Active => DbTaskLifecycleState::Active,
        api_types::TaskLifecycleState::Blocked => DbTaskLifecycleState::Blocked,
        api_types::TaskLifecycleState::ReadyToMerge => DbTaskLifecycleState::ReadyToMerge,
        api_types::TaskLifecycleState::Merging => DbTaskLifecycleState::Merging,
        api_types::TaskLifecycleState::Done => DbTaskLifecycleState::Done,
        api_types::TaskLifecycleState::Cancelled => DbTaskLifecycleState::Cancelled,
    };
    let cause = match request.gate_evaluation_id {
        Some(evaluation_id)
            if matches!(
                to_state,
                DbTaskLifecycleState::ReadyToMerge | DbTaskLifecycleState::Active
            ) =>
        {
            services::task_lifecycle::LifecycleCause::GateEvaluation(evaluation_id)
        }
        Some(_) => {
            return Err(ApiError::bad_request_with_code(
                "task_lifecycle.unexpected_gate_evaluation",
                "gate_evaluation_id is accepted only for exact merge-readiness lifecycle edges",
            ));
        }
        None if to_state == DbTaskLifecycleState::ReadyToMerge => {
            return Err(ApiError::bad_request_with_code(
                "task_lifecycle.gate_evaluation_required",
                "ready_to_merge requires the exact GateEvaluation that admitted it",
            ));
        }
        None => services::task_lifecycle::LifecycleCause::Actor(api_types::Actor::User {
            user_id: Some(user.user_id.clone()),
            source: api_types::UserActionSource::Api,
        }),
    };
    let result = services::task_lifecycle::TaskLifecycleService::new(
        Arc::clone(&state.db),
        Arc::clone(&state.event_bus),
    )
    .transition_at_lifecycle_version(
        services::task_lifecycle::TransitionLifecycleInput {
            task_id: task_id.clone(),
            expected_task_version: task.version,
            to_state,
            cause,
            reason_kind: request.reason_kind,
            reason_ref: request.reason_ref,
            idempotency_key: request.idempotency_key,
        },
        request.expected_lifecycle_version,
    )
    .await
    .map_err(ApiError::from)?;
    let (transition_id, gate_evaluation_id, replayed) = match result.transition {
        Some(transition) => (
            Some(transition.transition_id),
            transition.gate_evaluation_id,
            transition.replayed,
        ),
        None => (None, None, false),
    };
    Ok(Json(TaskLifecycleTransitionResponse {
        task_id,
        lifecycle: lifecycle_response(result.lifecycle),
        transition_id,
        gate_evaluation_id,
        replayed,
    }))
}

pub async fn list_task_lifecycle_transitions(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
) -> ApiResult<Json<Vec<TaskLifecycleTransitionFactResponse>>> {
    super::require_task_visible(&state, &task_id, &user).await?;
    let transitions = TaskLifecycleRepo::list_task_lifecycle_transitions(&*state.db, &task_id, 100)
        .await?
        .into_iter()
        .map(|transition| TaskLifecycleTransitionFactResponse {
            id: transition.id,
            task_id: transition.task_id,
            from_state: lifecycle_state_response(transition.from_state),
            to_state: lifecycle_state_response(transition.to_state),
            from_version: transition.from_version,
            to_version: transition.to_version,
            cause_kind: transition.cause_kind,
            cause_ref: transition.cause_ref,
            gate_evaluation_id: transition.gate_evaluation_id,
            reason_kind: transition.reason_kind,
            reason_ref: transition.reason_ref,
            domain_event_id: transition.domain_event_id,
            created_at: transition.created_at,
        })
        .collect();
    Ok(Json(transitions))
}

fn lifecycle_state_response(state: DbTaskLifecycleState) -> api_types::TaskLifecycleState {
    match state {
        DbTaskLifecycleState::Backlog => api_types::TaskLifecycleState::Backlog,
        DbTaskLifecycleState::Ready => api_types::TaskLifecycleState::Ready,
        DbTaskLifecycleState::Active => api_types::TaskLifecycleState::Active,
        DbTaskLifecycleState::Blocked => api_types::TaskLifecycleState::Blocked,
        DbTaskLifecycleState::ReadyToMerge => api_types::TaskLifecycleState::ReadyToMerge,
        DbTaskLifecycleState::Merging => api_types::TaskLifecycleState::Merging,
        DbTaskLifecycleState::Done => api_types::TaskLifecycleState::Done,
        DbTaskLifecycleState::Cancelled => api_types::TaskLifecycleState::Cancelled,
    }
}

fn policy_response(policy: GatePolicyRevision) -> ApiResult<GatePolicyRevisionResponse> {
    Ok(GatePolicyRevisionResponse {
        gate_id: policy.gate_id,
        revision: policy.revision,
        schema_version: policy.schema_version,
        policy: serde_json::from_str(&policy.policy_json)
            .map_err(|error| ApiError::internal(error.to_string()))?,
        policy_digest: policy.policy_digest,
        created_at: policy.created_at,
    })
}

fn evaluation_response(
    evaluation: GateEvaluation,
    inputs: Vec<GateEvaluationInput>,
) -> ApiResult<GateEvaluationResponse> {
    Ok(GateEvaluationResponse {
        id: evaluation.id,
        gate_id: evaluation.gate_id,
        task_id: evaluation.task_id,
        policy_revision: evaluation.policy_revision,
        outcome: evaluation.outcome.to_string(),
        input_digest: evaluation.input_digest,
        result: serde_json::from_str(&evaluation.result_json)
            .map_err(|error| ApiError::internal(error.to_string()))?,
        evaluated_at: evaluation.evaluated_at,
        inputs: inputs
            .into_iter()
            .map(evaluation_input_response)
            .collect::<ApiResult<Vec<_>>>()?,
    })
}

fn evaluation_input_response(input: GateEvaluationInput) -> ApiResult<GateEvaluationInputResponse> {
    Ok(GateEvaluationInputResponse {
        ordinal: input.ordinal,
        input_kind: input.input_kind,
        input_id: input.input_id,
        input_version: input.input_version,
        input_digest: input.input_digest,
        producer_ref: input.producer_ref,
        subject: serde_json::from_str(&input.subject_json)
            .map_err(|error| ApiError::internal(error.to_string()))?,
        status: input.status,
    })
}

pub async fn create_task_gate(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
    Json(request): Json<CreateTaskGateRequest>,
) -> ApiResult<Json<GateResponse>> {
    require_task_visible(&state, &task_id, &user).await?;
    let policy = serde_json::from_value(request.policy)
        .map_err(|error| ApiError::bad_request(format!("invalid Gate policy: {error}")))?;
    let engine = GateEngine::new(Arc::clone(&state.db), Arc::clone(&state.event_bus));
    let (gate, revision) = engine
        .create_gate_with_initial_policy(
            &task_id,
            &request.gate_kind,
            GateScopeKind::Task,
            &task_id,
            policy,
        )
        .await
        .map_err(ApiError::from)?;
    Ok(Json(GateResponse {
        gate: gate_response(gate),
        active_policy: Some(policy_response(revision)?),
    }))
}

pub async fn list_task_gates(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
) -> ApiResult<Json<Vec<GateResponse>>> {
    require_task_visible(&state, &task_id, &user).await?;
    let gates = GateRepo::list_active_gate_policies(&*state.db, &task_id).await?;
    let responses = gates
        .into_iter()
        .map(|(gate, active_policy)| {
            Ok(GateResponse {
                gate: gate_response(gate),
                active_policy: Some(policy_response(active_policy)?),
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(responses))
}

pub async fn get_gate(
    State(state): State<AppState>,
    Path(gate_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
) -> ApiResult<Json<GateResponse>> {
    let gate = GateRepo::get_gate(&*state.db, &gate_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Gate", gate_id.clone()))?;
    require_task_visible(&state, &gate.task_id, &user).await?;
    let active_policy = match gate.active_policy_revision {
        Some(revision) => Some(policy_response(
            GateRepo::get_gate_policy_revision(&*state.db, &gate.id, revision)
                .await?
                .ok_or_else(|| ApiError::not_found("Gate policy revision", gate.id.clone()))?,
        )?),
        None => None,
    };
    Ok(Json(GateResponse {
        gate: gate_response(gate),
        active_policy,
    }))
}

pub async fn revise_gate_policy(
    State(state): State<AppState>,
    Path(gate_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
    Json(request): Json<ReviseGatePolicyRequest>,
) -> ApiResult<Json<GatePolicyRevisionResponse>> {
    let gate = GateRepo::get_gate(&*state.db, &gate_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Gate", gate_id.clone()))?;
    require_task_visible(&state, &gate.task_id, &user).await?;
    let policy = serde_json::from_value(request.policy)
        .map_err(|error| ApiError::bad_request(format!("invalid Gate policy: {error}")))?;
    let revision = GateEngine::new(Arc::clone(&state.db), Arc::clone(&state.event_bus))
        .revise_policy(&gate_id, request.expected_active_revision, policy)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(policy_response(revision)?))
}

pub async fn evaluate_gate(
    State(state): State<AppState>,
    Path(gate_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
) -> ApiResult<Json<GateEvaluationResponse>> {
    let gate = GateRepo::get_gate(&*state.db, &gate_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Gate", gate_id.clone()))?;
    require_task_visible(&state, &gate.task_id, &user).await?;
    let evaluation = GateEngine::new(Arc::clone(&state.db), Arc::clone(&state.event_bus))
        .evaluate_active(&gate_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(evaluation_response(
        evaluation.evaluation,
        evaluation.inputs,
    )?))
}

pub async fn get_gate_evaluation(
    State(state): State<AppState>,
    Path(evaluation_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
) -> ApiResult<Json<GateEvaluationResponse>> {
    let evaluation = GateRepo::get_gate_evaluation(&*state.db, &evaluation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("GateEvaluation", evaluation_id.clone()))?;
    require_task_visible(&state, &evaluation.task_id, &user).await?;
    let inputs = GateRepo::list_gate_evaluation_inputs(&*state.db, &evaluation.id).await?;
    Ok(Json(evaluation_response(evaluation, inputs)?))
}

pub async fn merge_after_gate(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: crate::routes::auth::AuthenticatedUser,
    Json(request): Json<MergeAfterGateRequest>,
) -> ApiResult<Json<MergeAfterGateResponse>> {
    require_task_visible(&state, &task_id, &user).await?;
    let outcome = state
        .merge_service
        .merge_after_gate(task_id, &request.gate_evaluation_id)
        .await
        .map_err(ApiError::from)?;
    let response = match outcome {
        services::MergeOutcome::Done {
            before_sha,
            after_sha,
            branch,
        } => MergeAfterGateResponse {
            outcome: "done".to_owned(),
            before_sha: Some(before_sha),
            after_sha: Some(after_sha),
            branch: Some(branch),
            pr_url: None,
            target_branch: None,
            details: None,
            files: Vec::new(),
        },
        services::MergeOutcome::PullRequest {
            pr_url,
            branch,
            target_branch,
        } => MergeAfterGateResponse {
            outcome: "pull_request".to_owned(),
            before_sha: None,
            after_sha: None,
            branch: Some(branch),
            pr_url,
            target_branch: Some(target_branch),
            details: None,
            files: Vec::new(),
        },
        services::MergeOutcome::Conflict {
            details,
            conflict_paths,
        } => MergeAfterGateResponse {
            outcome: "conflict".to_owned(),
            before_sha: None,
            after_sha: None,
            branch: None,
            pr_url: None,
            target_branch: None,
            details: Some(details),
            files: conflict_paths
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        },
        services::MergeOutcome::Dirty { files } => MergeAfterGateResponse {
            outcome: "dirty".to_owned(),
            before_sha: None,
            after_sha: None,
            branch: None,
            pr_url: None,
            target_branch: None,
            details: None,
            files,
        },
        services::MergeOutcome::TargetDirty { files } => MergeAfterGateResponse {
            outcome: "target_dirty".to_owned(),
            before_sha: None,
            after_sha: None,
            branch: None,
            pr_url: None,
            target_branch: None,
            details: None,
            files,
        },
    };
    Ok(Json(response))
}
