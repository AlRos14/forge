use std::sync::Arc;

use db::SqliteDb;
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
                let Some((project_id, cause)) = evaluation_cause_from_event(&event) else {
                    continue;
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

fn evaluation_cause_from_event(event: &ForgeEvent) -> Option<(String, EvaluationCause)> {
    match &event.context {
        EventContext::TaskCreated { project_id, .. } => Some((
            project_id.clone(),
            EvaluationCause::TaskCreated {
                task_id: event.entity_id.clone(),
            },
        )),
        EventContext::TaskStatusChanged { project_id, .. } => Some((
            project_id.clone(),
            EvaluationCause::TaskTransitioned {
                task_id: event.entity_id.clone(),
            },
        )),
        EventContext::TaskMoved(payload) if payload.old_status != payload.new_status => Some((
            payload.project_id.clone(),
            EvaluationCause::TaskTransitioned {
                task_id: event.entity_id.clone(),
            },
        )),
        EventContext::TaskUpdated { project_id } if event.event_type == "task.archived" => Some((
            project_id.clone(),
            EvaluationCause::TaskArchived {
                task_id: event.entity_id.clone(),
            },
        )),
        _ => None,
    }
}
