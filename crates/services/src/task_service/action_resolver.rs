use api_types::{
    ExecutionAction, ExecutionActionKind, ExecutionStatus, RecoveryAction, StateKind,
    TaskBlockingAnnotation, WorkflowDefinition,
};
use std::collections::HashSet;

const INTERACTIVE_ROLE: &str = "interactive";

pub fn resolve_execution_actions(
    task: &db::Task,
    workflow: &WorkflowDefinition,
    executions: &[db::Execution],
    blocking_annotation: Option<&TaskBlockingAnnotation>,
) -> Vec<ExecutionAction> {
    resolve_execution_actions_with_session_state(
        task,
        workflow,
        executions,
        blocking_annotation,
        None,
        None,
    )
}

/// Resolve actions with the set of generic HarnessSessions that are known to
/// be active and externally identified. The plain resolver remains useful for
/// callers that already have an execution projection but the API route passes
/// this set so pending sessions are never advertised as resumable.
pub fn resolve_execution_actions_with_session_state(
    task: &db::Task,
    workflow: &WorkflowDefinition,
    executions: &[db::Execution],
    blocking_annotation: Option<&TaskBlockingAnnotation>,
    reusable_harness_session_ids: Option<&HashSet<String>>,
    current_workspace_id: Option<&str>,
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
                && has_resumable_session(execution, reusable_harness_session_ids, current_workspace_id)
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
        && blocked_execution.is_some_and(|execution| {
            has_resumable_session(
                execution,
                reusable_harness_session_ids,
                current_workspace_id,
            )
        });
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

fn has_resumable_session(
    execution: &db::Execution,
    reusable_harness_session_ids: Option<&HashSet<String>>,
    current_workspace_id: Option<&str>,
) -> bool {
    if let Some(session_id) = execution.harness_session_id.as_deref() {
        return reusable_harness_session_ids
            .map(|ids| ids.contains(session_id))
            .unwrap_or(false);
    }
    // Bounded historical compatibility: only rows without a generic
    // reference may use the old external-session projection.
    let historical_agent_ref = match execution.actor_ref() {
        None => true,
        Some(db::ActorRef::Agent(actor_id)) => {
            execution.agent_id.as_deref() == Some(actor_id.as_str())
        }
        Some(db::ActorRef::Human(_)) => false,
    };
    historical_agent_ref
        && execution.agent_id.is_some()
        && !execution
            .agent_id
            .as_deref()
            .is_some_and(|agent_id| agent_id.eq_ignore_ascii_case("human"))
        && execution
            .agent_session_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        && (execution.workspace_id.is_none()
            || execution.workspace_id.as_deref() == current_workspace_id)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn execution(
        id: &str,
        actor_ref: Option<db::ActorRef>,
        harness_session_id: Option<&str>,
        agent_session_id: Option<&str>,
    ) -> db::Execution {
        let (actor_kind, actor_id) = match actor_ref {
            Some(db::ActorRef::Agent(id)) => (Some(db::ActorKind::Agent), Some(id)),
            Some(db::ActorRef::Human(id)) => (Some(db::ActorKind::Human), Some(id)),
            None => (None, None),
        };
        db::Execution {
            id: id.to_owned(),
            task_id: "task".to_owned(),
            agent_id: actor_id.clone(),
            actor_kind,
            actor_id,
            role: "coder".to_owned(),
            purpose: Some(db::ExecutionPurpose::Implement),
            status: db::ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: agent_session_id.map(str::to_owned),
            harness_session_id: harness_session_id.map(str::to_owned),
            agent_message_id: None,
            last_activity_at: None,
            prompt: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn explicit_harness_state_controls_resumability() {
        let pending = execution(
            "pending",
            Some(db::ActorRef::Agent("a".to_owned())),
            Some("hs"),
            None,
        );
        assert!(!has_resumable_session(&pending, Some(&HashSet::new()), None));

        let active = execution(
            "active",
            Some(db::ActorRef::Agent("a".to_owned())),
            Some("hs"),
            Some("legacy"),
        );
        let active_ids = HashSet::from(["hs".to_owned()]);
        assert!(has_resumable_session(&active, Some(&active_ids), None));

        let agent_legacy = execution(
            "agent-legacy",
            Some(db::ActorRef::Agent("a".to_owned())),
            None,
            Some("legacy"),
        );
        assert!(!has_resumable_session(&agent_legacy, None, None));

        let mut historical = execution("historical", None, None, Some("legacy"));
        historical.agent_id = Some("a".to_owned());
        assert!(has_resumable_session(&historical, None, None));

        let agentless = execution("agentless", None, None, Some("legacy"));
        assert!(!has_resumable_session(&agentless, None, None));

        let mut workspace_scoped = execution("workspace", None, None, Some("legacy"));
        workspace_scoped.agent_id = Some("a".to_owned());
        workspace_scoped.workspace_id = Some("workspace-1".to_owned());
        assert!(!has_resumable_session(
            &workspace_scoped,
            None,
            Some("workspace-2")
        ));
        assert!(has_resumable_session(
            &workspace_scoped,
            None,
            Some("workspace-1")
        ));
    }
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
