use api_types::{ProjectHookAction, TaskType};
use async_trait::async_trait;
use db::{Project, ProjectHookRun, ProjectHookRunStatus};

use crate::{project_hooks::triggers::TriggerMatch, Result};

use super::ProjectHookService;

pub mod add_comment;
pub mod create_task;
pub mod dispatch_agent;
pub mod notify;

#[derive(Debug, Clone)]
pub struct ActionOutcome {
    pub status: ProjectHookRunStatus,
    pub automation_task_id: Option<String>,
    pub execution_id: Option<String>,
    pub agent_id: Option<String>,
    pub reason: Option<String>,
}

impl ActionOutcome {
    pub fn completed(reason: impl Into<String>) -> Self {
        Self {
            status: ProjectHookRunStatus::Completed,
            automation_task_id: None,
            execution_id: None,
            agent_id: None,
            reason: Some(reason.into()),
        }
    }

    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            status: ProjectHookRunStatus::Skipped,
            automation_task_id: None,
            execution_id: None,
            agent_id: None,
            reason: Some(reason.into()),
        }
    }
}

pub struct ActionContext<'a> {
    pub service: &'a ProjectHookService,
    pub project: &'a Project,
    pub rule_id: &'a str,
    pub run: &'a ProjectHookRun,
    pub trigger_match: &'a TriggerMatch,
}

#[async_trait]
pub trait HookActionHandler {
    async fn execute(&self, context: &ActionContext<'_>) -> Result<ActionOutcome>;
}

pub async fn execute_action(
    action: &ProjectHookAction,
    context: &ActionContext<'_>,
) -> Result<ActionOutcome> {
    match action {
        ProjectHookAction::DispatchAgent {
            agent_id,
            prompt,
            follow_up,
        } => {
            dispatch_agent::DispatchAgentAction {
                agent_id,
                prompt: prompt.as_deref(),
                follow_up: follow_up.as_ref(),
            }
            .execute(context)
            .await
        }
        ProjectHookAction::CreateTask {
            title,
            description,
            task_type,
            priority,
        } => {
            create_task::CreateTaskAction {
                title,
                description: description.as_deref(),
                task_type: *task_type,
                priority: *priority,
            }
            .execute(context)
            .await
        }
        ProjectHookAction::AddComment {
            target_task_id,
            content,
        } => {
            add_comment::AddCommentAction {
                target_task_id: target_task_id.as_deref(),
                content,
            }
            .execute(context)
            .await
        }
        ProjectHookAction::Notify {
            title,
            message,
            severity,
        } => {
            notify::NotifyAction {
                title,
                message,
                severity: severity.as_deref(),
            }
            .execute(context)
            .await
        }
    }
}

pub(crate) fn task_type_to_string(task_type: TaskType) -> String {
    match task_type {
        TaskType::Task => "task",
        TaskType::PlanningTask => "planning_task",
        TaskType::SubTask => "sub_task",
        TaskType::Discovery => "discovery",
    }
    .to_owned()
}
