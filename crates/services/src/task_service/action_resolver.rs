use api_types::{
    ExecutionAction, ExecutionActionKind, ExecutionStatus, RecoveryAction, StateKind,
    TaskBlockingAnnotation, WorkflowDefinition,
};

const INTERACTIVE_ROLE: &str = "interactive";

pub fn resolve_execution_actions(
    task: &db::Task,
    workflow: &WorkflowDefinition,
    executions: &[db::Execution],
    blocking_annotation: Option<&TaskBlockingAnnotation>,
) -> Vec<ExecutionAction> {
    let is_terminal = workflow.state_kind(&task.status) == Some(StateKind::Terminal);
    let current_state = workflow
        .states
        .iter()
        .find(|state| state.name == task.status);
    let effective_role = current_state.and_then(|state| {
        state
            .role
            .as_deref()
            .or_else(|| (state.kind == StateKind::Active).then_some("assignee"))
    });

    let running_executions: Vec<&db::Execution> = executions
        .iter()
        .filter(|execution| execution_status(execution) == ExecutionStatus::Running)
        .collect();
    let has_running_execution = !running_executions.is_empty();
    let has_running_interactive_execution = running_executions
        .iter()
        .any(|execution| execution.role == INTERACTIVE_ROLE);

    let latest_non_running_execution = executions
        .iter()
        .filter(|execution| execution_status(execution) != ExecutionStatus::Running)
        .max_by(|left, right| left.created_at.cmp(&right.created_at));
    let latest_resumable_execution = executions
        .iter()
        .filter(|execution| {
            execution_status(execution) != ExecutionStatus::Running
                && execution.agent_session_id.is_some()
        })
        .max_by(|left, right| left.created_at.cmp(&right.created_at));

    let blocked_execution = blocking_annotation
        .and_then(|annotation| annotation.blocked_execution_id.as_deref())
        .and_then(|execution_id| {
            executions
                .iter()
                .find(|execution| execution.id == execution_id)
        });
    let has_resume_recovery_action = blocking_annotation.is_some_and(|annotation| {
        annotation
            .recovery_actions
            .contains(&RecoveryAction::ResumeSession)
    });
    let has_recovery_session = has_resume_recovery_action
        && blocked_execution
            .and_then(|execution| execution.agent_session_id.as_ref())
            .is_some();
    let blocked_role_matches = effective_role
        .zip(blocked_execution.map(|execution| execution.role.as_str()))
        .is_some_and(|(role, blocked_role)| role == blocked_role);
    let retry_budget_exhausted_reason = blocking_annotation.and_then(retry_budget_exhausted_reason);

    let re_execute_target = effective_role.and_then(|role| {
        executions
            .iter()
            .filter(|execution| {
                execution_status(execution) != ExecutionStatus::Running && execution.role == role
            })
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
    });
    let has_previous_execution_for_role = re_execute_target.is_some();
    let has_running_execution_for_role = effective_role.is_some_and(|role| {
        running_executions
            .iter()
            .any(|execution| execution.role == role)
    });

    vec![
        action(
            ExecutionActionKind::ManualLaunch,
            "Start Manual Execution",
            !is_terminal && !has_running_interactive_execution,
            false,
            false,
            if is_terminal {
                Some("Task is in terminal state".to_owned())
            } else if has_running_interactive_execution {
                Some("An execution is already running".to_owned())
            } else {
                None
            },
        ),
        action_with_target(
            ExecutionActionKind::SessionFollowUp,
            "Continue Session Manually",
            !is_terminal
                && latest_resumable_execution.is_some()
                && !has_running_interactive_execution,
            false,
            true,
            if is_terminal {
                Some("Task is in terminal state".to_owned())
            } else if has_running_interactive_execution {
                Some("An execution is already running".to_owned())
            } else if latest_resumable_execution.is_none() {
                Some(no_resumable_session_reason(effective_role))
            } else {
                None
            },
            latest_resumable_execution.map(|e| e.id.as_str()),
        ),
        action_with_target(
            ExecutionActionKind::WorkflowResume,
            format!("Resume {}", effective_role.unwrap_or("Execution")),
            !is_terminal
                && retry_budget_exhausted_reason.is_none()
                && has_recovery_session
                && blocked_role_matches,
            true,
            true,
            if is_terminal {
                Some("Task is in terminal state".to_owned())
            } else if retry_budget_exhausted_reason.is_some() {
                retry_budget_exhausted_reason.clone()
            } else if !has_recovery_session {
                Some(no_resumable_session_reason(effective_role))
            } else if !blocked_role_matches {
                Some(role_mismatch_reason(
                    blocked_execution.map(|execution| execution.role.as_str()),
                    effective_role,
                ))
            } else {
                None
            },
            blocked_execution.map(|e| e.id.as_str()),
        ),
        action_with_target(
            ExecutionActionKind::ReExecute,
            format!("Re-execute {}", effective_role.unwrap_or("Execution")),
            !is_terminal
                && retry_budget_exhausted_reason.is_none()
                && has_previous_execution_for_role
                && !has_running_execution_for_role,
            true,
            false,
            if is_terminal {
                Some("Task is in terminal state".to_owned())
            } else if retry_budget_exhausted_reason.is_some() {
                retry_budget_exhausted_reason.clone()
            } else if !has_previous_execution_for_role {
                Some(re_execute_unavailable_reason(
                    latest_non_running_execution.map(|execution| execution.role.as_str()),
                    effective_role,
                ))
            } else if has_running_execution_for_role {
                Some("An execution is already running".to_owned())
            } else {
                None
            },
            re_execute_target.map(|e| e.id.as_str()),
        ),
        action(
            ExecutionActionKind::StopExecution,
            "Stop Execution",
            has_running_execution,
            false,
            false,
            if has_running_execution {
                None
            } else {
                Some("No running execution".to_owned())
            },
        ),
        action(
            ExecutionActionKind::CancelTask,
            "Cancel Task",
            !is_terminal,
            false,
            false,
            if is_terminal {
                Some("Task is already in terminal state".to_owned())
            } else {
                None
            },
        ),
    ]
}

fn no_resumable_session_reason(role: Option<&str>) -> String {
    format!(
        "No resumable {} session available",
        role.unwrap_or("execution")
    )
}

fn role_mismatch_reason(other_role: Option<&str>, current_role: Option<&str>) -> String {
    format!(
        "Latest execution is for {}, not {}",
        other_role.unwrap_or("unknown"),
        current_role.unwrap_or("current role")
    )
}

fn re_execute_unavailable_reason(
    latest_role: Option<&str>,
    effective_role: Option<&str>,
) -> String {
    match (latest_role, effective_role) {
        (Some(other_role), Some(current_role)) if other_role != current_role => {
            role_mismatch_reason(Some(other_role), Some(current_role))
        }
        _ => format!(
            "No previous {} execution available",
            effective_role.unwrap_or("role")
        ),
    }
}

fn retry_budget_exhausted_reason(annotation: &TaskBlockingAnnotation) -> Option<String> {
    // Annotations here may be synthesized from blocked metadata (see
    // blocked_metadata_annotation in the api layer), so both exhaustion
    // vocabularies apply.
    let exhausted = annotation.annotation_type.is_budget_exhausted_annotation()
        || annotation.annotation_type.is_retry_exhausted_metadata();
    exhausted.then(|| {
        format!(
            "Retry budget exhausted for {}",
            retry_budget_gate(annotation)
        )
    })
}

fn retry_budget_gate(annotation: &TaskBlockingAnnotation) -> String {
    match annotation.annotation_type {
        api_types::FailureKind::ReviewBudgetExhausted => "review".to_owned(),
        api_types::FailureKind::MergeFixBudgetExhausted => "merge_fix".to_owned(),
        _ => annotation.blocking_reason.clone(),
    }
}

fn action(
    action: ExecutionActionKind,
    label: impl Into<String>,
    enabled: bool,
    propagates: bool,
    requires_session: bool,
    disabled_reason: Option<String>,
) -> ExecutionAction {
    action_with_target(
        action,
        label,
        enabled,
        propagates,
        requires_session,
        disabled_reason,
        None,
    )
}

fn action_with_target(
    action: ExecutionActionKind,
    label: impl Into<String>,
    enabled: bool,
    propagates: bool,
    requires_session: bool,
    disabled_reason: Option<String>,
    target_execution_id: Option<&str>,
) -> ExecutionAction {
    ExecutionAction {
        action,
        label: label.into(),
        enabled,
        propagates,
        requires_session,
        disabled_reason,
        target_execution_id: target_execution_id.map(str::to_owned),
    }
}

fn execution_status(execution: &db::Execution) -> ExecutionStatus {
    match execution.status {
        db::ExecutionStatus::Running => ExecutionStatus::Running,
        db::ExecutionStatus::Completed => ExecutionStatus::Completed,
        db::ExecutionStatus::Failed => ExecutionStatus::Failed,
        db::ExecutionStatus::Cancelled => ExecutionStatus::Cancelled,
    }
}
