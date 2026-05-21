use api_types::ProjectHookRule;
use chrono::{DateTime, Utc};
use db::{
    new_uuid_v4, now_rfc3339, CreateProjectHookRun, Project, ProjectHookRun, ProjectHookRunRepo,
    ProjectHookRunStatus, UpdateProjectHookRun,
};
use events::{event_timestamp, EventContext, ForgeEvent, PROJECT_HOOK_RUN_CHANGED_EVENT};

use crate::{
    project_hooks::{
        actions::{execute_action, ActionContext, ActionOutcome},
        triggers::TriggerMatch,
        ProjectHookService,
    },
    Result,
};

pub struct ProjectHookEngine<'a> {
    service: &'a ProjectHookService,
}

struct RunStatusUpdate {
    status: ProjectHookRunStatus,
    automation_task_id: Option<Option<String>>,
    execution_id: Option<Option<String>>,
    agent_id: Option<Option<String>>,
    reason: Option<Option<String>>,
    terminal: bool,
}

impl<'a> ProjectHookEngine<'a> {
    pub fn new(service: &'a ProjectHookService) -> Self {
        Self { service }
    }

    pub async fn run(
        &self,
        project: &Project,
        rule: ProjectHookRule,
        trigger_match: TriggerMatch,
    ) -> Result<()> {
        let now = now_rfc3339();
        let max_concurrent_reason = format!("max_concurrent_runs reached for rule {}", rule.id);
        let Some(run) = ProjectHookRunRepo::try_claim_or_skip_at_limit(
            &*self.service.db,
            CreateProjectHookRun {
                id: new_uuid_v4(),
                project_id: project.id.clone(),
                rule_id: rule.id.clone(),
                trigger_type: trigger_match.trigger_type.clone(),
                dedupe_key: trigger_match.dedupe_key.clone(),
                status: ProjectHookRunStatus::Queued,
                source_task_id: trigger_match.source_task_id.clone(),
                source_execution_id: trigger_match.source_execution_id.clone(),
                automation_task_id: None,
                execution_id: None,
                agent_id: None,
                reason: trigger_match.reason.clone(),
                created_at: now.clone(),
                updated_at: now,
                completed_at: None,
            },
            i64::from(rule.max_concurrent_runs),
            &max_concurrent_reason,
        )
        .await?
        else {
            return Ok(());
        };
        self.publish_run_changed(&run);
        if run.status == ProjectHookRunStatus::Skipped {
            return Ok(());
        }

        let run = self
            .update_run_status(
                &run.id,
                RunStatusUpdate {
                    status: ProjectHookRunStatus::Running,
                    automation_task_id: None,
                    execution_id: None,
                    agent_id: None,
                    reason: Some(trigger_match.reason.clone()),
                    terminal: false,
                },
            )
            .await?;

        let cooldown_skip_reason = self.cooldown_skip_reason(&project.id, &rule).await?;
        if let Some(reason) = cooldown_skip_reason {
            self.update_run(
                &run.id,
                ProjectHookRunStatus::Skipped,
                ActionOutcome::skipped(reason),
                true,
            )
            .await?;
            return Ok(());
        }

        let context = ActionContext {
            service: self.service,
            project,
            rule_id: &rule.id,
            run: &run,
            trigger_match: &trigger_match,
        };
        match execute_action(&rule.action, &context).await {
            Ok(outcome) => {
                self.update_run(&run.id, outcome.status.clone(), outcome, true)
                    .await?;
            }
            Err(error) => {
                self.update_run(
                    &run.id,
                    ProjectHookRunStatus::Failed,
                    ActionOutcome {
                        status: ProjectHookRunStatus::Failed,
                        automation_task_id: None,
                        execution_id: None,
                        agent_id: None,
                        reason: Some(error.to_string()),
                    },
                    true,
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn cooldown_skip_reason(
        &self,
        project_id: &str,
        rule: &ProjectHookRule,
    ) -> Result<Option<String>> {
        let Some(cooldown_seconds) = rule.cooldown_seconds else {
            return Ok(None);
        };
        let runs =
            ProjectHookRunRepo::list_recent_for_project(&*self.service.db, project_id, 100).await?;
        let Some(run) = runs.into_iter().find(|run| {
            run.rule_id == rule.id
                && matches!(
                    run.status,
                    ProjectHookRunStatus::Completed
                        | ProjectHookRunStatus::Dispatched
                        | ProjectHookRunStatus::Skipped
                )
                && cooldown_active(cooldown_seconds, run)
        }) else {
            return Ok(None);
        };
        Ok(Some(format!(
            "rule {} is inside cooldown after run {}",
            rule.id, run.id
        )))
    }

    async fn update_run(
        &self,
        run_id: &str,
        status: ProjectHookRunStatus,
        outcome: ActionOutcome,
        terminal: bool,
    ) -> Result<ProjectHookRun> {
        self.update_run_status(
            run_id,
            RunStatusUpdate {
                status,
                automation_task_id: Some(outcome.automation_task_id),
                execution_id: Some(outcome.execution_id),
                agent_id: Some(outcome.agent_id),
                reason: Some(outcome.reason),
                terminal,
            },
        )
        .await
    }

    async fn update_run_status(
        &self,
        run_id: &str,
        update: RunStatusUpdate,
    ) -> Result<ProjectHookRun> {
        let now = now_rfc3339();
        let run = ProjectHookRunRepo::update_status(
            &*self.service.db,
            UpdateProjectHookRun {
                id: run_id.to_owned(),
                status: update.status,
                automation_task_id: update.automation_task_id,
                execution_id: update.execution_id,
                agent_id: update.agent_id,
                reason: update.reason,
                updated_at: now.clone(),
                completed_at: update.terminal.then_some(Some(now)),
            },
        )
        .await?;
        self.publish_run_changed(&run);
        Ok(run)
    }

    fn publish_run_changed(&self, run: &ProjectHookRun) {
        self.service.event_bus.publish(ForgeEvent {
            event_type: PROJECT_HOOK_RUN_CHANGED_EVENT.to_owned(),
            entity_id: run.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ProjectHookRunChanged {
                project_id: run.project_id.clone(),
                run_id: run.id.clone(),
                rule_id: run.rule_id.clone(),
                trigger_type: run.trigger_type.clone(),
                dedupe_key: run.dedupe_key.clone(),
                status: run.status.to_string(),
                source_task_id: run.source_task_id.clone(),
                automation_task_id: run.automation_task_id.clone(),
                execution_id: run.execution_id.clone(),
                agent_id: run.agent_id.clone(),
                reason: run.reason.clone(),
            },
        });
    }
}

fn cooldown_active(cooldown_seconds: u64, run: &ProjectHookRun) -> bool {
    let timestamp = run.completed_at.as_ref().unwrap_or(&run.updated_at);
    let Ok(timestamp) = DateTime::parse_from_rfc3339(timestamp) else {
        return false;
    };
    Utc::now()
        .signed_duration_since(timestamp.with_timezone(&Utc))
        .num_seconds()
        < cooldown_seconds as i64
}
