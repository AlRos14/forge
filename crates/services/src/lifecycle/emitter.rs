use std::{path::Path, path::PathBuf, sync::Arc};

use api_types::{LifecycleHookDef, LifecycleHooks, ProjectSettings, StateKind, WorkflowDefinition};
use db::{
    Execution, ExecutionRepo, PageRequest, ProjectRepo, RepoRepo, SortBy, SortOrder, Task,
    TaskRepo, TransitionLogRepo, WorkspaceRepo,
};
use events::{EventContext, ForgeEvent};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::{
    lifecycle::{LifecycleHookContext, LifecycleHookRunner, PluginRegistry},
    workflow::engine::WorkflowEngine,
};

#[derive(Clone)]
pub struct LifecycleEventEmitter {
    db: Arc<db::SqliteDb>,
    plugin_registry: Arc<PluginRegistry>,
}

impl LifecycleEventEmitter {
    pub fn new(db: Arc<db::SqliteDb>, plugin_registry: Arc<PluginRegistry>) -> Self {
        Self {
            db,
            plugin_registry,
        }
    }

    pub async fn run(&self, mut rx: broadcast::Receiver<ForgeEvent>) {
        info!("lifecycle event emitter started");

        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(error) = self.handle_event(event).await {
                        warn!(%error, "lifecycle event emitter failed");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "lifecycle event emitter lagged");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!("lifecycle event emitter stopped");
                    break;
                }
            }
        }
    }

    async fn handle_event(&self, event: ForgeEvent) -> Result<(), String> {
        let ForgeEvent {
            event_type,
            entity_id,
            context,
            ..
        } = event;

        match context {
            EventContext::TaskStatusChanged {
                project_id,
                old_status,
                new_status,
            } if event_type == "task.status_changed" => {
                self.handle_status_changed(&entity_id, &project_id, &old_status, &new_status)
                    .await
            }
            EventContext::TaskMoved(payload)
                if event_type == events::TASK_MOVED_EVENT
                    && payload.old_status != payload.new_status =>
            {
                self.handle_status_changed(
                    &entity_id,
                    &payload.project_id,
                    &payload.old_status,
                    &payload.new_status,
                )
                .await
            }
            EventContext::TaskAssigned {
                project_id,
                agent_id,
                execution_id,
            } if event_type == "task.execution_launched" => {
                self.emit_lifecycle_event(
                    &entity_id,
                    Some(&project_id),
                    api_types::LifecycleEvent::OnWorkStart,
                    None,
                    Some(agent_id),
                    Some(execution_id),
                )
                .await
            }
            EventContext::ExecutionStarted { task_id, agent_id } => {
                self.emit_lifecycle_event(
                    &task_id,
                    None,
                    api_types::LifecycleEvent::OnWorkStart,
                    None,
                    agent_id,
                    Some(entity_id),
                )
                .await
            }
            _ => Ok(()),
        }
    }

    async fn handle_status_changed(
        &self,
        task_id: &str,
        project_id: &str,
        old_status: &str,
        new_status: &str,
    ) -> Result<(), String> {
        let task = self.load_task(task_id).await?;
        let project = self.load_project(project_id).await?;
        let workflow = resolve_workflow(&project.workflow_definition);
        let old_kind = workflow.state_kind(old_status);
        let new_kind = workflow.state_kind(new_status);
        let cancellation_state = workflow.cancellation_state.as_deref();

        if matches!(new_kind, Some(StateKind::Active))
            && !matches!(old_kind, Some(StateKind::Active))
        {
            self.emit_lifecycle_event(
                &task.id,
                Some(project_id),
                api_types::LifecycleEvent::BeforeWork,
                Some(old_status.to_owned()),
                task.assignee_id.clone(),
                None,
            )
            .await?;
        }

        if matches!(new_kind, Some(StateKind::Terminal))
            && cancellation_state.is_some_and(|state| state == new_status)
        {
            self.emit_lifecycle_event(
                &task.id,
                Some(project_id),
                api_types::LifecycleEvent::OnTaskCancel,
                Some(old_status.to_owned()),
                task.assignee_id.clone(),
                None,
            )
            .await?;
            return Ok(());
        }

        if matches!(new_kind, Some(StateKind::Terminal)) {
            self.emit_lifecycle_event(
                &task.id,
                Some(project_id),
                api_types::LifecycleEvent::OnTaskDone,
                Some(old_status.to_owned()),
                task.assignee_id.clone(),
                None,
            )
            .await?;
        }

        if matches!(old_kind, Some(StateKind::Active))
            && !matches!(new_kind, Some(StateKind::Active | StateKind::Terminal))
        {
            self.emit_lifecycle_event(
                &task.id,
                Some(project_id),
                api_types::LifecycleEvent::OnWorkStop,
                Some(old_status.to_owned()),
                task.assignee_id.clone(),
                None,
            )
            .await?;
        }

        Ok(())
    }

    async fn emit_lifecycle_event(
        &self,
        task_id: &str,
        project_id_hint: Option<&str>,
        event: api_types::LifecycleEvent,
        previous_status: Option<String>,
        agent_id: Option<String>,
        execution_id: Option<String>,
    ) -> Result<(), String> {
        let task = self.load_task(task_id).await?;
        let project = self
            .load_project(project_id_hint.unwrap_or(&task.project_id))
            .await?;
        let settings = parse_project_settings(&project.settings)?;
        let hooks = lifecycle_hooks_for(&settings.lifecycle_hooks, event);

        if hooks.is_empty() {
            return Ok(());
        }

        let execution = self
            .resolve_execution(task_id, execution_id.as_deref())
            .await
            .map_err(|error| format!("failed to resolve execution for task {task_id}: {error}"))?;
        let resolved_execution_id =
            execution_id.or_else(|| execution.as_ref().map(|item| item.id.clone()));
        let resolved_agent_id = agent_id
            .or_else(|| execution.as_ref().and_then(|item| item.agent_id.clone()))
            .or_else(|| task.assignee_id.clone());
        let previous_status = match previous_status {
            Some(previous_status) => previous_status,
            None => self
                .latest_previous_status(task_id)
                .await
                .unwrap_or_else(|| task.status.clone()),
        };
        let worktree_path = self.resolve_worktree_path(&task, execution.as_ref()).await;
        let repo_path = self
            .resolve_repo_path(&task, worktree_path.as_deref())
            .await;
        let log_dir = resolve_log_dir(execution.as_ref(), resolved_execution_id.as_deref());

        let ctx = LifecycleHookContext {
            event,
            task_id: task.id.clone(),
            task_title: task.title.clone(),
            task_status: task.status.clone(),
            previous_status,
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            repo_path,
            worktree_path,
            agent_id: resolved_agent_id,
            execution_id: resolved_execution_id,
            log_dir,
        };

        LifecycleHookRunner::run_hooks(ctx, &hooks, Arc::clone(&self.plugin_registry)).await;
        Ok(())
    }

    async fn load_task(&self, task_id: &str) -> Result<Task, String> {
        TaskRepo::get_by_id(&*self.db, task_id, false)
            .await
            .map_err(|error| format!("failed to load task {task_id}: {error}"))?
            .ok_or_else(|| format!("task {task_id} not found"))
    }

    async fn load_project(&self, project_id: &str) -> Result<db::Project, String> {
        ProjectRepo::get_by_id(&*self.db, project_id)
            .await
            .map_err(|error| format!("failed to load project {project_id}: {error}"))?
            .ok_or_else(|| format!("project {project_id} not found"))
    }

    async fn latest_previous_status(&self, task_id: &str) -> Option<String> {
        let transition_logs = TransitionLogRepo::list_by_task(&*self.db, task_id)
            .await
            .ok()?;
        transition_logs.last().map(|entry| entry.from_state.clone())
    }

    async fn resolve_execution(
        &self,
        task_id: &str,
        execution_id: Option<&str>,
    ) -> Result<Option<Execution>, db::DbError> {
        if let Some(execution_id) = execution_id {
            if let Some(execution) = ExecutionRepo::get_by_id(&*self.db, execution_id).await? {
                return Ok(Some(execution));
            }
        }

        let page = ExecutionRepo::list_by_task(
            &*self.db,
            task_id,
            PageRequest {
                cursor: None,
                limit: 500,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await?;

        let executions = page.items;
        let fallback = executions.first().cloned();

        Ok(executions
            .into_iter()
            .find(|execution| execution.role == "executor")
            .or(fallback))
    }

    async fn resolve_worktree_path(
        &self,
        task: &Task,
        execution: Option<&Execution>,
    ) -> Option<String> {
        if let Ok(Some(workspace)) = WorkspaceRepo::get_by_task_id(&*self.db, &task.id).await {
            if Path::new(&workspace.worktree_path).exists() {
                return Some(workspace.worktree_path);
            }
        }

        let workspace_id = execution.and_then(|execution| execution.workspace_id.as_deref())?;
        let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
            .await
            .ok()??;

        if Path::new(&workspace.worktree_path).exists() {
            Some(workspace.worktree_path)
        } else {
            None
        }
    }

    async fn resolve_repo_path(&self, task: &Task, worktree_path: Option<&str>) -> String {
        let repo_path = match task.repo_id.as_deref() {
            Some(id) => RepoRepo::get_by_id(&*self.db, id)
                .await
                .ok()
                .flatten()
                .and_then(|repo| repo.local_path),
            None => None,
        };

        match repo_path {
            Some(repo_path) => repo_path,
            None => worktree_path.unwrap_or_default().to_owned(),
        }
    }
}

fn lifecycle_hooks_for(
    hooks: &LifecycleHooks,
    event: api_types::LifecycleEvent,
) -> Vec<LifecycleHookDef> {
    let hooks = hooks.get(&event).cloned().unwrap_or_default();
    if event != api_types::LifecycleEvent::BeforeWork {
        return hooks;
    }

    hooks
        .into_iter()
        .filter(|hook| !matches!(hook, LifecycleHookDef::Script { blocking: true, .. }))
        .collect()
}

fn parse_project_settings(settings: &str) -> Result<ProjectSettings, String> {
    serde_json::from_str::<ProjectSettings>(settings)
        .map_err(|error| format!("invalid project settings: {error}"))
}

fn resolve_workflow(workflow_definition: &str) -> WorkflowDefinition {
    WorkflowEngine::resolve_workflow(workflow_definition)
}

fn resolve_log_dir(execution: Option<&Execution>, execution_id: Option<&str>) -> Option<PathBuf> {
    if let Some(logs_path) = execution.and_then(|execution| execution.logs_path.as_deref()) {
        return Path::new(logs_path).parent().map(Path::to_path_buf);
    }

    execution_id
        .map(|execution_id| {
            std::env::temp_dir()
                .join("forge")
                .join("logs")
                .join(execution_id)
                .with_extension("jsonl")
        })
        .and_then(|path| path.parent().map(Path::to_path_buf))
}
