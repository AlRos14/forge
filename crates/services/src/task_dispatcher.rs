use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use events::EventBus;
use tokio::sync::Notify;
use tracing::Instrument;

use crate::{Result, TaskService};

mod active_recovery;

pub struct TaskDispatcher {
    db: Arc<db::SqliteDb>,
    event_bus: Arc<EventBus>,
    task_service: Arc<TaskService>,
    check_interval: Duration,
    stopped: AtomicBool,
    stop_notify: Notify,
}

impl TaskDispatcher {
    const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(10);

    pub fn new(
        db: Arc<db::SqliteDb>,
        event_bus: Arc<EventBus>,
        task_service: Arc<TaskService>,
    ) -> Self {
        Self::with_check_interval(db, event_bus, task_service, Self::DEFAULT_CHECK_INTERVAL)
    }

    pub fn with_check_interval(
        db: Arc<db::SqliteDb>,
        event_bus: Arc<EventBus>,
        task_service: Arc<TaskService>,
        check_interval: Duration,
    ) -> Self {
        Self {
            db,
            event_bus,
            task_service,
            check_interval,
            stopped: AtomicBool::new(false),
            stop_notify: Notify::new(),
        }
    }

    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let event_bus_strong_count = Arc::strong_count(&self.event_bus);
        tokio::spawn(
            async move {
                tracing::info!(
                    check_interval_seconds = self.check_interval.as_secs(),
                    "task dispatcher started"
                );
                while !self.is_stopped() {
                    if let Err(error) = self.check_once().await {
                        tracing::warn!(%error, "task dispatcher check failed");
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(self.check_interval) => {}
                        _ = self.stop_notify.notified() => {}
                    }
                }
                tracing::info!("task dispatcher stopped");
            }
            .instrument(tracing::info_span!(
                "task.dispatcher",
                event_bus_strong_count = event_bus_strong_count
            )),
        )
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.stop_notify.notify_one();
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    #[tracing::instrument(skip(self))]
    pub async fn check_once(&self) -> Result<u64> {
        let dispatched = self.recover_auto_cancelled_work_unit_executions().await?;

        tracing::info!(
            dispatched_tasks = dispatched,
            "task dispatcher check completed"
        );
        Ok(dispatched)
    }
}
