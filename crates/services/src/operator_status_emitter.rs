use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use events::{
    event_timestamp, EventBus, EventContext, ForgeEvent, OPERATIONS_STATUS_CHANGED_EVENT,
};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

pub struct OperatorStatusEmitter {
    _event_bus: Arc<EventBus>,
    handle: JoinHandle<()>,
}

impl OperatorStatusEmitter {
    pub fn start(event_bus: Arc<EventBus>) -> Self {
        let task_event_bus = Arc::clone(&event_bus);
        let handle = tokio::spawn(async move {
            let mut rx = task_event_bus.subscribe();
            let dirty = AtomicBool::new(false);
            let mut last_event_type: Option<String> = None;
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            interval.tick().await;

            loop {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Ok(event) => {
                                let event_type = event.event_type;
                                if is_status_affecting_event(&event_type) {
                                    last_event_type = Some(event_type);
                                    dirty.store(true, Ordering::Release);
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = interval.tick() => {
                        if dirty.swap(false, Ordering::AcqRel) {
                            let trigger = last_event_type
                                .clone()
                                .unwrap_or_else(|| "unknown".to_string());
                            task_event_bus.publish(ForgeEvent {
                                event_type: OPERATIONS_STATUS_CHANGED_EVENT.to_string(),
                                entity_id: "operations".to_string(),
                                timestamp: event_timestamp(),
                                context: EventContext::OperationsStatusChanged { trigger },
                            });
                        }
                    }
                }
            }
        });

        Self {
            _event_bus: event_bus,
            handle,
        }
    }

    pub fn stop(&self) {
        self.handle.abort();
    }
}

fn is_status_affecting_event(event_type: &str) -> bool {
    if event_type == OPERATIONS_STATUS_CHANGED_EVENT {
        return false;
    }

    [
        "task.",
        "execution.",
        "daemon.",
        "workspace.",
        "review.",
        "merge.",
        "cleanup.",
    ]
    .iter()
    .any(|prefix| event_type.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_affecting_event_matches_expected_types() {
        assert!(is_status_affecting_event("task.status_changed"));
        assert!(is_status_affecting_event("task.moved"));
        assert!(is_status_affecting_event("execution.started"));
        assert!(is_status_affecting_event("daemon.registered"));
        assert!(is_status_affecting_event("workspace.created"));
        assert!(is_status_affecting_event("review.decided"));
        assert!(is_status_affecting_event("merge.started"));
        assert!(!is_status_affecting_event(OPERATIONS_STATUS_CHANGED_EVENT));
        assert!(!is_status_affecting_event("unknown.event"));
    }

    #[tokio::test]
    async fn coalesces_status_changed_events() {
        tokio::time::pause();

        let event_bus = Arc::new(EventBus::new(64));
        let emitter = OperatorStatusEmitter::start(Arc::clone(&event_bus));
        let mut rx = event_bus.subscribe();

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        for index in 0..10 {
            event_bus.publish(ForgeEvent {
                event_type: "task.status_changed".to_string(),
                entity_id: format!("task-{index}"),
                timestamp: event_timestamp(),
                context: EventContext::TaskStatusChanged {
                    project_id: "project-1".to_string(),
                    old_status: "todo".to_string(),
                    new_status: "in_progress".to_string(),
                },
            });
        }

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(600)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        let mut operations_status_changed_count = 0;
        while let Ok(event) = rx.try_recv() {
            if event.event_type == OPERATIONS_STATUS_CHANGED_EVENT {
                operations_status_changed_count += 1;
                assert!(matches!(
                    event.context,
                    EventContext::OperationsStatusChanged { .. }
                ));
            }
        }

        emitter.stop();

        assert_eq!(operations_status_changed_count, 1);
    }
}
