use super::*;

pub async fn approve_gate(
    State(state): State<AppState>,
    Path((id, state_name)): Path<(String, String)>,
    Json(request): Json<ApproveGateRequest>,
) -> ApiResult<Json<TaskResponse>> {
    let task = transition_gate(
        &state,
        id,
        state_name,
        request.version,
        request.reason,
        GateDecision::Approve,
    )
    .await?;
    Ok(Json(task))
}

pub async fn reject_gate(
    State(state): State<AppState>,
    Path((id, state_name)): Path<(String, String)>,
    Json(request): Json<RejectGateRequest>,
) -> ApiResult<Json<TaskResponse>> {
    let task = transition_gate(
        &state,
        id,
        state_name,
        request.version,
        Some(required_reject_reason(request.reason)?),
        GateDecision::Reject,
    )
    .await?;
    Ok(Json(task))
}

#[derive(Debug, Deserialize)]
pub struct GateRejectRequest {
    pub reason: String,
}

pub async fn gate_approve(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskResponse>> {
    let task = TaskRepo::get_by_id(&*state.db, &id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", id.clone()))?;
    let project = ProjectRepo::get_by_id(&*state.db, &task.project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", task.project_id.clone()))?;
    let workflow = WorkflowEngine::resolve_workflow_for(&task, &project.workflow_definition);
    if workflow.state_kind(&task.status) != Some(StateKind::Gate) {
        return Err(ApiError::invalid_operation_conflict(
            "task is not in a gate state",
        ));
    }
    let target = workflow
        .states
        .iter()
        .find(|state| state.name == task.status)
        .and_then(|state| state.triggers.get(&WorkflowTrigger::Accept))
        .map(|definition| definition.to.clone())
        .ok_or_else(|| ApiError::invalid_operation_conflict("gate has no approve target"))?;
    state
        .task_service
        .transition(
            id.clone(),
            target,
            TransitionOptions {
                version: task.version,
                reason: None,
                triggered_by: "user:api".to_owned(),
                rejection: false,
                defer_dispatch_seconds: None,
            },
        )
        .await?;
    let task = TaskRepo::get_by_id(&*state.db, &id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", id.clone()))?;
    Ok(Json(task_response(&state.db, task).await?))
}

pub async fn gate_reject(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<GateRejectRequest>,
) -> ApiResult<Json<TaskResponse>> {
    let task = TaskRepo::get_by_id(&*state.db, &id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", id.clone()))?;
    let project = ProjectRepo::get_by_id(&*state.db, &task.project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", task.project_id.clone()))?;
    let workflow = WorkflowEngine::resolve_workflow_for(&task, &project.workflow_definition);
    if workflow.state_kind(&task.status) != Some(StateKind::Gate) {
        return Err(ApiError::invalid_operation_conflict(
            "task is not in a gate state",
        ));
    }
    let reject_target = workflow
        .gate_reject_target(&task.status)
        .ok_or_else(|| ApiError::invalid_operation_conflict("gate does not support rejection"))?;
    state
        .task_service
        .transition(
            id.clone(),
            reject_target.to_owned(),
            TransitionOptions {
                version: task.version,
                reason: Some(required_reject_reason(body.reason.clone())?),
                triggered_by: "user:api".to_owned(),
                rejection: true,
                defer_dispatch_seconds: None,
            },
        )
        .await?;
    let task = TaskRepo::get_by_id(&*state.db, &id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", id.clone()))?;
    Ok(Json(task_response(&state.db, task).await?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateDecision {
    Approve,
    Reject,
}

async fn transition_gate(
    state: &AppState,
    task_id: String,
    state_name: String,
    version: i64,
    reason: Option<String>,
    decision: GateDecision,
) -> ApiResult<TaskResponse> {
    let task = TaskRepo::get_by_id(&*state.db, &task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.clone()))?;
    let project = ProjectRepo::get_by_id(&*state.db, &task.project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", task.project_id.clone()))?;
    let workflow = WorkflowEngine::resolve_workflow(&project.workflow_definition);
    let gate_state = workflow
        .states
        .iter()
        .find(|state| state.name == state_name)
        .ok_or_else(|| ApiError::bad_request(format!("state '{state_name}' is not defined")))?;
    if gate_state.kind != StateKind::Gate {
        return Err(ApiError::bad_request(format!(
            "state '{state_name}' is not a gate"
        )));
    }
    if task.status != state_name {
        return Err(ApiError::invalid_operation_conflict(format!(
            "task {task_id} is in {} state; expected {state_name}",
            task.status
        )));
    }
    ensure_gate_decision_ready(state, &task_id, gate_state).await?;

    if decision == GateDecision::Approve && state_name == default_states::REVIEW {
        let latest_review = ReviewRepo::list_by_task(&*state.db, &task_id)
            .await?
            .into_iter()
            .max_by_key(|review| review.attempt_number);
        if latest_review
            .as_ref()
            .is_some_and(|review| review.status == ReviewStatus::AwaitingHuman)
        {
            let (task, _) = state.task_service.approve_review(task_id).await?;
            let mut response = task_response(&state.db, task).await?;
            response.awaiting_human = state
                .task_service
                .is_awaiting_human(response.id.clone())
                .await?;
            return Ok(response);
        }
    }

    let target_state = gate_decision_target(&workflow, &state_name, decision)?;
    let trigger_reason = gate_decision_reason(decision, reason);
    let result = state
        .task_service
        .transition(
            task_id,
            target_state,
            (
                version,
                Some(trigger_reason),
                decision == GateDecision::Reject,
            ),
        )
        .await?;

    let mut response = task_response(&state.db, result.task).await?;
    response.awaiting_human = state
        .task_service
        .is_awaiting_human(response.id.clone())
        .await?;
    Ok(response)
}

async fn ensure_gate_decision_ready(
    state: &AppState,
    task_id: &str,
    gate_state: &api_types::StateDefinition,
) -> ApiResult<()> {
    let Some(role) = gate_state.role.as_deref() else {
        return Ok(());
    };
    let page = ExecutionRepo::list_by_task(
        &*state.db,
        task_id,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    if page
        .items
        .iter()
        .any(|execution| execution.role == role && execution.status == ExecutionStatus::Running)
    {
        return Err(ApiError::invalid_operation_conflict(format!(
            "gate '{}' is still running {role} execution; wait for it to finish before approving or rejecting",
            gate_state.name
        )));
    }
    Ok(())
}

fn gate_decision_target(
    workflow: &WorkflowDefinition,
    gate_state: &str,
    decision: GateDecision,
) -> ApiResult<String> {
    let gate = workflow
        .states
        .iter()
        .find(|state| state.name == gate_state)
        .ok_or_else(|| ApiError::bad_request(format!("unknown gate state '{gate_state}'")))?;

    let trigger_target = |trigger: WorkflowTrigger| {
        gate.triggers
            .get(&trigger)
            .map(|definition| definition.to.as_str())
    };

    match decision {
        GateDecision::Approve => {
            if let Some(target) = trigger_target(WorkflowTrigger::Accept) {
                return Ok(target.to_owned());
            }

            Err(ApiError::bad_request(format!(
                "gate '{gate_state}' has no approve target"
            )))
        }
        GateDecision::Reject => {
            if let Some(reject_target) = workflow
                .states
                .iter()
                .find(|state| state.name == gate_state)
                .and_then(|state| state.gate_config.as_ref())
                .and_then(|config| config.reject_target.as_deref())
            {
                if trigger_target(WorkflowTrigger::Reject) == Some(reject_target) {
                    return Ok(reject_target.to_owned());
                }
            }

            if let Some(target) = trigger_target(WorkflowTrigger::Reject) {
                return Ok(target.to_owned());
            }

            Err(ApiError::bad_request(format!(
                "gate '{gate_state}' has no reject target"
            )))
        }
    }
}

fn gate_decision_reason(decision: GateDecision, reason: Option<String>) -> String {
    let prefix = match decision {
        GateDecision::Approve => "gate approved",
        GateDecision::Reject => "gate rejected",
    };
    reason
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(|value| format!("{prefix}: {value}"))
        .unwrap_or_else(|| prefix.to_owned())
}

fn required_reject_reason(reason: String) -> ApiResult<String> {
    let reason = reason.trim().to_owned();
    if reason.is_empty() {
        return Err(ApiError::bad_request("rejection reason is required"));
    }
    Ok(reason)
}
