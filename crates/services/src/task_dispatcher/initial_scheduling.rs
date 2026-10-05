use db::{
    AgentRepo, DbError, Project, Task, TaskLifecycleRepo, TaskRoleAssignment,
    TaskRoleAssignmentRepo,
};

use crate::{
    agent_service::{compute_effective_status, EffectiveStatus},
    Assignee, Result, ServiceError,
};

use super::TaskDispatcher;

#[derive(Debug)]
pub(super) struct InitialScheduleTarget {
    pub(super) agent_id: String,
}

impl TaskDispatcher {
    pub(super) async fn dispatch_initial_tasks(&self, project: &Project) -> Result<u64> {
        let mut tasks = self
            .list_tasks(&project.id, vec!["todo".to_owned()])
            .await?;
        tasks.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut dispatched = 0;
        for task in tasks {
            if self.is_stopped() {
                break;
            }
            let Some(target) = self.resolve_initial_schedule_target(&task).await? else {
                continue;
            };
            match self.dispatch_initial_task(&task, &target).await {
                Ok(true) => dispatched += 1,
                Ok(false) => {}
                Err(ServiceError::Db(DbError::VersionConflict)) => {
                    tracing::debug!(task_id = %task.id, "task dispatcher initial claim lost version race");
                }
                Err(error) => {
                    tracing::warn!(task_id = %task.id, %error, "task dispatcher initial dispatch failed");
                }
            }
        }
        Ok(dispatched)
    }

    pub(super) async fn dispatch_initial_task(
        &self,
        task: &Task,
        target: &InitialScheduleTarget,
    ) -> Result<bool> {
        if self.is_stopped() || task.repo_id.is_none() {
            return Ok(false);
        }
        let agent = AgentRepo::get_by_id(&*self.db, &target.agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", target.agent_id.clone()))?;
        if compute_effective_status(&self.db, &agent).await? != EffectiveStatus::Active {
            return Ok(false);
        }
        if TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .is_none_or(|lifecycle| lifecycle.state != db::TaskLifecycleState::Ready)
        {
            return Ok(false);
        }
        if self.is_stopped() {
            return Ok(false);
        }
        self.task_service
            .claim_task(
                task.id.clone(),
                Assignee::Agent(target.agent_id.clone()),
                None,
            )
            .await?;
        Ok(true)
    }

    pub(super) async fn resolve_initial_schedule_target(
        &self,
        task: &Task,
    ) -> Result<Option<InitialScheduleTarget>> {
        let role = crate::task_service::execution::task_role_for_task_type(&task.task_type);
        let memberships =
            crate::task_service::current_role_memberships_authoritative(&self.db, &task.id, role)
                .await?;
        let agent_id = if let Some(memberships) = memberships {
            if task.repo_id.is_some() {
                crate::task_service::select_usable_repository_agent_id(
                    &self.db,
                    &task.project_id,
                    &memberships,
                )
                .await?
            } else {
                crate::task_service::select_usable_agent_id(&self.db, &memberships).await?
            }
        } else {
            self.legacy_assigned_agent_for_initial_schedule(task, role)
                .await?
        };
        Ok(agent_id.map(|agent_id| InitialScheduleTarget { agent_id }))
    }

    async fn legacy_assigned_agent_for_initial_schedule(
        &self,
        task: &Task,
        role: &str,
    ) -> Result<Option<String>> {
        let aliases: &[&str] = if role == "implementer" {
            &["implementer", "coder", "worker", "assignee", "executor"]
        } else {
            &[role]
        };
        for alias in aliases {
            let assignment =
                TaskRoleAssignmentRepo::get_by_task_and_role(&*self.db, &task.id, alias).await?;
            if let Some(agent_id) = assignment.and_then(legacy_agent_assignment) {
                return Ok(Some(agent_id));
            }
        }
        if role == "implementer"
            && task.assignee_type.as_deref() == Some("agent")
            && task.assignee_id.is_some()
        {
            return Ok(task.assignee_id.clone());
        }
        Ok(None)
    }
}

fn legacy_agent_assignment(assignment: TaskRoleAssignment) -> Option<String> {
    (assignment.assignee_type == Some(db::AssigneeKind::Agent))
        .then_some(assignment.assignee_id)
        .flatten()
}
