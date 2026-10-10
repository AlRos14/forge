use std::sync::Arc;

use db::{DomainEventRepo, SqliteDb, TaskRepo};
use events::{EventBus, EventContext, ForgeEvent};

use crate::{NotificationService, Result, TaskService};

pub mod actions;
mod engine;
pub mod evaluator;
pub mod triggers;

#[cfg(test)]
mod tests;

pub use evaluator::EvaluationCause;

#[derive(Clone)]
pub struct ProjectHookService {
    pub(crate) db: Arc<SqliteDb>,
    pub(crate) event_bus: Arc<EventBus>,
    pub(crate) task_service: Arc<TaskService>,
    pub(crate) notification_service: Arc<NotificationService>,
}

impl ProjectHookService {
    pub fn new(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        task_service: Arc<TaskService>,
        notification_service: Arc<NotificationService>,
    ) -> Self {
        Self {
            db,
            event_bus,
            task_service,
            notification_service,
        }
    }

    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut receiver = self.event_bus.subscribe();
            loop {
                let Ok(event) = receiver.recv().await else {
                    break;
                };
                let (project_id, cause) = match evaluation_cause_from_event(&self, &event).await {
                    Ok(Some(value)) => value,
                    Ok(None) => continue,
                    Err(error) => {
                        tracing::warn!(%error, "project hook event lookup failed");
                        continue;
                    }
                };
                let service = Arc::clone(&self);
                tokio::spawn(async move {
                    if let Err(error) = service.evaluate_for_project(project_id, cause).await {
                        tracing::warn!(%error, "project hook evaluation failed");
                    }
                });
            }
        })
    }

    pub async fn evaluate_for_project(
        &self,
        project_id: impl Into<String>,
        cause: EvaluationCause,
    ) -> Result<()> {
        evaluator::evaluate_for_project(self, project_id.into(), cause).await
    }
}

async fn evaluation_cause_from_event(
    service: &ProjectHookService,
    event: &ForgeEvent,
) -> Result<Option<(String, EvaluationCause)>> {
    match &event.context {
        EventContext::TaskCreated { project_id, .. } if event.event_type == "task.created" => {
            Ok(Some((
                project_id.clone(),
                EvaluationCause::TaskCreated {
                    task_id: event.entity_id.clone(),
                },
            )))
        }
        EventContext::TaskUpdated { project_id } if event.event_type == "task.archived" => {
            Ok(Some((
                project_id.clone(),
                EvaluationCause::TaskArchived {
                    task_id: event.entity_id.clone(),
                },
            )))
        }
        EventContext::DomainEventCommitted { .. }
            if event.event_type == "domain_event.committed" =>
        {
            let Some(domain_event) =
                DomainEventRepo::get_event(&*service.db, &event.entity_id).await?
            else {
                return Ok(None);
            };
            if domain_event.entity_type != "task"
                || domain_event.event_type != "task.lifecycle_changed"
            {
                return Ok(None);
            }
            let task_id = domain_event.entity_id;
            let Some(task) = TaskRepo::get_by_id(&*service.db, &task_id, false).await? else {
                return Ok(None);
            };
            Ok(Some((
                task.project_id,
                EvaluationCause::TaskTransitioned { task_id },
            )))
        }
        _ => Ok(None),
    }
}
