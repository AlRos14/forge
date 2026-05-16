use std::sync::Arc;

use db::{
    new_uuid_v4, now_rfc3339, CreateNotification, NotificationRepo, ReviewRepo, SqliteDb, TaskRepo,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};

pub struct NotificationService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
}

impl NotificationService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self { db, event_bus }
    }

    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut rx = self.event_bus.subscribe();
            loop {
                let Ok(event) = rx.recv().await else {
                    break;
                };
                if let Err(error) = self.handle_event(event).await {
                    tracing::warn!(%error, "notification service failed to handle event");
                }
            }
        })
    }

    async fn handle_event(&self, event: ForgeEvent) -> crate::Result<()> {
        match event.context {
            EventContext::TaskStatusChanged {
                project_id,
                new_status,
                ..
            } if new_status == crate::workflow::default_states::DONE => {
                let Some(task) = TaskRepo::get_by_id(&*self.db, &event.entity_id, true).await?
                else {
                    return Ok(());
                };
                self.create_and_publish(
                    project_id,
                    Some(task.id),
                    "task.done".to_owned(),
                    task.title,
                    None,
                )
                .await?;
            }
            EventContext::TaskBlocked {
                project_id, reason, ..
            } => {
                let Some(task) = TaskRepo::get_by_id(&*self.db, &event.entity_id, true).await?
                else {
                    return Ok(());
                };
                self.create_and_publish(
                    project_id,
                    Some(task.id),
                    "task.blocked".to_owned(),
                    task.title,
                    Some(reason),
                )
                .await?;
            }
            EventContext::ReviewPassed { task_id, .. } => {
                let Some(task) = TaskRepo::get_by_id(&*self.db, &task_id, true).await? else {
                    return Ok(());
                };
                self.create_and_publish(
                    task.project_id,
                    Some(task.id),
                    "review.passed".to_owned(),
                    format!("Review passed: {}", task.title),
                    None,
                )
                .await?;
            }
            EventContext::ReviewFailed {
                task_id, review_id, ..
            } => {
                let Some(task) = TaskRepo::get_by_id(&*self.db, &task_id, true).await? else {
                    return Ok(());
                };
                let reason = ReviewRepo::get_by_id(&*self.db, &review_id)
                    .await?
                    .and_then(|review| extract_review_failure_reason(&review.step_results_json));
                self.create_and_publish(
                    task.project_id,
                    Some(task.id),
                    "review.failed".to_owned(),
                    format!("Review failed: {}", task.title),
                    reason,
                )
                .await?;
            }
            EventContext::MergeFailed { task_id, reason } => {
                let Some(task) = TaskRepo::get_by_id(&*self.db, &task_id, true).await? else {
                    return Ok(());
                };
                self.create_and_publish(
                    task.project_id,
                    Some(task.id),
                    "merge.failed".to_owned(),
                    task.title,
                    Some(reason),
                )
                .await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn create_and_publish(
        &self,
        project_id: String,
        task_id: Option<String>,
        event_type: String,
        title: String,
        body: Option<String>,
    ) -> crate::Result<()> {
        let notification = NotificationRepo::create(
            &*self.db,
            CreateNotification {
                id: new_uuid_v4(),
                project_id,
                task_id,
                event_type: event_type.clone(),
                title: title.clone(),
                body,
                read: false,
                created_at: now_rfc3339(),
            },
        )
        .await?;

        self.event_bus.publish(ForgeEvent {
            event_type: "notification.created".to_owned(),
            entity_id: notification.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::NotificationCreated {
                notification_id: notification.id,
                project_id: notification.project_id,
                task_id: notification.task_id,
                event_type,
                title,
            },
        });
        Ok(())
    }
}

fn extract_review_failure_reason(step_results_json: &str) -> Option<String> {
    let details = serde_json::from_str::<serde_json::Value>(step_results_json).ok()?;
    details
        .get("auditor")
        .and_then(|auditor| auditor.get("reason"))
        .and_then(|reason| reason.as_str())
        .map(|reason| reason.to_owned())
}
