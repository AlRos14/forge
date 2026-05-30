use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, Weak},
};

use async_trait::async_trait;
use db::{
    AgentRepo, DaemonRepo, Execution, ExecutionRepo, ExecutionStatus, TaskRepo, UpdateExecution,
    WorkspaceRepo,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use executors::{LogKind, LogStream, LogWriter};
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    daemon_transport::DaemonExecutionEventHandler, task_service::logs::execution_logs_path, Result,
    ServiceError, TaskService,
};

const REMOTE_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

pub struct ServerExecutionEventSink {
    db: Arc<db::SqliteDb>,
    event_bus: Arc<EventBus>,
    workspace_root: PathBuf,
    task_service: Mutex<Option<Weak<TaskService>>>,
    writers: AsyncMutex<HashMap<String, Arc<AsyncMutex<LogWriter>>>>,
}

impl ServerExecutionEventSink {
    pub fn new(db: Arc<db::SqliteDb>, event_bus: Arc<EventBus>, workspace_root: PathBuf) -> Self {
        Self {
            db,
            event_bus,
            workspace_root,
            task_service: Mutex::new(None),
            writers: AsyncMutex::new(HashMap::new()),
        }
    }

    pub fn set_task_service(&self, task_service: Weak<TaskService>) {
        *lock(&self.task_service) = Some(task_service);
    }

    async fn writer_for(
        &self,
        notification: &api_types::ExecutionLogNotification,
        execution: &Execution,
    ) -> Result<Arc<AsyncMutex<LogWriter>>> {
        if let Some(writer) = self.writers.lock().await.get(&notification.execution_id) {
            return Ok(Arc::clone(writer));
        }

        let logs_path = match execution.logs_path.clone() {
            Some(path) => path,
            None => {
                let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
                let workspace_task_id =
                    if let Some(workspace_id) = execution.workspace_id.as_deref() {
                        WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                            .await?
                            .map(|workspace| workspace.task_id)
                            .unwrap_or_else(|| task.id.clone())
                    } else {
                        task.id.clone()
                    };
                let path = execution_logs_path(
                    &self.workspace_root,
                    &task.project_id,
                    &workspace_task_id,
                    &execution.id,
                );
                ExecutionRepo::update(
                    &*self.db,
                    UpdateExecution {
                        id: execution.id.clone(),
                        status: None,
                        stop_reason: None,
                        stopped_by: None,
                        resume_policy: None,
                        stopped_at: None,
                        agent_session_id: None,
                        agent_message_id: None,
                        last_activity_at: Some(Some(notification.ts.clone())),
                        summary: None,
                        logs_path: Some(Some(path.clone())),
                        before_sha: None,
                        after_sha: None,
                        error: None,
                        executor_config_snapshot_json: None,
                        updated_at: db::now_rfc3339(),
                    },
                )
                .await?;
                path
            }
        };

        let writer = Arc::new(AsyncMutex::new(LogWriter::new(
            logs_path,
            notification.execution_id.clone(),
            REMOTE_LOG_MAX_BYTES,
        )));
        self.writers
            .lock()
            .await
            .insert(notification.execution_id.clone(), Arc::clone(&writer));
        Ok(writer)
    }

    async fn authorized_execution(
        &self,
        daemon_id: &str,
        execution_id: &str,
    ) -> Result<Option<Execution>> {
        let Some(execution) = ExecutionRepo::get_by_id(&*self.db, execution_id).await? else {
            tracing::warn!(
                sending_daemon = %daemon_id,
                execution_id = %execution_id,
                "dropping execution notification for missing execution"
            );
            return Ok(None);
        };

        let Some(agent_id) = execution.agent_id.clone() else {
            tracing::warn!(
                sending_daemon = %daemon_id,
                execution_id = %execution_id,
                "rejecting execution notification: execution has no agent"
            );
            return Ok(None);
        };

        let Some(agent) = AgentRepo::get_by_id(&*self.db, &agent_id).await? else {
            tracing::warn!(
                sending_daemon = %daemon_id,
                execution_id = %execution_id,
                agent_id = %agent_id,
                "rejecting execution notification: execution agent was not found"
            );
            return Ok(None);
        };

        if agent.daemon_id.as_deref() == Some(daemon_id)
            || (agent.daemon_id.is_none() && self.is_embedded_daemon_sender(daemon_id).await?)
        {
            return Ok(Some(execution));
        }

        tracing::warn!(
            sending_daemon = %daemon_id,
            expected_daemon = ?agent.daemon_id,
            execution_id = %execution_id,
            "rejecting execution notification: daemon does not own this execution"
        );
        Ok(None)
    }

    async fn is_embedded_daemon_sender(&self, daemon_id: &str) -> Result<bool> {
        Ok(DaemonRepo::get_by_id(&*self.db, daemon_id)
            .await?
            .is_some_and(|daemon| {
                daemon.machine_id == crate::embedded_daemon::embedded_machine_id()
            }))
    }
}

#[async_trait]
impl DaemonExecutionEventHandler for ServerExecutionEventSink {
    async fn handle_log(
        &self,
        daemon_id: &str,
        notification: api_types::ExecutionLogNotification,
    ) -> Result<()> {
        let Some(execution) = self
            .authorized_execution(daemon_id, &notification.execution_id)
            .await?
        else {
            return Ok(());
        };

        let writer = self.writer_for(&notification, &execution).await?;
        let kind = notification
            .kind
            .as_deref()
            .and_then(|value| value.parse::<LogKind>().ok())
            .unwrap_or(match notification.stream.as_str() {
                "stderr" => LogKind::Stderr,
                _ => LogKind::Stdout,
            });
        let stream = match notification.log_stream.as_deref() {
            Some("heartbeat") => LogStream::Heartbeat,
            _ => LogStream::Main,
        };
        let payload = notification.payload.clone().unwrap_or_else(|| {
            json!({
                "line": notification.line,
                "daemon_seq": notification.seq,
                "daemon_ts": notification.ts,
                "stream": notification.stream,
            })
        });
        writer
            .lock()
            .await
            .write(kind.clone(), stream.clone(), payload.clone())
            .await
            .map_err(|error| {
                ServiceError::invalid_operation(format!("failed to write execution log: {error}"))
            })?;

        let execution_id = notification.execution_id.clone();
        ExecutionRepo::update_last_activity_at(&*self.db, &execution_id, &event_timestamp())
            .await?;
        let log = json!({
            "schema_version": 1,
            "sequence": notification.seq,
            "timestamp": notification.ts,
            "execution_id": execution_id.clone(),
            "kind": kind,
            "stream": stream,
            "payload": payload,
            "truncated": notification.truncated.unwrap_or(false),
        });
        self.event_bus.publish(ForgeEvent {
            event_type: "execution.log".to_owned(),
            entity_id: execution_id,
            timestamp: event_timestamp(),
            context: EventContext::ExecutionLog {
                task_id: execution.task_id,
                log,
                logs: None,
            },
        });
        Ok(())
    }

    async fn handle_terminal(
        &self,
        daemon_id: &str,
        notification: api_types::ExecutionTerminalNotification,
    ) -> Result<()> {
        if self
            .authorized_execution(daemon_id, &notification.execution_id)
            .await?
            .is_none()
        {
            return Ok(());
        }

        self.writers.lock().await.remove(&notification.execution_id);
        let Some(task_service) = lock(&self.task_service).as_ref().and_then(Weak::upgrade) else {
            return Err(ServiceError::invalid_operation(
                "task service is unavailable for daemon terminal notification",
            ));
        };
        let execution = task_service
            .complete_remote_execution(notification.clone())
            .await?;
        if execution.status != ExecutionStatus::Running {
            task_service
                .maybe_cascade_executor_completion(&notification.execution_id)
                .await?;
        }
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
