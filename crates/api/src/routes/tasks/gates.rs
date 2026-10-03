use super::*;

pub async fn approve_gate(
    State(state): State<AppState>,
    user: crate::routes::auth::AuthenticatedUser,
    Path((id, state_name)): Path<(String, String)>,
    Json(request): Json<ApproveGateRequest>,
) -> ApiResult<Json<TaskResponse>> {
    if state_name == default_states::REVIEW {
        return project_review_gate_decision(
            &state,
            &user,
            &id,
            request.version,
            GateDecision::Approve,
            request.reason,
        )
        .await
        .map(Json);
    }
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
    user: crate::routes::auth::AuthenticatedUser,
    Path((id, state_name)): Path<(String, String)>,
    Json(request): Json<RejectGateRequest>,
) -> ApiResult<Json<TaskResponse>> {
    if state_name == default_states::REVIEW {
        return project_review_gate_decision(
            &state,
            &user,
            &id,
            request.version,
            GateDecision::Reject,
            Some(request.reason),
        )
        .await
        .map(Json);
    }
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

/// The legacy generic gate URL has no Execution id in its request. Preserve
/// it only as a projection when exactly one live Human reviewer Execution for
/// this user exists; the durable verdict is still its exact ReviewReport.
async fn project_review_gate_decision(
    state: &AppState,
    user: &crate::routes::auth::AuthenticatedUser,
    task_id: &str,
    expected_version: i64,
    decision: GateDecision,
    reason: Option<String>,
) -> ApiResult<TaskResponse> {
    let task = TaskRepo::get_by_id(&*state.db, task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.to_owned()))?;
    if task.status != default_states::REVIEW {
        return Err(ApiError::invalid_operation_conflict(format!(
            "task {task_id} is in {} state; expected review",
            task.status
        )));
    }
    if task.version != expected_version {
        return Err(ApiError::invalid_operation_conflict(
            "Task changed since the review decision was prepared",
        ));
    }

    let page = ExecutionRepo::list_by_task_and_role(
        &*state.db,
        task_id,
        services::workflow::default_roles::REVIEWER,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    let mut candidates = page.items.into_iter().filter(|execution| {
        execution.purpose == Some(db::ExecutionPurpose::Review)
            && execution.status == ExecutionStatus::Running
            && execution.actor_ref() == Some(db::ActorRef::Human(user.user_id.clone()))
    });
    let Some(execution) = candidates.next() else {
        return Err(ApiError::invalid_operation_conflict(
            "Review decision requires one running Human reviewer Execution for this user",
        ));
    };
    if candidates.next().is_some() || page.next_cursor.is_some() {
        return Err(ApiError::invalid_operation_conflict(
            "Review decision is ambiguous; submit a ReviewReport to the exact Execution id",
        ));
    }

    let (verdict, summary, findings) = match decision {
        GateDecision::Approve => (
            api_types::ReviewReportVerdict::Pass,
            reason
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or_else(|| "Human reviewer approved this Task.".to_owned()),
            Vec::new(),
        ),
        GateDecision::Reject => {
            let reason = reason
                .filter(|reason| !reason.trim().is_empty())
                .ok_or_else(|| ApiError::bad_request("Review changes require a reason"))?;
            (
                api_types::ReviewReportVerdict::RequestChanges,
                reason.clone(),
                vec![reason],
            )
        }
    };
    state
        .task_service
        .submit_human_review_report(
            &execution.id,
            &user.user_id,
            api_types::SubmitReviewReportRequest {
                verdict,
                summary,
                criteria: vec!["Human review decision".to_owned()],
                findings,
                questions: Vec::new(),
                evidence_ids: Vec::new(),
                artifact_ids: Vec::new(),
            },
        )
        .await?;
    let updated = TaskRepo::get_by_id(&*state.db, task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.to_owned()))?;
    let mut response = task_response(&state.db, updated).await?;
    response.awaiting_human = state
        .task_service
        .is_awaiting_human(task_id.to_owned())
        .await?;
    Ok(response)
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
    let workflow = WorkflowEngine::resolve_workflow_for_task(
        &task,
        &project.workflow_definition,
        &api_types::Actor::user(api_types::UserActionSource::Api),
    );
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
    if state_name == default_states::REVIEW {
        return Err(ApiError::invalid_operation_conflict(
            "Review gates require a completed reviewer Execution and exact ReviewReport; task-level approval is retired",
        ));
    }
    ensure_gate_decision_ready(state, &task_id, gate_state).await?;

    let target_state = gate_decision_target(&workflow, &state_name, decision)?;
    let trigger_reason = gate_decision_reason(decision, reason);
    let result = state
        .task_service
        .transition(
            task_id.clone(),
            target_state,
            (
                version,
                Some(trigger_reason.clone()),
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
    if page.items.iter().any(|execution| {
        execution.work_unit_id.is_none()
            && execution.role == role
            && execution.status == ExecutionStatus::Running
    }) {
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
