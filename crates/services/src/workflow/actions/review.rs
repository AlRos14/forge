use std::sync::Arc;

use async_trait::async_trait;
use db::{ArtifactKind, CollaborationRepo, ExecutionPurpose, ExecutionRepo, ExecutionStatus};

use crate::workflow::{
    default_states, effective_role, engine::WorkflowEngine, HookAction, HookContext, HookResult,
};

use super::common::{get_role_assignment, review_ci_steps};

pub struct RunCiSteps;

#[async_trait]
impl HookAction for RunCiSteps {
    async fn execute(&self, ctx: &HookContext) -> HookResult {
        let ci_steps = match review_ci_steps(&ctx.state_config) {
            Ok(ci_steps) => ci_steps,
            Err(reason) => return HookResult::Failed { reason },
        };
        if ci_steps.is_empty() {
            return HookResult::Skipped {
                reason: "no ci steps".to_string(),
            };
        }

        let Some(workspace_id) = ctx.workspace_id.as_deref() else {
            return HookResult::Failed {
                reason: "deterministic validation requires an exact Workspace identity".to_owned(),
            };
        };
        let validation =
            crate::ValidationService::new(Arc::clone(&ctx.db), Arc::clone(&ctx.event_bus));
        for (index, command) in ci_steps.iter().enumerate() {
            let result = match validation
                .run_command(
                    &ctx.task_id,
                    workspace_id,
                    command,
                    index,
                    ctx.execution_id.as_deref(),
                    ctx.workspace_exec_locks.as_deref(),
                )
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    return HookResult::Failed {
                        reason: error.to_string(),
                    }
                }
            };
            if result.run.status != db::ValidationRunStatus::Passed {
                return HookResult::Failed {
                    reason: format!(
                        "ValidationRun {} for check {} ended as {}",
                        result.run.id, result.run.check_identity, result.run.status
                    ),
                };
            }
        }
        HookResult::Ok
    }
}

pub struct AutoCascadeOnReviewPass;

#[async_trait]
impl HookAction for AutoCascadeOnReviewPass {
    async fn execute(&self, ctx: &HookContext) -> HookResult {
        let Some(execution_id) = ctx.execution_id.as_deref() else {
            return HookResult::Skipped {
                reason: "no exact Review Execution is attached to this transition".to_owned(),
            };
        };
        let execution = match ExecutionRepo::get_by_id(&*ctx.db, execution_id).await {
            Ok(Some(execution)) => execution,
            Ok(None) => {
                return HookResult::Failed {
                    reason: "Review Execution not found".into(),
                }
            }
            Err(error) => {
                return HookResult::Failed {
                    reason: error.to_string(),
                }
            }
        };
        if execution.task_id != ctx.task_id
            || execution.role != crate::workflow::default_roles::REVIEWER
            || execution.purpose != Some(ExecutionPurpose::Review)
        {
            return HookResult::Skipped {
                reason: "transition is not attached to an exact reviewer Review Execution".into(),
            };
        }
        if execution.status != ExecutionStatus::Completed {
            return HookResult::Ok;
        }
        let report = match CollaborationRepo::get_execution_artifact_output(
            &*ctx.db,
            execution_id,
            ArtifactKind::ReviewReport,
        )
        .await
        {
            Ok(Some(report)) => report,
            Ok(None) => {
                return HookResult::Failed {
                    reason: "completed Review Execution has no ReviewReport Artifact".into(),
                }
            }
            Err(error) => {
                return HookResult::Failed {
                    reason: error.to_string(),
                }
            }
        };
        let parsed = report
            .content
            .as_deref()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(content).ok());
        let Some(verdict) = parsed
            .as_ref()
            .and_then(|value| value.get("verdict"))
            .and_then(serde_json::Value::as_str)
        else {
            return HookResult::Failed {
                reason: "ReviewReport has no structured verdict".into(),
            };
        };
        let human_review_execution = matches!(execution.actor_ref(), Some(db::ActorRef::Human(_)));
        match verdict {
            // A Human reviewer Execution is the exact cognitive review and
            // therefore carries the former approve/reject action in its
            // ReviewReport. Requiring the retired task-level approval after
            // that report would create a second, actor-less verdict source.
            "pass" if human_review_execution || !gate_requires_user_approval(ctx) => {
                HookResult::Cascade {
                    to: default_states::MERGING.to_owned(),
                    reason: format!("ReviewReport {} passed", report.id),
                }
            }
            "request_changes" => HookResult::Cascade {
                to: ctx
                    .workflow
                    .states
                    .iter()
                    .find(|state| state.name == ctx.to_state)
                    .and_then(|state| state.gate_config.as_ref())
                    .and_then(|gate| gate.reject_target.clone())
                    .unwrap_or_else(|| default_states::IN_PROGRESS.to_owned()),
                reason: format!("ReviewReport {} requests changes", report.id),
            },
            "pass" | "questions" => HookResult::Ok,
            _ => HookResult::Failed {
                reason: "ReviewReport verdict is outside the supported contract".into(),
            },
        }
    }
}

pub struct AutoCascadeOnUnconfiguredReview;

#[async_trait]
impl HookAction for AutoCascadeOnUnconfiguredReview {
    async fn execute(&self, ctx: &HookContext) -> HookResult {
        let Some(state) = ctx
            .workflow
            .states
            .iter()
            .find(|state| state.name == ctx.to_state)
        else {
            return HookResult::Failed {
                reason: WorkflowEngine::undefined_state_message(&ctx.to_state, &ctx.workflow),
            };
        };
        let Some(role_name) = effective_role(state) else {
            return HookResult::Skipped {
                reason: "review state has no role".to_string(),
            };
        };
        let assignment = match get_role_assignment(ctx, role_name).await {
            Ok(assignment) => assignment,
            Err(reason) => return HookResult::Failed { reason },
        };
        if assignment
            .as_ref()
            .is_some_and(|assignment| assignment.assignee_id.is_some())
        {
            return HookResult::Skipped {
                reason: format!("{role_name} role assigned"),
            };
        }

        let ci_steps = match review_ci_steps(&ctx.state_config) {
            Ok(ci_steps) => ci_steps,
            Err(reason) => return HookResult::Failed { reason },
        };
        if !ci_steps.is_empty() {
            return HookResult::Skipped {
                reason: "review checks configured".to_string(),
            };
        }

        if gate_requires_user_approval(ctx) || human_review_requested(ctx, false) {
            return HookResult::Ok;
        }

        HookResult::Cascade {
            to: default_states::MERGING.to_string(),
            reason: "review skipped: no checks or reviewer assigned".to_string(),
        }
    }
}

fn gate_requires_user_approval(ctx: &HookContext) -> bool {
    ctx.gate_config
        .as_ref()
        .is_some_and(|gate_config| gate_config.requires_user_approval())
}

fn human_review_requested(ctx: &HookContext, reviewer_assigned: bool) -> bool {
    ctx.triggered_by.is_user() && ctx.to_state == default_states::REVIEW && !reviewer_assigned
}
