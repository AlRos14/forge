use crate::{
    daemon_transport::DaemonConnectionRegistry, embedded_daemon::is_embedded_daemon_machine,
    Result, ServiceError, TaskService,
};
use chrono::{Duration as ChronoDuration, Utc};
use db::{
    now_rfc3339, Agent, AgentListQuery, AgentRepo, AgentStatus, Daemon, DaemonRepo, Execution,
    ExecutionRepo, ExecutionStatus, PageRequest, Project, ProjectRepo, ResumePolicy, SortBy,
    SortOrder, SqliteDb, StopReason, Task, TaskLifecycleRepo, TaskLifecycleState, TaskListQuery,
    TaskRepo, UpdateAgent, UpdateExecution, UpdateTask, WorkspaceLeaseRepo,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use executors::TaskExecutor;
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tracing::Instrument;

#[derive(Clone)]
pub struct CrashRecovery {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
}

impl CrashRecovery {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self { db, event_bus }
    }

    #[tracing::instrument(skip(self))]
    pub async fn run(&self) -> Result<u64> {
        self.run_recovery().await
    }

    #[tracing::instrument(skip(self))]
    pub async fn run_recovery(&self) -> Result<u64> {
        // Expire stale grants before recovering active Tasks. The recovery
        // pass below then sees the still-running attempt and requeues/blocks
        // it through the normal crash-recovery state machine.
        expire_workspace_leases(&self.db, &self.event_bus, None, None, None, false).await?;
        // An admitted WorkUnit Execution whose provider Start has not been
        // claimed is still queued work, not an interrupted provider attempt.
        // Leave it Running for TaskDispatcher, which retries the exact
        // Execution after an Agent is unpaused.
        let pending_provider_starts: HashSet<String> =
            ExecutionRepo::list_pending_provider_starts(self.db.as_ref())
                .await?
                .into_iter()
                .map(|execution| execution.id)
                .collect();
        let executions: Vec<Execution> = ExecutionRepo::list_running(self.db.as_ref())
            .await?
            .into_iter()
            .filter(|execution| !pending_provider_starts.contains(&execution.id))
            .collect();
        let executions_by_task = group_executions_by_task(executions);
        let mut recovered = 0;

        for (task_id, executions) in executions_by_task {
            let Some(task) = TaskRepo::get_by_id(self.db.as_ref(), &task_id, false).await? else {
                continue;
            };
            let outcome = recover_executions(
                &self.db,
                &self.event_bus,
                task,
                executions,
                StopReason::CrashRecovery,
                &api_types::Actor::system(api_types::SystemComponent::CrashRecovery),
            )
            .await?;

            if outcome.annotated {
                self.publish(ForgeEvent {
                    event_type: "task.recovered".to_owned(),
                    entity_id: outcome.task.id,
                    timestamp: event_timestamp(),
                    context: EventContext::TaskRecovered {
                        project_id: outcome.task.project_id,
                        reason: "crash_recovery".to_owned(),
                    },
                });
                recovered += 1;
            }
        }

        recovered += sweep_stale_recovery_annotations(&self.db).await?;

        tracing::info!(recovered_tasks = recovered, "crash recovery completed");
        Ok(recovered)
    }

    fn publish(&self, event: ForgeEvent) {
        self.event_bus.publish(event);
    }
}

pub struct HeartbeatMonitor {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    task_service: Option<Arc<TaskService>>,
    task_executor: Option<Arc<dyn TaskExecutor>>,
    daemon_connections: Option<Arc<DaemonConnectionRegistry>>,
    check_interval: Duration,
    execution_stall_timeout: Duration,
    daemon_disconnect_grace: Duration,
    disconnect_observed: Mutex<HashMap<String, Instant>>,
    stopped: AtomicBool,
    stop_notify: tokio::sync::Notify,
}

impl HeartbeatMonitor {
    const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(10);
    const DEFAULT_EXECUTION_STALL_TIMEOUT: Duration = Duration::from_secs(300);
    const DEFAULT_DAEMON_DISCONNECT_GRACE: Duration = Duration::from_secs(120);

    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self::with_check_interval(db, event_bus, Self::DEFAULT_CHECK_INTERVAL)
    }

    pub fn with_check_interval(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        check_interval: Duration,
    ) -> Self {
        Self {
            db,
            event_bus,
            task_service: None,
            task_executor: None,
            daemon_connections: None,
            check_interval,
            execution_stall_timeout: Self::DEFAULT_EXECUTION_STALL_TIMEOUT,
            daemon_disconnect_grace: Self::DEFAULT_DAEMON_DISCONNECT_GRACE,
            disconnect_observed: Mutex::new(HashMap::new()),
            stopped: AtomicBool::new(false),
            stop_notify: tokio::sync::Notify::new(),
        }
    }

    pub fn with_task_service(mut self, task_service: Arc<TaskService>) -> Self {
        self.task_service = Some(task_service);
        self
    }

    pub fn with_task_executor(mut self, task_executor: Arc<dyn TaskExecutor>) -> Self {
        self.task_executor = Some(task_executor);
        self
    }

    pub fn with_daemon_connections(
        mut self,
        daemon_connections: Arc<DaemonConnectionRegistry>,
    ) -> Self {
        self.daemon_connections = Some(daemon_connections);
        self
    }

    pub fn with_execution_stall_timeout(mut self, execution_stall_timeout: Duration) -> Self {
        self.execution_stall_timeout = execution_stall_timeout;
        self
    }

    pub fn with_daemon_disconnect_grace(mut self, daemon_disconnect_grace: Duration) -> Self {
        self.daemon_disconnect_grace = daemon_disconnect_grace;
        self
    }

    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(
            async move {
                tracing::info!(
                    check_interval_seconds = self.check_interval.as_secs(),
                    "heartbeat monitor started"
                );
                while !self.is_stopped() {
                    if let Err(error) = self.check_timeouts().await {
                        tracing::warn!(%error, "heartbeat monitor check failed");
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(self.check_interval) => {}
                        _ = self.stop_notify.notified() => {}
                    }
                }
                tracing::info!("heartbeat monitor stopped");
            }
            .instrument(tracing::info_span!("heartbeat.monitor")),
        )
    }

    #[tracing::instrument(skip(self))]
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.stop_notify.notify_one();
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    #[tracing::instrument(skip(self))]
    pub async fn check_once(&self) -> Result<u64> {
        let agents = self.list_busy_agents().await?;
        let pending_provider_starts: HashSet<String> =
            ExecutionRepo::list_pending_provider_starts(&*self.db)
                .await?
                .into_iter()
                .map(|execution| execution.id)
                .collect();
        let running_executions = ExecutionRepo::list_running(&*self.db).await?;
        let mut timed_out = 0;

        for agent in agents {
            if !agent_timed_out(&agent) {
                continue;
            }

            let mut has_pending_provider_start = false;
            let mut timed_out_executions = Vec::new();
            for execution in running_executions
                .iter()
                .filter(|execution| execution.agent_id.as_deref() == Some(agent.id.as_str()))
            {
                if pending_provider_starts.contains(&execution.id) {
                    has_pending_provider_start = true;
                } else {
                    timed_out_executions.push(execution.clone());
                }
            }
            if has_pending_provider_start && timed_out_executions.is_empty() {
                tracing::debug!(
                    agent_id = %agent.id,
                    "skipping heartbeat timeout while the Agent only has queued provider Starts"
                );
                continue;
            }

            let last_heartbeat = agent
                .last_heartbeat_at
                .clone()
                .unwrap_or_else(|| "never".to_owned());

            AgentRepo::update(
                &*self.db,
                UpdateAgent {
                    id: agent.id.clone(),
                    expected_version: agent.version,
                    name: None,
                    description: None,
                    max_concurrent_tasks: None,
                    heartbeat_interval_seconds: None,
                    max_missed_heartbeats: None,
                    status: Some(AgentStatus::Error),
                    last_heartbeat_at: None,
                    model: None,
                    reasoning_effort: None,
                    permission_policy: None,
                    capabilities_json: None,
                    config_json: None,
                    daemon_id: None,
                    is_default: None,
                    paused: None,
                    prompt_template: None,
                    updated_at: now_rfc3339(),
                },
            )
            .await?;
            self.publish(ForgeEvent {
                event_type: "agent.timeout".to_owned(),
                entity_id: agent.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::AgentTimeout {
                    last_heartbeat: last_heartbeat.clone(),
                },
            });

            for (task_id, executions) in group_executions_by_task(timed_out_executions) {
                let Some(task) = TaskRepo::get_by_id(self.db.as_ref(), &task_id, false).await?
                else {
                    continue;
                };
                let outcome = recover_executions(
                    &self.db,
                    &self.event_bus,
                    task,
                    executions,
                    StopReason::AgentTimeout,
                    &api_types::Actor::system(api_types::SystemComponent::HeartbeatMonitor),
                )
                .await?;

                if outcome.annotated {
                    self.publish(ForgeEvent {
                        event_type: "task.recovered".to_owned(),
                        entity_id: outcome.task.id.clone(),
                        timestamp: event_timestamp(),
                        context: EventContext::TaskRecovered {
                            project_id: outcome.task.project_id,
                            reason: "agent_timeout".to_owned(),
                        },
                    });
                }
            }

            timed_out += 1;
        }

        if timed_out > 0 {
            tracing::info!(
                timed_out_agents = timed_out,
                "heartbeat monitor detected timed out agents"
            );
        }
        renew_workspace_leases(&self.db).await?;
        let expired = expire_workspace_leases(
            &self.db,
            &self.event_bus,
            self.daemon_connections.as_deref(),
            self.task_executor.as_deref(),
            self.task_service.as_deref(),
            true,
        )
        .await?;
        let stalled = self.check_stalled_executions().await?;
        let disconnected = self.check_disconnected_daemon_executions().await?;
        Ok(timed_out + stalled + disconnected + expired)
    }

    #[tracing::instrument(skip(self))]
    async fn check_timeouts(&self) -> Result<()> {
        self.check_once().await.map(|_| ())
    }

    #[tracing::instrument(skip(self))]
    async fn list_busy_agents(&self) -> Result<Vec<Agent>> {
        let mut agents = Vec::new();
        let mut cursor = None;
        loop {
            let page = AgentRepo::list(
                &*self.db,
                AgentListQuery {
                    status: Some(AgentStatus::Busy),
                    executor_type: None,
                    capabilities: Vec::new(),
                    harness_only: true,
                    page: page_request(cursor),
                },
            )
            .await?;
            agents.extend(page.items);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(agents)
    }

    async fn check_stalled_executions(&self) -> Result<u64> {
        let stale_before = Utc::now()
            - ChronoDuration::from_std(self.execution_stall_timeout).unwrap_or_else(|_| {
                ChronoDuration::seconds(Self::DEFAULT_EXECUTION_STALL_TIMEOUT.as_secs() as i64)
            });
        let stale_before = stale_before.to_rfc3339();
        let executions = ExecutionRepo::list_stalled_running(&*self.db, &stale_before).await?;
        let mut stalled = 0;

        for execution in executions {
            if self.task_service.is_some()
                || self.daemon_connections.is_some()
                || self.task_executor.is_some()
            {
                if let Err(error) = cancel_execution_on_frozen_host(
                    &self.db,
                    &execution,
                    "execution stalled",
                    self.daemon_connections.as_deref(),
                    self.task_service.as_deref(),
                    self.task_executor.as_deref(),
                )
                .await
                {
                    tracing::warn!(
                        execution_id = %execution.id,
                        %error,
                        "failed to cancel stalled execution on its admitted daemon"
                    );
                }
            }

            let now = now_rfc3339();
            let event = crate::task_service::execution_stalled_domain_event(
                &execution,
                &stale_before,
                &now,
            );
            let (updated, committed_event) = ExecutionRepo::update_with_event(
                &*self.db,
                UpdateExecution {
                    id: execution.id.clone(),
                    status: Some(ExecutionStatus::Failed),
                    stop_reason: Some(Some(StopReason::ExecutionStalled)),
                    stopped_by: Some(Some(
                        api_types::Actor::system(api_types::SystemComponent::HeartbeatMonitor)
                            .display(),
                    )),
                    resume_policy: Some(Some(ResumePolicy::Manual)),
                    stopped_at: Some(Some(now.clone())),
                    external_session_id: None,
                    agent_message_id: None,
                    last_activity_at: Some(Some(now.clone())),
                    summary: None,
                    logs_path: None,
                    before_sha: None,
                    after_sha: None,
                    error: Some(Some(format!(
                        "Execution stalled: no activity since before {stale_before}"
                    ))),
                    executor_config_snapshot_json: None,
                    updated_at: now,
                },
                event,
            )
            .await?;
            crate::DomainEventService::publish_committed_hint(&self.event_bus, &committed_event);

            revoke_active_workspace_lease(&self.db, &updated.id).await;

            self.publish(ForgeEvent {
                event_type: "execution.stalled".to_owned(),
                entity_id: updated.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::ExecutionStalled {
                    task_id: updated.task_id.clone(),
                    execution_id: updated.id.clone(),
                    stale_before: stale_before.clone(),
                },
            });
            self.publish(ForgeEvent {
                event_type: "reconciliation.event".to_owned(),
                entity_id: updated.task_id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::ReconciliationEvent {
                    task_id: Some(updated.task_id.clone()),
                    execution_id: Some(updated.id.clone()),
                    reason: "stalled".to_owned(),
                },
            });

            if stalled_execution_should_block_task(&updated) {
                if let Some(task_service) = self.task_service.as_ref() {
                    if let Err(error) = task_service.annotate_executor_failure_block(&updated).await
                    {
                        tracing::warn!(
                            execution_id = %updated.id,
                            task_id = %updated.task_id,
                            %error,
                            "failed to cascade stalled execution"
                        );
                    }
                }
            }
            stalled += 1;
        }

        if stalled > 0 {
            tracing::info!(
                stalled_executions = stalled,
                "heartbeat monitor detected stalled executions"
            );
        }
        Ok(stalled)
    }

    async fn check_disconnected_daemon_executions(&self) -> Result<u64> {
        let Some(daemon_connections) = self.daemon_connections.as_ref() else {
            return Ok(0);
        };

        let pending_provider_starts: HashSet<String> =
            ExecutionRepo::list_pending_provider_starts(&*self.db)
                .await?
                .into_iter()
                .map(|execution| execution.id)
                .collect();
        let running: Vec<_> = ExecutionRepo::list_running(&*self.db)
            .await?
            .into_iter()
            .filter(|execution| !pending_provider_starts.contains(&execution.id))
            .collect();
        let mut running_ids = HashMap::new();
        for execution in &running {
            running_ids.insert(execution.id.clone(), ());
        }

        {
            let mut observed = self
                .disconnect_observed
                .lock()
                .expect("disconnect observation lock");
            observed.retain(|execution_id, _| running_ids.contains_key(execution_id));
        }

        let mut disconnected = 0_u64;
        let now = Instant::now();

        for execution in running {
            let (daemon_id, daemon) = match resolve_execution_daemon(&self.db, &execution).await {
                Ok(Some(host)) => host,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(
                        execution_id = %execution.id,
                        %error,
                        "skipping daemon disconnect recovery without a frozen Execution host"
                    );
                    continue;
                }
            };
            if is_embedded_daemon_machine(&daemon.machine_id) {
                continue;
            }
            if daemon_connections.is_connected(&daemon_id) {
                self.disconnect_observed
                    .lock()
                    .expect("disconnect observation lock")
                    .remove(&execution.id);
                continue;
            }

            let first_observed = {
                let mut observed = self
                    .disconnect_observed
                    .lock()
                    .expect("disconnect observation lock");
                observed
                    .entry(execution.id.clone())
                    .or_insert_with(|| now)
                    .to_owned()
            };
            if now.duration_since(first_observed) < self.daemon_disconnect_grace {
                continue;
            }

            let updated = fail_execution_daemon_disconnected(
                &self.db,
                &self.event_bus,
                self.task_service.as_deref(),
                FailDaemonDisconnectedExecution {
                    execution: &execution,
                    daemon_id: &daemon_id,
                    error_message: format!("Remote daemon {daemon_id} disconnected"),
                    stopped_by: &api_types::Actor::system(
                        api_types::SystemComponent::HeartbeatMonitor,
                    )
                    .display(),
                    reconciliation_reason: "daemon_disconnected",
                },
            )
            .await?;
            if updated.status == ExecutionStatus::Running {
                self.disconnect_observed
                    .lock()
                    .expect("disconnect observation lock")
                    .remove(&execution.id);
                continue;
            }
            self.disconnect_observed
                .lock()
                .expect("disconnect observation lock")
                .remove(&updated.id);
            disconnected += 1;
        }

        if disconnected > 0 {
            tracing::info!(
                disconnected_executions = disconnected,
                "heartbeat monitor interrupted executions on disconnected daemons"
            );
        }
        Ok(disconnected)
    }

    fn publish(&self, event: ForgeEvent) {
        self.event_bus.publish(event);
    }
}

const WORKSPACE_LEASE_RENEW_WINDOW_SECONDS: i64 = 5 * 60;
const WORKSPACE_LEASE_EXTENSION_SECONDS: i64 = 15 * 60;

async fn renew_workspace_leases(db: &SqliteDb) -> Result<u64> {
    let now = chrono::Utc::now();
    let renewed = WorkspaceLeaseRepo::renew_active(
        db,
        &now.to_rfc3339(),
        &(now + chrono::Duration::seconds(WORKSPACE_LEASE_RENEW_WINDOW_SECONDS)).to_rfc3339(),
        &(now + chrono::Duration::seconds(WORKSPACE_LEASE_EXTENSION_SECONDS)).to_rfc3339(),
        500,
    )
    .await?;
    if !renewed.is_empty() {
        tracing::debug!(
            renewed_leases = renewed.len(),
            "renewed active WorkspaceLeases"
        );
    }
    Ok(renewed.len() as u64)
}

/// Expire scheduler grants and, during the live heartbeat pass, stop any
/// execution that lost its authority.  Startup recovery deliberately leaves
/// the execution running long enough for `recover_task` to apply its normal
/// requeue/block policy; the heartbeat path terminalizes it immediately.
async fn expire_workspace_leases(
    db: &Arc<SqliteDb>,
    event_bus: &Arc<EventBus>,
    daemon_connections: Option<&DaemonConnectionRegistry>,
    task_executor: Option<&dyn TaskExecutor>,
    task_service: Option<&TaskService>,
    terminalize_running: bool,
) -> Result<u64> {
    let expired = WorkspaceLeaseRepo::expire(db.as_ref(), &now_rfc3339(), 500).await?;
    let expired_count = expired.len() as u64;
    let default_task_service;
    let authority_task_service = match task_service {
        Some(task_service) => task_service,
        None => {
            default_task_service = TaskService::new(Arc::clone(db), Arc::clone(event_bus));
            &default_task_service
        }
    };
    let unresolved_pending =
        resolve_pending_starts_without_authority(db.as_ref(), authority_task_service).await?;
    if !terminalize_running {
        return Ok(expired_count + unresolved_pending);
    }

    for lease in expired {
        let Some(execution) = ExecutionRepo::get_by_id(db.as_ref(), &lease.execution_id).await?
        else {
            continue;
        };
        if execution.status != ExecutionStatus::Running {
            continue;
        }

        if task_executor.is_some() || daemon_connections.is_some() || task_service.is_some() {
            if let Err(error) = cancel_execution_on_frozen_host(
                db,
                &execution,
                "workspace lease expired",
                daemon_connections,
                task_service,
                task_executor,
            )
            .await
            {
                tracing::warn!(
                    execution_id = %execution.id,
                    %error,
                    "failed to cancel expired-lease execution on its admitted daemon"
                );
            }
        }

        let now = now_rfc3339();
        let event = crate::task_service::execution_stalled_domain_event(
            &execution,
            &lease.expires_at,
            &now,
        );
        let updated = match ExecutionRepo::update_with_event(
            db.as_ref(),
            UpdateExecution {
                id: execution.id.clone(),
                status: Some(ExecutionStatus::Failed),
                stop_reason: Some(Some(StopReason::ExecutionStalled)),
                stopped_by: Some(Some(
                    api_types::Actor::system(api_types::SystemComponent::HeartbeatMonitor)
                        .display(),
                )),
                resume_policy: Some(Some(ResumePolicy::Manual)),
                stopped_at: Some(Some(now.clone())),
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: Some(Some(now.clone())),
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: Some(Some("scheduler WorkspaceLease expired".to_owned())),
                executor_config_snapshot_json: None,
                updated_at: now,
            },
            event,
        )
        .await
        {
            Ok((updated, committed_event)) => {
                crate::DomainEventService::publish_committed_hint(event_bus, &committed_event);
                updated
            }
            Err(error) => {
                tracing::warn!(
                    execution_id = %execution.id,
                    %error,
                    "failed to stop execution after WorkspaceLease expiry"
                );
                continue;
            }
        };

        event_bus.publish(ForgeEvent {
            event_type: "execution.stalled".to_owned(),
            entity_id: updated.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ExecutionStalled {
                task_id: updated.task_id.clone(),
                execution_id: updated.id.clone(),
                stale_before: lease.expires_at.clone(),
            },
        });
        event_bus.publish(ForgeEvent {
            event_type: "reconciliation.event".to_owned(),
            entity_id: updated.task_id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ReconciliationEvent {
                task_id: Some(updated.task_id.clone()),
                execution_id: Some(updated.id.clone()),
                reason: "workspace_lease_expired".to_owned(),
            },
        });
        if let Err(error) = authority_task_service
            .annotate_executor_failure_block(&updated)
            .await
        {
            tracing::warn!(
                execution_id = %updated.id,
                task_id = %updated.task_id,
                %error,
                "failed to cascade expired WorkspaceLease execution"
            );
        }
    }

    Ok(expired_count + unresolved_pending)
}

async fn resolve_pending_starts_without_authority(
    db: &SqliteDb,
    task_service: &TaskService,
) -> Result<u64> {
    let pending = ExecutionRepo::list_pending_provider_starts(db).await?;
    let mut resolved = 0;
    for execution in pending {
        let Err(error) = task_service
            .verify_execution_workspace_authority(&execution)
            .await
        else {
            continue;
        };
        if !is_lost_execution_authority(&error) {
            return Err(error);
        }

        let current = ExecutionRepo::get_by_id(db, &execution.id).await?;
        if !current.is_some_and(|current| current.status == ExecutionStatus::Running) {
            continue;
        }
        let still_pending = ExecutionRepo::list_pending_provider_starts(db)
            .await?
            .into_iter()
            .any(|pending| pending.id == execution.id);
        if !still_pending {
            continue;
        }
        let resolved_execution = task_service
            .fail_execution_before_dispatch(
                &execution.id,
                format!("provider Start remained pending after its authority was lost: {error}"),
            )
            .await?;
        if resolved_execution.status != ExecutionStatus::Running {
            resolved += 1;
        }
    }
    Ok(resolved)
}

fn is_lost_execution_authority(error: &ServiceError) -> bool {
    matches!(
        error,
        ServiceError::InvalidOperation { .. }
            | ServiceError::NotFound { .. }
            | ServiceError::Conflict(_)
            | ServiceError::AuthorizationDenied { .. }
    )
}

async fn revoke_active_workspace_lease(db: &SqliteDb, execution_id: &str) {
    let execution = match ExecutionRepo::get_by_id(db, execution_id).await {
        Ok(Some(execution)) => execution,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, execution_id, "failed to load Execution lease binding");
            return;
        }
    };
    let lease_result = match execution.work_unit_id.as_deref() {
        Some(work_unit_id) => WorkspaceLeaseRepo::get_active_for_work_unit(db, work_unit_id).await,
        None => WorkspaceLeaseRepo::get_active_for_task(db, &execution.task_id).await,
    };
    match lease_result {
        Ok(Some(lease)) if lease.execution_id == execution_id => {
            if let Err(error) =
                WorkspaceLeaseRepo::revoke(db, &lease.id, lease.version, &now_rfc3339()).await
            {
                tracing::warn!(
                    execution_id,
                    lease_id = %lease.id,
                    %error,
                    "failed to revoke WorkspaceLease at recovery terminal boundary"
                );
            }
        }
        Ok(Some(_)) => {
            tracing::debug!(
                execution_id,
                "leaving another Execution's WorkspaceLease active"
            );
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, execution_id, "failed to load WorkspaceLease at recovery terminal boundary")
        }
    }
}

pub(crate) struct CancelledExecution {
    pub execution_id: String,
    pub resumable_external_session_id: Option<String>,
    pub exact_work_unit_target: bool,
}

pub(crate) struct RecoverTaskOutcome {
    pub task: Task,
    pub annotated: bool,
}

fn group_executions_by_task(
    executions: impl IntoIterator<Item = Execution>,
) -> BTreeMap<String, Vec<Execution>> {
    let mut grouped = BTreeMap::<String, Vec<Execution>>::new();
    for execution in executions {
        grouped
            .entry(execution.task_id.clone())
            .or_default()
            .push(execution);
    }
    grouped
}

async fn recover_executions(
    db: &Arc<SqliteDb>,
    event_bus: &Arc<EventBus>,
    task: Task,
    executions: Vec<Execution>,
    stop_reason: StopReason,
    stopped_by: &api_types::Actor,
) -> Result<RecoverTaskOutcome> {
    let cancelled = cancel_execution_records(
        db,
        &executions,
        stop_reason.clone(),
        stopped_by,
        ResumePolicy::Manual,
    )
    .await?;

    // A cancelled attempt must never retain its repository authority. If the
    // recovery policy later schedules a retry, it receives a fresh execution
    // identity and lease through TaskService admission.
    for execution in &cancelled {
        revoke_active_workspace_lease(db, &execution.execution_id).await;
    }

    if cancelled.is_empty() {
        return Ok(RecoverTaskOutcome {
            task,
            annotated: false,
        });
    }

    let has_exact_resumable_crash_recovery = stop_reason == StopReason::CrashRecovery
        && cancelled.iter().any(|execution| {
            execution.exact_work_unit_target && execution.resumable_external_session_id.is_some()
        });
    if has_exact_resumable_crash_recovery {
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(db.as_ref(), &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        let task_has_no_blocking_metadata = task.error_annotation.is_none()
            && task.blocked_json.is_none()
            && task.failed_json.is_none();
        if lifecycle.state == TaskLifecycleState::Active && task_has_no_blocking_metadata {
            // Recovery policy belongs to each exact Execution. A non-resumable
            // sibling remains Manual while resumable siblings retain their
            // existing session identity for the automatic dispatcher.
            for execution in cancelled.iter().filter(|execution| {
                execution.exact_work_unit_target
                    && execution.resumable_external_session_id.is_some()
            }) {
                ExecutionRepo::update(
                    db.as_ref(),
                    UpdateExecution {
                        id: execution.execution_id.clone(),
                        status: None,
                        stop_reason: None,
                        stopped_by: None,
                        resume_policy: Some(Some(ResumePolicy::Auto)),
                        stopped_at: None,
                        external_session_id: None,
                        agent_message_id: None,
                        last_activity_at: None,
                        summary: None,
                        logs_path: None,
                        before_sha: None,
                        after_sha: None,
                        error: None,
                        executor_config_snapshot_json: None,
                        updated_at: now_rfc3339(),
                    },
                )
                .await?;
            }
            return Ok(RecoverTaskOutcome {
                task,
                annotated: false,
            });
        }
    }

    let every_execution_is_resumable = cancelled.iter().all(|execution| {
        execution.exact_work_unit_target && execution.resumable_external_session_id.is_some()
    });
    if stop_reason == StopReason::AgentTimeout && every_execution_is_resumable {
        tracing::warn!(
            task_id = %task.id,
            execution_count = cancelled.len(),
            "recovered task without an automatic session restart after Agent timeout"
        );
        return Ok(RecoverTaskOutcome {
            task,
            annotated: false,
        });
    }

    let exact_recovery_target = if cancelled.len() == 1 && cancelled[0].exact_work_unit_target {
        Some(cancelled[0].execution_id.clone())
    } else {
        None
    };
    let (blocking_reason, message) = match (exact_recovery_target.is_some(), cancelled.len()) {
        (true, _) if stop_reason == StopReason::AgentTimeout => {
            ("agent_timeout", "Recovered after agent heartbeat timeout")
        }
        (true, _) => ("crash_recovery", "Recovered after server restart"),
        (false, 1) => (
            "recovery_target_unavailable",
            "Recovery stopped an Execution without a provable open WorkUnit target",
        ),
        (false, _) => (
            "recovery_target_ambiguous",
            "Recovery stopped multiple Executions; choose an exact WorkUnit target before retrying",
        ),
    };
    let artifact = exact_recovery_target.as_ref().map(|execution_id| {
        json!({
            "kind": "execution",
            "id": execution_id,
            "log_path": null,
        })
    });
    let annotation = json!({
        "type": api_types::FailureKind::RecoveryRequired,
        "blocking_reason": blocking_reason,
        "blocked_by": stopped_by.display(),
        "blocked_at": now_rfc3339(),
        "blocked_execution_id": exact_recovery_target,
        "artifact": artifact,
        "message": message,
        "recovery_actions": if exact_recovery_target.is_some() {
            vec!["reexecute", "cancel_task"]
        } else {
            Vec::new()
        },
    })
    .to_string();
    let reason_ref = exact_recovery_target
        .clone()
        .unwrap_or_else(|| format!("task-version:{}", task.version));
    let cause = exact_recovery_target
        .clone()
        .map(crate::task_lifecycle::LifecycleCause::Execution)
        .unwrap_or_else(|| crate::task_lifecycle::LifecycleCause::Actor(stopped_by.clone()));
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(db.as_ref(), &task.id)
        .await?
        .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
    if lifecycle.state != TaskLifecycleState::Active {
        return Ok(RecoverTaskOutcome {
            task,
            annotated: false,
        });
    }
    let block =
        crate::task_lifecycle::TaskLifecycleService::new(Arc::clone(db), Arc::clone(event_bus))
            .block(
                &task.id,
                cause,
                blocking_reason,
                reason_ref.clone(),
                format!(
                    "recovery-required:{}:{blocking_reason}:{reason_ref}",
                    task.id
                ),
            )
            .await?;
    if block.lifecycle.state != TaskLifecycleState::Blocked
        || block.lifecycle.reason_kind.as_deref() != Some(blocking_reason)
        || block.lifecycle.reason_ref.as_deref() != Some(reason_ref.as_str())
    {
        return Ok(RecoverTaskOutcome {
            task: block.task,
            annotated: false,
        });
    }
    let task = block.task;
    let task = TaskRepo::update(
        db.as_ref(),
        UpdateTask {
            id: task.id.clone(),
            expected_version: task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(Some(annotation)),
            blocked_json: None,
            failed_json: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await?;

    Ok(RecoverTaskOutcome {
        task,
        annotated: true,
    })
}

async fn sweep_stale_recovery_annotations(db: &Arc<SqliteDb>) -> Result<u64> {
    let mut cleared = 0_u64;

    for project in list_projects(db).await? {
        let mut cursor = None;
        loop {
            let page = TaskRepo::list(
                db.as_ref(),
                TaskListQuery {
                    project_id: project.id.clone(),
                    q: None,
                    lifecycle_states: Vec::new(),
                    agent_ids: Vec::new(),
                    assignee_types: Vec::new(),
                    assignee_ids: Vec::new(),
                    priority: None,
                    include_cancelled: false,
                    include_deleted: false,
                    page: page_request(cursor),
                },
            )
            .await?;

            for task in page.items {
                let Some(annotation_json) = task.error_annotation.as_deref() else {
                    continue;
                };
                let Ok(annotation) = serde_json::from_str::<serde_json::Value>(annotation_json)
                else {
                    continue;
                };
                if annotation.get("type").and_then(serde_json::Value::as_str)
                    != Some("recovery_required")
                {
                    continue;
                }

                let blocked_execution_id =
                    annotation.get("blocked_execution_id").and_then(|value| {
                        if value.is_null() {
                            None
                        } else {
                            value.as_str()
                        }
                    });

                let should_clear = match blocked_execution_id {
                    // Ambiguous recovery records stay blocked until an
                    // explicit exact-target decision resolves them.
                    None => false,
                    Some(execution_id) => {
                        match ExecutionRepo::get_by_id(db.as_ref(), execution_id).await? {
                            None => true,
                            Some(execution) => !execution_awaits_recovery(&execution),
                        }
                    }
                };

                if !should_clear {
                    continue;
                }

                TaskRepo::update(
                    db.as_ref(),
                    UpdateTask {
                        id: task.id.clone(),
                        expected_version: task.version,
                        title: None,
                        description: None,
                        priority: None,
                        merge_config: None,
                        error_annotation: Some(None),
                        blocked_json: None,
                        failed_json: None,
                        parent_task_id: None,
                        updated_at: now_rfc3339(),
                    },
                )
                .await?;
                cleared += 1;
            }

            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }

    Ok(cleared)
}

fn execution_awaits_recovery(execution: &db::Execution) -> bool {
    execution.status == ExecutionStatus::Cancelled
        && matches!(execution.resume_policy, Some(ResumePolicy::Manual))
}

pub(crate) async fn cancel_running_executions(
    db: &SqliteDb,
    task_id: &str,
    stop_reason: StopReason,
    stopped_by: &api_types::Actor,
    resume_policy: ResumePolicy,
) -> Result<Vec<CancelledExecution>> {
    let pending_provider_starts: HashSet<String> = ExecutionRepo::list_pending_provider_starts(db)
        .await?
        .into_iter()
        .map(|execution| execution.id)
        .collect();
    let mut cursor = None;
    let mut running = Vec::new();
    loop {
        let page = ExecutionRepo::list_by_task(
            db,
            task_id,
            PageRequest {
                cursor,
                limit: 500,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Asc,
            },
        )
        .await?;
        running.extend(page.items.into_iter().filter(|execution| {
            execution.status == ExecutionStatus::Running
                && !pending_provider_starts.contains(&execution.id)
        }));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    cancel_execution_records(db, &running, stop_reason, stopped_by, resume_policy).await
}

async fn cancel_execution_records(
    db: &SqliteDb,
    executions: &[Execution],
    stop_reason: StopReason,
    stopped_by: &api_types::Actor,
    resume_policy: ResumePolicy,
) -> Result<Vec<CancelledExecution>> {
    let mut cancelled = Vec::new();
    for execution in executions {
        if execution.status != ExecutionStatus::Running {
            continue;
        }
        let exact_work_unit_target = crate::task_service::execution::has_exact_work_unit_target(
            db,
            &execution.task_id,
            execution,
        )
        .await?;
        let resumable_external_session_id = if exact_work_unit_target {
            crate::task_service::resumable_external_session(
                db,
                execution,
                execution.agent_id.as_deref(),
                execution.workspace_id.as_deref(),
            )
            .await?
        } else {
            None
        };
        let cancelled_execution = CancelledExecution {
            execution_id: execution.id.clone(),
            resumable_external_session_id,
            exact_work_unit_target,
        };
        let execution_resume_policy = if stop_reason == StopReason::GracefulShutdown
            && execution.work_unit_id.is_some()
            && (!exact_work_unit_target
                || cancelled_execution.resumable_external_session_id.is_none())
        {
            // A graceful stop may only auto-dispatch through the exact live
            // HarnessSession. WorkUnit attempts without that authority remain
            // available for an explicit exact operator decision.
            ResumePolicy::Manual
        } else {
            resume_policy.clone()
        };
        let now = now_rfc3339();
        let event = crate::task_service::execution_status_domain_event(
            execution,
            &ExecutionStatus::Cancelled,
            &now,
        );
        if ExecutionRepo::update_with_event(
            db,
            UpdateExecution {
                id: execution.id.clone(),
                status: Some(ExecutionStatus::Cancelled),
                stop_reason: Some(Some(stop_reason.clone())),
                stopped_by: Some(Some(stopped_by.display())),
                resume_policy: Some(Some(execution_resume_policy)),
                stopped_at: Some(Some(now.clone())),
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: Some(Some("Recovered".to_owned())),
                executor_config_snapshot_json: None,
                updated_at: now,
            },
            event,
        )
        .await
        .is_ok()
        {
            cancelled.push(cancelled_execution);
        }
    }
    Ok(cancelled)
}

async fn list_projects(db: &SqliteDb) -> Result<Vec<Project>> {
    let mut projects = Vec::new();
    let mut cursor = None;
    loop {
        let page = ProjectRepo::list(db, page_request(cursor)).await?;
        projects.extend(page.items);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(projects)
}

fn page_request(cursor: Option<String>) -> PageRequest {
    PageRequest {
        cursor,
        limit: 500,
        include_total: false,
        sort_by: SortBy::CreatedAt,
        sort_order: SortOrder::Asc,
    }
}

fn stalled_execution_should_block_task(execution: &db::Execution) -> bool {
    matches!(
        execution.role.as_str(),
        "interactive" | "executor" | "coder"
    )
}

pub(crate) async fn resolve_execution_daemon(
    db: &SqliteDb,
    execution: &Execution,
) -> Result<Option<(String, Daemon)>> {
    if execution.agent_id.is_none() {
        return Ok(None);
    }
    let daemon_id = crate::daemon_transport::router::resolved_daemon_id_for_execution(execution)?;
    let daemon = DaemonRepo::get_by_id(db, &daemon_id)
        .await?
        .ok_or_else(|| ServiceError::not_found("daemon", daemon_id.clone()))?;
    Ok(Some((daemon_id, daemon)))
}

pub(crate) async fn cancel_execution_on_frozen_host(
    db: &SqliteDb,
    execution: &Execution,
    reason: &str,
    daemon_connections: Option<&DaemonConnectionRegistry>,
    task_service: Option<&TaskService>,
    task_executor: Option<&dyn TaskExecutor>,
) -> Result<()> {
    if let Some(task_service) = task_service {
        return task_service
            .cancel_execution_with_provider(execution, reason)
            .await;
    }
    let Some((daemon_id, daemon)) = resolve_execution_daemon(db, execution).await? else {
        return Err(ServiceError::invalid_operation(
            "execution has no Agent host snapshot for recovery cancellation",
        ));
    };
    let params = api_types::ExecutionCancelParams {
        execution_id: execution.id.clone(),
        reason: Some(reason.to_owned()),
    };
    if let Some(registry) = daemon_connections {
        let provider =
            crate::daemon_transport::select_execution_provider(Some(&daemon_id), db, registry)
                .await?;
        provider.cancel(params).await?;
        return Ok(());
    }
    if daemon.status == db::DaemonStatus::Online && is_embedded_daemon_machine(&daemon.machine_id) {
        let task_executor = task_executor.ok_or_else(|| {
            ServiceError::invalid_operation(
                "local execution cancellation is not configured for its frozen daemon",
            )
        })?;
        return task_executor.cancel(&execution.id).await.map_err(|error| {
            ServiceError::invalid_operation(format!(
                "local execution cancellation failed on frozen daemon {daemon_id}: {error}"
            ))
        });
    }
    Err(ServiceError::DaemonUnavailable { daemon_id })
}

pub(crate) struct FailDaemonDisconnectedExecution<'a> {
    pub execution: &'a Execution,
    pub daemon_id: &'a str,
    pub error_message: String,
    pub stopped_by: &'a str,
    pub reconciliation_reason: &'a str,
}

pub(crate) async fn fail_execution_daemon_disconnected(
    db: &SqliteDb,
    event_bus: &EventBus,
    task_service: Option<&TaskService>,
    input: FailDaemonDisconnectedExecution<'_>,
) -> Result<Execution> {
    let FailDaemonDisconnectedExecution {
        execution,
        daemon_id,
        error_message,
        stopped_by,
        reconciliation_reason,
    } = input;
    let Some(current) = ExecutionRepo::get_by_id(db, &execution.id).await? else {
        return Err(ServiceError::not_found("execution", execution.id.clone()));
    };
    if current.status != ExecutionStatus::Running {
        return Ok(current);
    }
    let now = now_rfc3339();
    let event = crate::task_service::execution_status_domain_event(
        execution,
        &ExecutionStatus::Failed,
        &now,
    );
    let update = UpdateExecution {
        id: execution.id.clone(),
        status: Some(ExecutionStatus::Failed),
        stop_reason: Some(Some(StopReason::DaemonDisconnected)),
        stopped_by: Some(Some(stopped_by.to_owned())),
        resume_policy: Some(Some(ResumePolicy::Manual)),
        stopped_at: Some(Some(now.clone())),
        external_session_id: None,
        agent_message_id: None,
        last_activity_at: Some(Some(now.clone())),
        summary: None,
        logs_path: None,
        before_sha: None,
        after_sha: None,
        error: Some(Some(error_message)),
        executor_config_snapshot_json: None,
        updated_at: now,
    };
    let (updated, committed_event) =
        match ExecutionRepo::fail_running_unless_provider_start_pending(
            db,
            &execution.id,
            update,
            event,
        )
        .await?
        {
            db::RunningExecutionFailure::Failed(updated, event) => (*updated, event),
            db::RunningExecutionFailure::Pending(current)
            | db::RunningExecutionFailure::NotRunning(current) => return Ok(current),
        };
    crate::DomainEventService::publish_committed_hint(event_bus, &committed_event);

    revoke_active_workspace_lease(db, &updated.id).await;

    event_bus.publish(ForgeEvent {
        event_type: "execution.daemon_disconnected".to_owned(),
        entity_id: updated.id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::ExecutionDaemonDisconnected {
            task_id: updated.task_id.clone(),
            execution_id: updated.id.clone(),
            daemon_id: daemon_id.to_owned(),
        },
    });
    event_bus.publish(ForgeEvent {
        event_type: "reconciliation.event".to_owned(),
        entity_id: updated.task_id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::ReconciliationEvent {
            task_id: Some(updated.task_id.clone()),
            execution_id: Some(updated.id.clone()),
            reason: reconciliation_reason.to_owned(),
        },
    });

    if stalled_execution_should_block_task(&updated) {
        if let Some(task_service) = task_service {
            if let Err(error) = task_service.annotate_executor_failure_block(&updated).await {
                tracing::warn!(
                    execution_id = %updated.id,
                    task_id = %updated.task_id,
                    %error,
                    "failed to cascade daemon-disconnected execution"
                );
            }
        }
    }

    Ok(updated)
}

pub(crate) const DAEMON_REPORT_RECONCILE_MIN_AGE: Duration = Duration::from_secs(60);

pub(crate) async fn reconcile_daemon_report_executions(
    db: &SqliteDb,
    event_bus: &EventBus,
    task_service: Option<&TaskService>,
    daemon: &Daemon,
    active_execution_ids: &[String],
) -> Result<u64> {
    if is_embedded_daemon_machine(&daemon.machine_id) {
        return Ok(0);
    }

    let created_before = (Utc::now()
        - ChronoDuration::seconds(DAEMON_REPORT_RECONCILE_MIN_AGE.as_secs() as i64))
    .to_rfc3339();
    let executions = ExecutionRepo::list_running_for_daemon_not_in(
        db,
        &daemon.id,
        &created_before,
        active_execution_ids,
    )
    .await?;

    let mut interrupted = 0_u64;
    for execution in executions {
        let updated = fail_execution_daemon_disconnected(
            db,
            event_bus,
            task_service,
            FailDaemonDisconnectedExecution {
                execution: &execution,
                daemon_id: &daemon.id,
                error_message: "daemon no longer running this execution".to_owned(),
                stopped_by: &api_types::Actor::system(api_types::SystemComponent::DaemonReport)
                    .display(),
                reconciliation_reason: "daemon_disconnected",
            },
        )
        .await?;
        if updated.status == ExecutionStatus::Running {
            continue;
        }
        interrupted += 1;
    }
    Ok(interrupted)
}

fn agent_timed_out(agent: &Agent) -> bool {
    let Some(last_heartbeat_at) = &agent.last_heartbeat_at else {
        return true;
    };
    let Some(last_heartbeat_at) = parse_rfc3339_unix_seconds(last_heartbeat_at) else {
        return true;
    };
    let heartbeat_interval = agent.heartbeat_interval_seconds.max(1) as u64;
    let max_missed = agent.max_missed_heartbeats.max(1) as u64;
    let timeout = heartbeat_interval.saturating_mul(max_missed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    now.saturating_sub(last_heartbeat_at) > timeout
}

fn parse_rfc3339_unix_seconds(value: &str) -> Option<u64> {
    let (date, time_with_offset) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i32>().ok()?;
    let month = date_parts.next()?.parse::<u32>().ok()?;
    let day = date_parts.next()?.parse::<u32>().ok()?;

    let (time, offset_seconds) = split_time_offset(time_with_offset)?;
    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse::<u32>().ok()?;
    let minute = time_parts.next()?.parse::<u32>().ok()?;
    let second = time_parts.next()?.split('.').next()?.parse::<u32>().ok()?;

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)?
        .checked_add((hour as i64).checked_mul(3_600)?)?
        .checked_add((minute as i64).checked_mul(60)?)?
        .checked_add(second as i64)?
        .checked_sub(offset_seconds as i64)?;

    u64::try_from(seconds).ok()
}

fn split_time_offset(value: &str) -> Option<(&str, i32)> {
    if let Some(time) = value.strip_suffix('Z') {
        return Some((time, 0));
    }
    let offset_index = value
        .char_indices()
        .skip(1)
        .find_map(|(index, character)| matches!(character, '+' | '-').then_some(index))?;
    let (time, offset) = value.split_at(offset_index);
    let sign = if offset.starts_with('+') { 1 } else { -1 };
    let mut offset_parts = offset[1..].split(':');
    let hours = offset_parts.next()?.parse::<i32>().ok()?;
    let minutes = offset_parts.next()?.parse::<i32>().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some((time, sign * (hours * 3_600 + minutes * 60)))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        daemon_service::{DaemonReportInput, DaemonService, DetectedCliInput},
        daemon_transport::{DaemonConnection, DaemonConnectionRegistry},
        TaskDispatcher, TaskService,
    };
    use db::{
        create_sqlite_pool, new_uuid_v4, run_migrations, AgentRepo, CreateAgent, CreateExecution,
        CreateProject, CreateRepo, CreateRoleMembership, CreateTask, CreateTaskRole, DaemonRepo,
        DaemonStatus, RepoRepo, RoleMembershipRepo, RoleMembershipStatus, TaskLifecycleState,
        TaskRoleRepo, UpdateAgent, UpdateDaemonReport, UpdateProject, UpdateRoleMembership,
        UpsertDaemon, User, UserRepo, WorkUnitRepo,
    };
    use executors::{ExecutionContext, ExecutionOutcome, ExecutionResult, ExecutorError};
    use serde_json::Value;

    #[derive(Default)]
    struct RecordingCancelExecutor {
        cancelled: std::sync::Mutex<Vec<String>>,
    }

    #[derive(Default)]
    struct RecoveryStartExecutor {
        calls: std::sync::atomic::AtomicUsize,
        started: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl TaskExecutor for RecoveryStartExecutor {
        async fn execute(
            &self,
            _ctx: ExecutionContext,
        ) -> std::result::Result<ExecutionResult, ExecutorError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.started.notify_one();
            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                ..Default::default()
            })
        }

        async fn cancel(&self, _execution_id: &str) -> std::result::Result<(), ExecutorError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl TaskExecutor for RecordingCancelExecutor {
        async fn execute(
            &self,
            _ctx: ExecutionContext,
        ) -> std::result::Result<ExecutionResult, ExecutorError> {
            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                after_sha: None,
                agent_session_id: None,
                summary: None,
                error: None,
                usage: None,
                ..Default::default()
            })
        }

        async fn cancel(&self, execution_id: &str) -> std::result::Result<(), ExecutorError> {
            self.cancelled
                .lock()
                .expect("cancel log lock")
                .push(execution_id.to_owned());
            Ok(())
        }
    }

    async fn sqlite_db() -> SqliteDb {
        let pool = create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        run_migrations(&pool).await.expect("migrations run");
        SqliteDb::new(pool)
    }

    async fn seed_project_repo(db: &SqliteDb) -> (String, String) {
        let now = now_rfc3339();
        let project_id = new_uuid_v4();
        let repo_id = new_uuid_v4();
        ProjectRepo::create(
            db,
            CreateProject {
                id: project_id.clone(),
                name: "Forge".to_owned(),
                settings: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("project creates");
        RepoRepo::create(
            db,
            CreateRepo {
                id: repo_id.clone(),
                project_id: project_id.clone(),
                name: "forge".to_owned(),
                remote_url: "https://example.com/forge.git".to_owned(),
                local_path: None,
                work_mode: db::WorkMode::DirectMerge,
                default_branch: "main".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("repo creates");
        ProjectRepo::update(
            db,
            UpdateProject {
                id: project_id.clone(),
                name: None,
                settings: None,
                primary_repo_id: Some(Some(repo_id.clone())),
                paused_at: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("project primary repo updates");
        (project_id, repo_id)
    }

    async fn seed_agent(
        db: &SqliteDb,
        status: AgentStatus,
        last_heartbeat_at: Option<String>,
    ) -> Agent {
        seed_agent_with_harness(db, status, last_heartbeat_at, "shell").await
    }

    async fn seed_agent_with_harness(
        db: &SqliteDb,
        status: AgentStatus,
        last_heartbeat_at: Option<String>,
        harness_kind: &str,
    ) -> Agent {
        let now = now_rfc3339();
        let daemon_id = new_uuid_v4();
        DaemonRepo::upsert_by_machine_id(
            db,
            UpsertDaemon {
                id: daemon_id.clone(),
                machine_id: format!("machine-{daemon_id}"),
                hostname: "test-host".to_owned(),
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                agent_version: None,
                labels_json: "{}".to_owned(),
                status: DaemonStatus::Online,
                registration_token_hash: None,
                owner_id: None,
                visibility: "global".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("daemon creates");

        AgentRepo::create(
            db,
            CreateAgent {
                id: new_uuid_v4(),
                name: harness_kind.to_owned(),
                description: None,
                executor_type: harness_kind.to_owned(),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                capabilities_json: "[]".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: None,
                daemon_id: Some(daemon_id),
                max_concurrent_tasks: 1,
                heartbeat_interval_seconds: 1,
                max_missed_heartbeats: 1,
                status,
                last_heartbeat_at,
                is_default: false,
                paused: false,
                owner_id: None,
                visibility: "global".to_owned(),
                prompt_template: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("agent creates")
    }

    fn recovery_fixture_state(label: &str) -> TaskLifecycleState {
        match label {
            "backlog" => TaskLifecycleState::Backlog,
            "ready" => TaskLifecycleState::Ready,
            "active" => TaskLifecycleState::Active,
            "blocked" => TaskLifecycleState::Blocked,
            "done" => TaskLifecycleState::Done,
            "cancelled" => TaskLifecycleState::Cancelled,
            other => panic!("unknown recovery fixture lifecycle label: {other}"),
        }
    }

    async fn seed_task(
        db: &SqliteDb,
        project_id: String,
        repo_id: String,
        state_label: impl AsRef<str>,
        agent_id: Option<String>,
    ) -> Task {
        seed_task_with_assignee(db, project_id, repo_id, state_label, "agent", agent_id).await
    }

    async fn seed_task_with_assignee(
        db: &SqliteDb,
        project_id: String,
        repo_id: String,
        state_label: impl AsRef<str>,
        assignee_type: &str,
        assignee_id: Option<String>,
    ) -> Task {
        let now = now_rfc3339();
        let task = TaskRepo::create(
            db,
            CreateTask {
                id: new_uuid_v4(),
                project_id,
                repo_id: Some(repo_id),
                parent_task_id: None,
                subtask_order: None,
                title: "Recover me".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                lifecycle_state: recovery_fixture_state(state_label.as_ref()),
                is_automation: false,
                priority: 0,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("task creates");
        if let Some(actor_id) = assignee_id {
            let actor_kind = if assignee_type == "agent" {
                db::ActorKind::Agent
            } else {
                let user = User {
                    id: actor_id.clone(),
                    email: format!("{actor_id}@test.invalid"),
                    password_hash: "test-only".to_owned(),
                    display_name: Some("Recovery Human".to_owned()),
                    is_admin: false,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                };
                UserRepo::create_user(db, &user)
                    .await
                    .expect("Human Actor creates");
                db::ActorKind::Human
            };
            let role = TaskRoleRepo::create(
                db,
                CreateTaskRole {
                    id: new_uuid_v4(),
                    task_id: task.id.clone(),
                    role: db::canonical_task_role_name("coder")
                        .expect("coder resolves to canonical implementer"),
                    coordination_mode: None,
                    policy_json: "{}".to_owned(),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
            )
            .await
            .expect("TaskRole creates");
            RoleMembershipRepo::add(
                db,
                CreateRoleMembership {
                    id: new_uuid_v4(),
                    task_role_id: role.id,
                    actor_kind,
                    actor_id,
                    status: RoleMembershipStatus::Active,
                    created_at: now.clone(),
                    updated_at: now,
                },
            )
            .await
            .expect("RoleMembership creates");
        }
        task
    }

    async fn seed_running_execution(
        db: &SqliteDb,
        task_id: String,
        agent_id: String,
        agent_session_id: Option<String>,
    ) -> db::Execution {
        seed_work_unit_execution(db, task_id, agent_id, agent_session_id, true).await
    }

    async fn seed_pending_provider_execution(
        db: &SqliteDb,
        task_id: String,
        agent_id: String,
    ) -> db::Execution {
        seed_work_unit_execution(db, task_id, agent_id, None, false).await
    }

    async fn seed_work_unit_execution(
        db: &SqliteDb,
        task_id: String,
        agent_id: String,
        agent_session_id: Option<String>,
        provider_started: bool,
    ) -> db::Execution {
        let agent = AgentRepo::get_by_id(db, &agent_id)
            .await
            .expect("agent loads")
            .expect("agent exists");
        let resolved_daemon_id = match agent.daemon_id.as_deref() {
            Some(daemon_id) => daemon_id.to_owned(),
            None => {
                let machine_id = crate::embedded_daemon::embedded_machine_id();
                let existing: Option<String> =
                    sqlx::query_scalar("SELECT id FROM daemon WHERE machine_id = ? LIMIT 1")
                        .bind(&machine_id)
                        .fetch_optional(db.pool())
                        .await
                        .expect("embedded test daemon lookup succeeds");
                match existing {
                    Some(daemon_id) => daemon_id,
                    None => {
                        let now = now_rfc3339();
                        let daemon_id = new_uuid_v4();
                        DaemonRepo::upsert_by_machine_id(
                            db,
                            UpsertDaemon {
                                id: daemon_id.clone(),
                                machine_id,
                                hostname: "embedded-test-host".to_owned(),
                                os: "linux".to_owned(),
                                arch: "x86_64".to_owned(),
                                agent_version: None,
                                labels_json: "{}".to_owned(),
                                status: DaemonStatus::Online,
                                registration_token_hash: None,
                                owner_id: None,
                                visibility: "global".to_owned(),
                                created_at: now.clone(),
                                updated_at: now,
                            },
                        )
                        .await
                        .expect("embedded test daemon creates");
                        daemon_id
                    }
                }
            }
        };
        let resume_support = if agent.executor_type == "codex" {
            "native"
        } else {
            "unsupported"
        };
        let now = now_rfc3339();
        let execution_id = new_uuid_v4();
        let work_unit_id = new_uuid_v4();
        let actor = db::ActorRef::Agent(agent_id.clone());
        let work_unit = db::WorkUnitRepo::create(
            db,
            db::CreateWorkUnit {
                id: work_unit_id.clone(),
                task_id: task_id.clone(),
                parent_work_unit_id: None,
                title: "Recovery fixture".to_owned(),
                scope: "exact recovery target".to_owned(),
                role: "implementer".to_owned(),
                assigned_actor: Some(actor.clone()),
                requires_integration: false,
                provenance: None,
                created_by: actor.clone(),
                created_at: now.clone(),
            },
            db::CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "work_unit.created".to_owned(),
                entity_type: "work_unit".to_owned(),
                entity_id: work_unit_id.clone(),
                actor_type: "agent".to_owned(),
                actor_id: Some(agent_id.clone()),
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: work_unit_id.clone(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("recovery-work-unit:{work_unit_id}")),
                payload_json: serde_json::json!({"work_unit_id": work_unit_id}).to_string(),
                created_at: now.clone(),
            },
        )
        .await
        .expect("exact WorkUnit creates")
        .record;
        let mut execution = db::WorkUnitExecutionRepo::create_for_work_unit(
            db,
            db::CreateWorkUnitExecution {
                execution: CreateExecution {
                    id: execution_id.clone(),
                    task_id,
                    agent_id: Some(agent_id.clone()),
                    actor_ref: Some(actor),
                    purpose: Some(db::ExecutionPurpose::Implement),
                    harness_session_id: None,
                    role: "implementer".to_owned(),
                    status: ExecutionStatus::Running,
                    stop_reason: None,
                    stopped_by: None,
                    resume_policy: None,
                    stopped_at: None,
                    parent_execution_id: None,
                    agent_message_id: None,
                    last_activity_at: None,
                    summary: None,
                    logs_path: None,
                    before_sha: None,
                    after_sha: None,
                    error: None,
                    executor_config_snapshot_json: Some(
                        serde_json::json!({
                            "executor_type": agent.executor_type,
                            "config": {},
                            "agent_id": agent.id,
                            "agent_daemon_id": agent.daemon_id,
                            "resolved_daemon_id": resolved_daemon_id,
                            "harness_capabilities": {
                                "schema_version": 1,
                                "capabilities": { "resume": resume_support }
                            }
                        })
                        .to_string(),
                    ),
                    workspace_id: None,
                    created_at: now.clone(),
                    updated_at: now,
                },
                work_unit_id,
                work_unit_version: work_unit.version,
                workspace_lease: None,
            },
            db::CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "execution.started".to_owned(),
                entity_type: "execution".to_owned(),
                entity_id: execution_id.clone(),
                actor_type: "agent".to_owned(),
                actor_id: Some(agent_id),
                scope_type: "task".to_owned(),
                scope_id: work_unit.task_id.clone(),
                correlation_id: execution_id.clone(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("execution.started:{execution_id}")),
                payload_json: serde_json::json!({"work_unit_id": work_unit.id}).to_string(),
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("WorkUnit Execution creates")
        .record;
        if provider_started {
            assert_eq!(
                ExecutionRepo::claim_provider_start(
                    db,
                    &execution.id,
                    provider_start_receipt_for_test(
                        &execution,
                        "execution.provider_start_attempted",
                    ),
                )
                .await
                .expect("fixture records that provider Start was attempted"),
                db::ProviderStartClaim::Claimed
            );
            ExecutionRepo::record_provider_start_outcome(
                db,
                &execution.id,
                provider_start_receipt_for_test(&execution, "execution.provider_start_accepted"),
            )
            .await
            .expect("fixture records an accepted provider Start");
        }
        if let Some(external_session_id) = agent_session_id {
            execution = ExecutionRepo::record_harness_session_result(
                db,
                &execution.id,
                &external_session_id,
                &now_rfc3339(),
            )
            .await
            .expect("harness session activates");
        }
        execution
    }

    fn provider_start_receipt_for_test(
        execution: &Execution,
        event_type: &str,
    ) -> db::CreateDomainEvent {
        let actor = execution.actor_ref();
        db::CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: event_type.to_owned(),
            entity_type: "execution".to_owned(),
            entity_id: execution.id.clone(),
            actor_type: actor
                .as_ref()
                .map(|actor| actor.kind().to_string())
                .unwrap_or_else(|| "system".to_owned()),
            actor_id: actor.as_ref().map(|actor| actor.id().to_owned()),
            scope_type: "task".to_owned(),
            scope_id: execution.task_id.clone(),
            correlation_id: execution.id.clone(),
            causation_id: execution.parent_execution_id.clone(),
            causation_depth: i64::from(execution.parent_execution_id.is_some()),
            dedupe_key: Some(format!("{event_type}:{}", execution.id)),
            payload_json: serde_json::json!({
                "execution_id": execution.id,
                "task_id": execution.task_id,
                "agent_id": execution.agent_id,
                "outcome": event_type.strip_prefix("execution.provider_start_").unwrap_or_default(),
            })
            .to_string(),
            created_at: now_rfc3339(),
        }
    }

    struct ParallelRecoveryFixture {
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        task: Task,
        agents: Vec<Agent>,
        executions: Vec<db::Execution>,
    }

    async fn seed_parallel_recovery_fixture(
        harnesses: &[&str],
        sessions: &[Option<&str>],
    ) -> ParallelRecoveryFixture {
        let db = Arc::new(sqlite_db().await);
        seed_parallel_recovery_fixture_with_db(harnesses, sessions, db).await
    }

    async fn seed_parallel_recovery_fixture_with_db(
        harnesses: &[&str],
        sessions: &[Option<&str>],
        db: Arc<SqliteDb>,
    ) -> ParallelRecoveryFixture {
        assert!(harnesses.len() >= 2);
        assert_eq!(harnesses.len(), sessions.len());
        let event_bus = Arc::new(EventBus::new(64));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let mut agents = Vec::with_capacity(harnesses.len());
        for harness in harnesses {
            agents.push(
                seed_agent_with_harness(&db, AgentStatus::Busy, Some(now_rfc3339()), harness).await,
            );
        }
        let embedded_machine_id = crate::embedded_daemon::embedded_machine_id();
        let existing_embedded_daemon_id: Option<String> =
            sqlx::query_scalar("SELECT id FROM daemon WHERE machine_id = ?")
                .bind(&embedded_machine_id)
                .fetch_optional(db.pool())
                .await
                .expect("embedded fixture daemon lookup succeeds");
        let embedded_daemon_id = if let Some(daemon_id) = existing_embedded_daemon_id {
            daemon_id
        } else {
            let daemon_id = agents[0].daemon_id.clone().expect("fixture Agent daemon");
            sqlx::query("UPDATE daemon SET machine_id = ? WHERE id = ?")
                .bind(&embedded_machine_id)
                .bind(&daemon_id)
                .execute(db.pool())
                .await
                .expect("fixture daemon becomes the embedded execution host");
            daemon_id
        };
        let detected_clis_json = serde_json::to_string(
            &harnesses
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|kind| json!({"kind": kind, "availability": "authenticated"}))
                .collect::<Vec<_>>(),
        )
        .expect("fixture harness discovery serializes");
        DaemonRepo::update_report(
            &*db,
            UpdateDaemonReport {
                id: embedded_daemon_id.clone(),
                detected_clis_json,
                labels_json: None,
                status: DaemonStatus::Online,
                last_report_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("embedded fixture daemon advertises every authenticated harness");
        for agent in &mut agents {
            if agent.daemon_id.as_deref() == Some(embedded_daemon_id.as_str()) {
                continue;
            }
            *agent = AgentRepo::update(
                &*db,
                UpdateAgent {
                    id: agent.id.clone(),
                    expected_version: agent.version,
                    name: None,
                    description: None,
                    model: None,
                    reasoning_effort: None,
                    permission_policy: None,
                    prompt_template: None,
                    capabilities_json: None,
                    config_json: None,
                    daemon_id: Some(Some(embedded_daemon_id.clone())),
                    max_concurrent_tasks: None,
                    heartbeat_interval_seconds: None,
                    max_missed_heartbeats: None,
                    status: None,
                    last_heartbeat_at: None,
                    is_default: None,
                    paused: None,
                    updated_at: now_rfc3339(),
                },
            )
            .await
            .expect("recovery fixture Agent routes to the embedded execution host");
        }
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agents[0].id.clone()),
        )
        .await;
        let role = TaskRoleRepo::get_by_task_and_role(&*db, &task.id, "implementer")
            .await
            .expect("TaskRole loads")
            .expect("TaskRole exists");
        TaskRoleRepo::update(
            &*db,
            db::UpdateTaskRole {
                id: role.id.clone(),
                expected_version: role.version,
                coordination_mode: Some(Some(db::CoordinationMode::Collaborative)),
                policy_json: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("TaskRole allows multiple independent Actors");
        for agent in agents.iter().skip(1) {
            RoleMembershipRepo::add(
                &*db,
                CreateRoleMembership {
                    id: new_uuid_v4(),
                    task_role_id: role.id.clone(),
                    actor_kind: db::ActorKind::Agent,
                    actor_id: agent.id.clone(),
                    status: RoleMembershipStatus::Active,
                    created_at: now_rfc3339(),
                    updated_at: now_rfc3339(),
                },
            )
            .await
            .expect("each exact Actor has an active TaskRole membership");
        }
        let mut executions = Vec::with_capacity(agents.len());
        for (agent, session) in agents.iter().zip(sessions) {
            executions.push(
                seed_running_execution(
                    &db,
                    task.id.clone(),
                    agent.id.clone(),
                    session.map(str::to_owned),
                )
                .await,
            );
        }
        ParallelRecoveryFixture {
            db,
            event_bus,
            task,
            agents,
            executions,
        }
    }

    #[tokio::test]
    async fn crash_recovery_ignores_idle_in_progress_tasks() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let mut rx = event_bus.subscribe();
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent = seed_agent(&db, AgentStatus::Busy, Some(now_rfc3339())).await;
        let in_progress = seed_task(
            &db,
            project_id.clone(),
            repo_id.clone(),
            "active".to_owned(),
            Some(agent.id),
        )
        .await;
        let todo = seed_task(&db, project_id, repo_id, "ready".to_owned(), None).await;

        let recovery = CrashRecovery::new(Arc::clone(&db), event_bus);
        let recovered = recovery.run_recovery().await.expect("recovery runs");
        assert_eq!(recovered, 0);

        let recovered_task = TaskRepo::get_by_id(&*db, &in_progress.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(recovered_task.lifecycle_state, TaskLifecycleState::Active);
        assert_eq!(recovered_task.error_annotation, None);
        let implementer = TaskRoleRepo::get_by_task_and_role(&*db, &in_progress.id, "implementer")
            .await
            .expect("implementer TaskRole loads")
            .expect("implementer TaskRole exists");
        let members = RoleMembershipRepo::list_by_role(&*db, &implementer.id, false)
            .await
            .expect("RoleMemberships load");
        assert!(members.iter().any(|member| {
            member.actor_kind == db::ActorKind::Agent
                && member.status == RoleMembershipStatus::Active
        }));

        let unchanged = TaskRepo::get_by_id(&*db, &todo.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(unchanged.error_annotation, None);

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn crash_recovery_skips_user_assigned_in_progress_tasks() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let mut rx = event_bus.subscribe();
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let user_task = seed_task_with_assignee(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            "user",
            Some("human-user".to_owned()),
        )
        .await;

        let recovery = CrashRecovery::new(Arc::clone(&db), event_bus);
        let recovered = recovery.run_recovery().await.expect("recovery runs");
        assert_eq!(recovered, 0);

        let unchanged = TaskRepo::get_by_id(&*db, &user_task.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(unchanged.lifecycle_state, TaskLifecycleState::Active);
        assert_eq!(unchanged.error_annotation, None);

        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn crash_recovery_keeps_historical_ambiguous_lifecycle_blocked() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let task = seed_task(&db, project_id, repo_id, "active".to_owned(), None).await;
        crate::task_lifecycle::TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .block(
                &task.id,
                crate::task_lifecycle::LifecycleCause::System(
                    api_types::SystemComponent::TaskDispatcher,
                ),
                "ambiguous_review",
                task.id.clone(),
                format!("historical-ambiguity:{}", task.id),
            )
            .await
            .expect("historical ambiguity is recorded with an exact lifecycle receipt");

        let recovered = CrashRecovery::new(Arc::clone(&db), event_bus)
            .run_recovery()
            .await
            .expect("recovery runs");

        assert_eq!(recovered, 0);
        let recovered_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(recovered_task.lifecycle_state, TaskLifecycleState::Blocked);
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("lifecycle loads")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Blocked);
        assert_eq!(lifecycle.reason_kind.as_deref(), Some("ambiguous_review"));
    }

    #[tokio::test]
    async fn heartbeat_monitor_times_out_busy_agents_and_recovers_tasks() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let mut rx = event_bus.subscribe();
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let old_heartbeat = "1970-01-01T00:00:00+00:00".to_owned();
        let agent = seed_agent(&db, AgentStatus::Busy, Some(old_heartbeat.clone())).await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent.id.clone()),
        )
        .await;
        let _execution = seed_running_execution(&db, task.id.clone(), agent.id.clone(), None).await;

        let monitor = HeartbeatMonitor::with_check_interval(
            Arc::clone(&db),
            event_bus,
            Duration::from_millis(5),
        );
        let timed_out = monitor.check_once().await.expect("monitor runs");
        assert_eq!(timed_out, 1);

        let updated_agent = AgentRepo::get_by_id(&*db, &agent.id)
            .await
            .expect("agent loads")
            .expect("agent exists");
        assert_eq!(updated_agent.status, AgentStatus::Error);

        let recovered_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(recovered_task.lifecycle_state, TaskLifecycleState::Blocked);
        let implementer = TaskRoleRepo::get_by_task_and_role(&*db, &task.id, "implementer")
            .await
            .expect("implementer role loads")
            .expect("implementer role exists");
        let membership = RoleMembershipRepo::list_by_role(&*db, &implementer.id, true)
            .await
            .expect("membership history loads")
            .into_iter()
            .find(|member| member.status == RoleMembershipStatus::Active)
            .expect("the exact active membership remains");
        assert_eq!(membership.actor_id, agent.id);
        let annotation: Value =
            serde_json::from_str(recovered_task.error_annotation.as_deref().unwrap()).unwrap();
        assert_eq!(annotation["type"], "recovery_required");
        assert_eq!(annotation["blocking_reason"], "agent_timeout");
        assert_eq!(annotation["blocked_by"], "system:heartbeat_monitor");
        assert_eq!(
            annotation["message"],
            "Recovered after agent heartbeat timeout"
        );
        assert_eq!(
            annotation["recovery_actions"],
            json!(["reexecute", "cancel_task"]),
            "an exact WorkUnit target can be recovered or cancelled; reset-to-initial has no exact V2 authority"
        );

        let mut event_types = Vec::new();
        let mut recovered_event_id = None;
        while recovered_event_id.is_none() {
            let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("recovery event arrives")
                .expect("recovery event receives");
            if event.event_type == "task.recovered" {
                recovered_event_id = Some(event.entity_id);
            }
            event_types.push(event.event_type);
        }
        assert!(event_types.iter().any(|event| event == "agent.timeout"));
        assert!(event_types
            .iter()
            .any(|event| event == "domain_event.committed"));
        assert!(event_types.iter().any(|event| event == "task.recovered"));
        assert_eq!(recovered_event_id.as_deref(), Some(task.id.as_str()));
    }

    #[tokio::test]
    async fn rotated_logs_keep_execution_alive_but_silence_still_stalls() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, _) = seed_agent_with_daemon(
            &db,
            &crate::embedded_daemon::embedded_machine_id(),
            AgentStatus::Idle,
        )
        .await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent_id, None).await;
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut writer = executors::LogWriter::new(
            dir.path().join("execution.jsonl"),
            execution.id.clone(),
            500,
        );
        writer.set_log_sender(tx);
        writer
            .write(
                executors::LogKind::Assistant,
                executors::LogStream::Main,
                json!({"text": "long output ".repeat(100)}),
            )
            .await
            .unwrap();
        rx.recv().await.unwrap();
        ExecutionRepo::update_last_activity_at(&*db, &execution.id, "1970-01-01T00:00:00+00:00")
            .await
            .unwrap();
        writer
            .write(
                executors::LogKind::SessionInfo,
                executors::LogStream::Heartbeat,
                json!({"type": "codex_turn_heartbeat"}),
            )
            .await
            .unwrap();
        let heartbeat = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(heartbeat.stream, executors::LogStream::Heartbeat);
        ExecutionRepo::update_last_activity_at(&*db, &execution.id, &heartbeat.timestamp)
            .await
            .unwrap();
        let monitor = HeartbeatMonitor::new(Arc::clone(&db), event_bus);
        assert_eq!(monitor.check_stalled_executions().await.unwrap(), 0);
        ExecutionRepo::update_last_activity_at(&*db, &execution.id, "1970-01-01T00:00:00+00:00")
            .await
            .unwrap();
        assert_eq!(monitor.check_stalled_executions().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn heartbeat_monitor_marks_stalled_executions_without_legacy_retry_authority() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let mut rx = event_bus.subscribe();
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, _) = seed_agent_with_daemon(
            &db,
            &crate::embedded_daemon::embedded_machine_id(),
            AgentStatus::Idle,
        )
        .await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent_id, None).await;
        ExecutionRepo::update(
            &*db,
            UpdateExecution {
                id: execution.id.clone(),
                status: None,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: Some(Some("1970-01-01T00:00:00+00:00".to_owned())),
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("execution activity updates");

        let executor = Arc::new(RecordingCancelExecutor::default());
        let task_service = Arc::new(
            TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
                .with_task_executor(executor.clone()),
        );
        let monitor = HeartbeatMonitor::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_task_service(task_service)
            .with_task_executor(executor.clone())
            .with_execution_stall_timeout(Duration::from_secs(1));

        let stalled = monitor.check_once().await.expect("monitor checks");

        assert_eq!(stalled, 1);
        assert_eq!(
            executor
                .cancelled
                .lock()
                .expect("cancel log lock")
                .as_slice(),
            std::slice::from_ref(&execution.id)
        );
        let updated_execution = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(updated_execution.status, ExecutionStatus::Failed);
        assert_eq!(
            updated_execution.stop_reason,
            Some(StopReason::ExecutionStalled)
        );
        assert_eq!(updated_execution.resume_policy, Some(ResumePolicy::Manual));

        let updated_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        let metadata = updated_task
            .metadata_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or(Value::Null);
        assert!(metadata.get("execution_retry_count").is_none());
        let mut event_types = Vec::new();
        for _ in 0..3 {
            event_types.push(rx.recv().await.expect("event receives").event_type);
        }
        assert!(event_types.iter().any(|event| event == "execution.stalled"));
        assert!(!event_types
            .iter()
            .any(|event| event == "task.execution_retry"));
    }

    #[tokio::test]
    async fn heartbeat_monitor_start_stops() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let monitor = Arc::new(HeartbeatMonitor::with_check_interval(
            db,
            event_bus,
            Duration::from_millis(1),
        ));

        let handle = Arc::clone(&monitor).start();
        monitor.stop();
        handle.await.expect("monitor task joins");
        assert!(monitor.is_stopped());
    }

    #[tokio::test]
    async fn recovery_monitors_all_active_states() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent = seed_agent(
            &db,
            AgentStatus::Busy,
            Some("1970-01-01T00:00:00+00:00".to_owned()),
        )
        .await;

        let task_in_progress = seed_task(
            &db,
            project_id.clone(),
            repo_id.clone(),
            "active".to_owned(),
            Some(agent.id.clone()),
        )
        .await;
        let task_merge_failed = seed_task(
            &db,
            project_id,
            repo_id,
            "blocked".to_owned(),
            Some(agent.id),
        )
        .await;

        let recovery = CrashRecovery::new(Arc::clone(&db), event_bus);
        let recovered = recovery.run_recovery().await.expect("recovery runs");
        assert_eq!(recovered, 0);

        let t1 = TaskRepo::get_by_id(&*db, &task_in_progress.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(t1.lifecycle_state, TaskLifecycleState::Active);

        let t2 = TaskRepo::get_by_id(&*db, &task_merge_failed.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(t2.lifecycle_state, TaskLifecycleState::Blocked);
    }

    #[tokio::test]
    async fn crash_recovery_cancels_running_executions() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent = seed_agent(&db, AgentStatus::Busy, Some(now_rfc3339())).await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent.id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent.id, None).await;

        let recovery = CrashRecovery::new(Arc::clone(&db), event_bus);
        let recovered = recovery.run_recovery().await.expect("recovery runs");
        assert_eq!(recovered, 1);

        let updated_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(updated_task.lifecycle_state, TaskLifecycleState::Blocked);
        let annotation: Value =
            serde_json::from_str(updated_task.error_annotation.as_deref().unwrap()).unwrap();
        assert_eq!(annotation["blocked_execution_id"], execution.id);
        assert_eq!(annotation["artifact"]["kind"], "execution");
        assert_eq!(annotation["artifact"]["id"], execution.id);

        let updated = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(updated.status, ExecutionStatus::Cancelled);
        assert!(updated.error.as_deref().unwrap().contains("Recovered"));
        assert_eq!(updated.resume_policy, Some(ResumePolicy::Manual));
    }

    #[tokio::test]
    async fn crash_recovery_with_two_manual_work_units_can_be_resolved_by_exact_selection() {
        let fixture = seed_parallel_recovery_fixture(&["shell", "shell"], &[None, None]).await;
        let db = Arc::clone(&fixture.db);
        let event_bus = Arc::clone(&fixture.event_bus);
        let task = fixture.task.clone();
        let execution_a = fixture.executions[0].clone();
        let execution_b = fixture.executions[1].clone();
        let work_unit_a = execution_a
            .work_unit_id
            .clone()
            .expect("A has an exact WorkUnit");
        let work_unit_b = execution_b
            .work_unit_id
            .clone()
            .expect("B has an exact WorkUnit");
        assert_ne!(work_unit_a, work_unit_b);

        let recovered = CrashRecovery::new(Arc::clone(&db), Arc::clone(&event_bus))
            .run_recovery()
            .await
            .expect("CrashRecovery runs");
        assert_eq!(recovered, 1);

        let stopped_a = ExecutionRepo::get_by_id(&*db, &execution_a.id)
            .await
            .expect("A reloads")
            .expect("A remains historical");
        let stopped_b = ExecutionRepo::get_by_id(&*db, &execution_b.id)
            .await
            .expect("B reloads")
            .expect("B remains historical");
        for (execution, work_unit_id) in [
            (&stopped_a, work_unit_a.as_str()),
            (&stopped_b, work_unit_b.as_str()),
        ] {
            assert_eq!(execution.status, ExecutionStatus::Cancelled);
            assert_eq!(execution.stop_reason, Some(StopReason::CrashRecovery));
            assert_eq!(execution.resume_policy, Some(ResumePolicy::Manual));
            assert_eq!(execution.work_unit_id.as_deref(), Some(work_unit_id));
            assert_eq!(execution.task_id, task.id);
        }

        let active_leases: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workspace_lease
             WHERE execution_id IN (?, ?) AND status = 'active'",
        )
        .bind(&execution_a.id)
        .bind(&execution_b.id)
        .fetch_one(db.pool())
        .await
        .expect("lease state loads");
        assert_eq!(
            active_leases, 0,
            "cancelled attempts retain no active lease"
        );

        let blocked_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("Task reloads")
            .expect("Task remains");
        let annotation: Value = serde_json::from_str(
            blocked_task
                .error_annotation
                .as_deref()
                .expect("aggregate recovery annotation persists"),
        )
        .expect("annotation is valid JSON");
        assert_eq!(annotation["blocking_reason"], "recovery_target_ambiguous");
        assert!(annotation["blocked_execution_id"].is_null());
        assert_eq!(annotation["recovery_actions"], serde_json::json!([]));
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists");
        assert_eq!(lifecycle.state, TaskLifecycleState::Blocked);
        assert_eq!(
            lifecycle.reason_kind.as_deref(),
            Some("recovery_target_ambiguous")
        );
        assert!(lifecycle
            .reason_ref
            .as_deref()
            .unwrap()
            .starts_with("task-version:"));

        let transition_count_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_lifecycle_transition WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .expect("lifecycle receipts load");
        let execution_count_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .expect("Execution count loads");
        let event_count_before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event WHERE scope_type = 'task' AND scope_id = ?",
        )
        .bind(&task.id)
        .fetch_one(db.pool())
        .await
        .expect("DomainEvent count loads");

        let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
        let task_level_recovery = service
            .recover_task(
                task.id.clone(),
                api_types::RecoveryAction::Reexecute,
                None,
                None,
            )
            .await
            .expect_err("Task-level recovery must not infer A or B");
        assert!(task_level_recovery
            .to_string()
            .contains("no blocking recovery annotation"));

        for parent in [&execution_a, &execution_b] {
            let individual_recovery = service
                .re_execute_execution_with_context(parent.id.clone(), None)
                .await
                .err()
                .expect("exact parent alone cannot resolve an ambiguous aggregate block");
            assert!(individual_recovery
                .to_string()
                .contains("exact current Execution block"));
        }

        let final_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("Task reloads after rejected requests")
            .expect("Task remains");
        assert_eq!(final_task.lifecycle_state, TaskLifecycleState::Blocked);
        assert_eq!(final_task.error_annotation, blocked_task.error_annotation);
        let transition_count_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_lifecycle_transition WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .expect("lifecycle receipts reload");
        let execution_count_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .expect("Execution count reloads");
        let event_count_after: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event WHERE scope_type = 'task' AND scope_id = ?",
        )
        .bind(&task.id)
        .fetch_one(db.pool())
        .await
        .expect("DomainEvent count reloads");
        assert_eq!(transition_count_after, transition_count_before);
        assert_eq!(execution_count_after, execution_count_before);
        assert_eq!(event_count_after, event_count_before);

        let operator_id = "recovery-operator";
        let request_a = api_types::RecoverWorkUnitExecutionRequest {
            work_unit_id: work_unit_a.clone(),
            expected_task_version: final_task.version,
            expected_lifecycle_version: lifecycle.version,
            expected_work_unit_version: WorkUnitRepo::get_by_id(&*db, &work_unit_a)
                .await
                .expect("A WorkUnit loads")
                .expect("A WorkUnit exists")
                .version,
            idempotency_key: "recover-a-once".to_owned(),
            context: Some("resume the exact A scope".to_owned()),
        };
        let child_a = service
            .recover_work_unit_execution(
                task.id.clone(),
                execution_a.id.clone(),
                request_a.clone(),
                operator_id,
            )
            .await
            .expect("explicit A selection resolves and admits atomically");
        assert!(!child_a.idempotent_replay);
        assert_eq!(
            child_a.execution.parent_execution_id.as_deref(),
            Some(execution_a.id.as_str())
        );
        assert_eq!(
            child_a.execution.work_unit_id.as_deref(),
            Some(work_unit_a.as_str())
        );
        assert_eq!(child_a.task.lifecycle_state, TaskLifecycleState::Active);
        assert!(child_a.task.error_annotation.is_none());
        assert!(child_a.task.blocked_json.is_none());

        let receipt_a: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'execution.recovery_admitted'
               AND entity_id = ? AND actor_type = 'human' AND actor_id = ?",
        )
        .bind(&child_a.execution.id)
        .bind(operator_id)
        .fetch_one(db.pool())
        .await
        .expect("A human decision receipt loads");
        assert_eq!(receipt_a, 1);
        let child_a_replay = service
            .recover_work_unit_execution(
                task.id.clone(),
                execution_a.id.clone(),
                request_a,
                operator_id,
            )
            .await
            .expect("same-key retry reads the durable decision");
        assert!(child_a_replay.idempotent_replay);
        assert_eq!(child_a_replay.execution.id, child_a.execution.id);

        let task_after_a = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("Task reloads after A")
            .expect("Task remains");
        let lifecycle_after_a = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("lifecycle reloads after A")
            .expect("lifecycle remains");
        let work_unit_a_current = WorkUnitRepo::get_by_id(&*db, &work_unit_a)
            .await
            .expect("A WorkUnit reloads")
            .expect("A WorkUnit remains");
        let child_agent = AgentRepo::get_by_id(
            &*db,
            child_a
                .execution
                .agent_id
                .as_deref()
                .expect("A child retains its exact Agent"),
        )
        .await
        .expect("child Agent loads")
        .expect("child Agent remains");
        AgentRepo::update(
            &*db,
            UpdateAgent {
                id: child_agent.id.clone(),
                expected_version: child_agent.version,
                name: None,
                description: None,
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: None,
                config_json: None,
                daemon_id: None,
                max_concurrent_tasks: Some(2),
                heartbeat_interval_seconds: None,
                max_missed_heartbeats: None,
                status: None,
                last_heartbeat_at: None,
                is_default: None,
                paused: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("test Agent capacity allows the durable duplicate guard to decide");
        assert!(
            service
                .recover_work_unit_execution(
                    task.id.clone(),
                    execution_a.id.clone(),
                    api_types::RecoverWorkUnitExecutionRequest {
                        work_unit_id: work_unit_a.clone(),
                        expected_task_version: task_after_a.version,
                        expected_lifecycle_version: lifecycle_after_a.version,
                        expected_work_unit_version: work_unit_a_current.version,
                        idempotency_key: "recover-a-second-child-is-forbidden".to_owned(),
                        context: None,
                    },
                    operator_id,
                )
                .await
                .is_err(),
            "a different key cannot create another child from the consumed parent"
        );

        let current_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("Task reloads before selecting B")
            .expect("Task remains");
        let current_lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("lifecycle reloads before selecting B")
            .expect("lifecycle remains");
        let current_work_unit_b = WorkUnitRepo::get_by_id(&*db, &work_unit_b)
            .await
            .expect("B WorkUnit reloads")
            .expect("B WorkUnit remains");
        let child_b = service
            .recover_work_unit_execution(
                task.id.clone(),
                execution_b.id.clone(),
                api_types::RecoverWorkUnitExecutionRequest {
                    work_unit_id: work_unit_b.clone(),
                    expected_task_version: current_task.version,
                    expected_lifecycle_version: current_lifecycle.version,
                    expected_work_unit_version: current_work_unit_b.version,
                    idempotency_key: "recover-b-once".to_owned(),
                    context: None,
                },
                operator_id,
            )
            .await
            .expect("B remains independently recoverable after A");
        assert!(!child_b.idempotent_replay);
        assert_eq!(
            child_b.execution.parent_execution_id.as_deref(),
            Some(execution_b.id.as_str())
        );
        assert_eq!(
            child_b.execution.work_unit_id.as_deref(),
            Some(work_unit_b.as_str())
        );

        let final_lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("final lifecycle loads")
            .expect("final lifecycle remains");
        assert_eq!(final_lifecycle.state, TaskLifecycleState::Active);
        assert_eq!(final_lifecycle.version, lifecycle.version + 1);
        let final_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("final Task loads")
            .expect("final Task remains");
        assert!(final_task.error_annotation.is_none());
        assert!(final_task.blocked_json.is_none());
        for (parent, child, work_unit_id) in [
            (&stopped_a, &child_a.execution, work_unit_a.as_str()),
            (&stopped_b, &child_b.execution, work_unit_b.as_str()),
        ] {
            let persisted_parent = ExecutionRepo::get_by_id(&*db, &parent.id)
                .await
                .expect("cancelled parent reloads")
                .expect("cancelled parent remains");
            assert_eq!(persisted_parent.status, ExecutionStatus::Cancelled);
            assert_eq!(persisted_parent.work_unit_id.as_deref(), Some(work_unit_id));
            assert_eq!(
                child.parent_execution_id.as_deref(),
                Some(parent.id.as_str())
            );
            assert_eq!(child.work_unit_id.as_deref(), Some(work_unit_id));
            let child_started: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM domain_event
                 WHERE event_type = 'execution.started' AND entity_id = ?
                   AND scope_type = 'task' AND scope_id = ?",
            )
            .bind(&child.id)
            .bind(&task.id)
            .fetch_one(db.pool())
            .await
            .expect("child start receipt loads");
            assert_eq!(child_started, 1);
        }
        let recovery_receipt_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'execution.recovery_admitted' AND scope_id = ?",
        )
        .bind(&task.id)
        .fetch_one(db.pool())
        .await
        .expect("recovery decision receipts load");
        assert_eq!(recovery_receipt_count, 2);
        let execution_count_final: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .expect("all attempts remain durable");
        assert_eq!(execution_count_final, execution_count_before + 2);
        let transition_count_final: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_lifecycle_transition WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .expect("lifecycle receipt count reloads");
        assert_eq!(transition_count_final, transition_count_before + 1);
    }

    #[tokio::test]
    async fn explicit_recovery_same_key_after_post_commit_crash_replays_without_duplicate() {
        let database_dir = tempfile::TempDir::new().expect("file-backed recovery database dir");
        let database_url = format!(
            "sqlite://{}",
            database_dir.path().join("recovery.db").display()
        );
        let pool = db::create_sqlite_pool(&database_url)
            .await
            .expect("file-backed recovery database opens");
        db::run_migrations(&pool)
            .await
            .expect("current migrations apply");
        let db = Arc::new(SqliteDb::new(pool));
        let fixture =
            seed_parallel_recovery_fixture_with_db(&["shell", "shell"], &[None, None], db).await;
        let db = Arc::clone(&fixture.db);
        let event_bus = Arc::clone(&fixture.event_bus);
        let task = fixture.task.clone();
        let parent = fixture.executions[0].clone();
        let work_unit_id = parent
            .work_unit_id
            .clone()
            .expect("parent A has an exact WorkUnit");

        CrashRecovery::new(Arc::clone(&db), Arc::clone(&event_bus))
            .run_recovery()
            .await
            .expect("initial crash recovery records the aggregate ambiguity");
        let blocked_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("Task reloads")
            .expect("Task remains");
        let blocked_lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("lifecycle loads")
            .expect("lifecycle remains");
        let work_unit = WorkUnitRepo::get_by_id(&*db, &work_unit_id)
            .await
            .expect("A WorkUnit loads")
            .expect("A WorkUnit remains");
        let request = api_types::RecoverWorkUnitExecutionRequest {
            work_unit_id: work_unit_id.clone(),
            expected_task_version: blocked_task.version,
            expected_lifecycle_version: blocked_lifecycle.version,
            expected_work_unit_version: work_unit.version,
            idempotency_key: "post-commit-crash-decision".to_owned(),
            context: Some("operator selected A".to_owned()),
        };
        let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
        let committed = service
            .recover_work_unit_execution(
                task.id.clone(),
                parent.id.clone(),
                request.clone(),
                "recovery-operator",
            )
            .await
            .expect("human decision and child commit before provider dispatch");
        assert!(!committed.idempotent_replay);

        let children_before_restart: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution
             WHERE task_id = ? AND parent_execution_id IS NOT NULL",
        )
        .bind(&task.id)
        .fetch_one(db.pool())
        .await
        .expect("child count loads before restart");
        assert_eq!(children_before_restart, 1);

        drop(service);
        db.pool().close().await;
        let reopened_pool = db::create_sqlite_pool(&database_url)
            .await
            .expect("recovery database reopens after simulated process exit");
        db::run_migrations(&reopened_pool)
            .await
            .expect("current migrations remain safe on restart");
        let db = Arc::new(SqliteDb::new(reopened_pool));

        assert_eq!(
            CrashRecovery::new(Arc::clone(&db), Arc::clone(&event_bus))
                .run_recovery()
                .await
                .expect("startup recovery preserves the unattempted committed child"),
            0,
            "a pending provider Start is not misclassified as an interrupted provider run"
        );
        let reconciled_child = ExecutionRepo::get_by_id(&*db, &committed.execution.id)
            .await
            .expect("child reloads after restart")
            .expect("child remains in history");
        assert_eq!(reconciled_child.status, ExecutionStatus::Running);
        assert_eq!(reconciled_child.stop_reason, None);
        let reconciled_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("Task reloads after restart")
            .expect("Task remains");
        assert_eq!(reconciled_task.lifecycle_state, TaskLifecycleState::Active);
        assert!(reconciled_task.error_annotation.is_none());

        let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
        let replay = service
            .recover_work_unit_execution(
                task.id.clone(),
                parent.id.clone(),
                request,
                "recovery-operator",
            )
            .await
            .expect("same-key retry reads the committed receipt after startup recovery");
        assert!(replay.idempotent_replay);
        assert_eq!(replay.execution.id, committed.execution.id);
        assert_eq!(replay.execution.status, ExecutionStatus::Running);
        let children_after_replay: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution
             WHERE task_id = ? AND parent_execution_id IS NOT NULL",
        )
        .bind(&task.id)
        .fetch_one(db.pool())
        .await
        .expect("child count reloads after retry");
        assert_eq!(children_after_replay, children_before_restart);
        let decision_receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'execution.recovery_admitted'
               AND scope_id = ? AND actor_id = 'recovery-operator'",
        )
        .bind(&task.id)
        .fetch_one(db.pool())
        .await
        .expect("human decision receipt count loads");
        assert_eq!(decision_receipts, 1);
    }

    #[tokio::test]
    async fn crash_recovery_mixes_auto_and_manual_policy_by_exact_work_unit() {
        let fixture =
            seed_parallel_recovery_fixture(&["codex", "shell"], &[Some("codex-session-a"), None])
                .await;
        let parent_a = &fixture.executions[0];
        let parent_b = &fixture.executions[1];
        let work_unit_a = parent_a.work_unit_id.as_deref().expect("A exact WorkUnit");
        let work_unit_b = parent_b.work_unit_id.as_deref().expect("B exact WorkUnit");

        let recovered = CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .run_recovery()
            .await
            .expect("mixed CrashRecovery policies complete");
        assert_eq!(recovered, 0, "neither Task is given an aggregate block");

        let stopped_a = ExecutionRepo::get_by_id(&*fixture.db, &parent_a.id)
            .await
            .expect("A reloads")
            .expect("A remains historical");
        let stopped_b = ExecutionRepo::get_by_id(&*fixture.db, &parent_b.id)
            .await
            .expect("B reloads")
            .expect("B remains historical");
        assert_eq!(stopped_a.status, ExecutionStatus::Cancelled);
        assert_eq!(stopped_a.resume_policy, Some(ResumePolicy::Auto));
        assert_eq!(stopped_a.work_unit_id.as_deref(), Some(work_unit_a));
        assert_eq!(stopped_b.status, ExecutionStatus::Cancelled);
        assert_eq!(stopped_b.resume_policy, Some(ResumePolicy::Manual));
        assert_eq!(stopped_b.work_unit_id.as_deref(), Some(work_unit_b));
        let automatic_candidates =
            ExecutionRepo::list_work_unit_auto_recovery_candidates(&*fixture.db)
                .await
                .expect("exact automatic candidates load");
        assert_eq!(
            automatic_candidates
                .iter()
                .map(|execution| execution.id.as_str())
                .collect::<Vec<_>>(),
            vec![parent_a.id.as_str()]
        );

        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task reloads")
            .expect("Task remains");
        assert_eq!(task.lifecycle_state, TaskLifecycleState::Active);
        assert!(task.error_annotation.is_none());
        assert!(task.blocked_json.is_none());
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle reloads")
            .expect("TaskLifecycle remains");
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        let (first, second) = tokio::join!(
            service.re_execute_automatic_recovery_execution(parent_a.id.clone()),
            service.re_execute_automatic_recovery_execution(parent_a.id.clone()),
        );
        let first = first.expect("first exact Auto admission resolves");
        let second = second.expect("competing exact Auto admission resolves");
        assert_ne!(first.idempotent_replay, second.idempotent_replay);
        let (auto_child, replay) = if first.idempotent_replay {
            (second, first)
        } else {
            (first, second)
        };
        assert_eq!(
            auto_child.execution.parent_execution_id.as_deref(),
            Some(parent_a.id.as_str())
        );
        assert_eq!(
            auto_child.execution.harness_session_id, parent_a.harness_session_id,
            "automatic recovery must retain the exact resumable session"
        );
        assert_eq!(
            auto_child.execution.work_unit_id.as_deref(),
            Some(work_unit_a)
        );
        let invocation = crate::task_service::execution::harness_invocation_for_execution(
            &fixture.db,
            &auto_child.execution,
            auto_child.execution.workspace_id.as_deref(),
        )
        .await
        .expect("automatic recovery resolves its exact HarnessSession invocation");
        assert_eq!(
            invocation,
            api_types::HarnessInvocation::Resume {
                external_session_id: "codex-session-a".to_owned()
            }
        );
        assert!(replay.idempotent_replay);
        assert_eq!(replay.execution.id, auto_child.execution.id);
        assert!(
            ExecutionRepo::list_work_unit_auto_recovery_candidates(&*fixture.db)
                .await
                .expect("remaining automatic candidates reload")
                .iter()
                .all(|candidate| candidate.id != parent_a.id),
            "a durable child consumes the automatic recovery candidate"
        );

        let work_unit = WorkUnitRepo::get_by_id(&*fixture.db, work_unit_b)
            .await
            .expect("B WorkUnit reloads")
            .expect("B WorkUnit remains");
        let manual_child = service
            .recover_work_unit_execution(
                task.id.clone(),
                parent_b.id.clone(),
                api_types::RecoverWorkUnitExecutionRequest {
                    work_unit_id: work_unit_b.to_owned(),
                    expected_task_version: task.version,
                    expected_lifecycle_version: lifecycle.version,
                    expected_work_unit_version: work_unit.version,
                    idempotency_key: "mixed-manual-b".to_owned(),
                    context: None,
                },
                "recovery-operator",
            )
            .await
            .expect("manual B remains independently selectable");
        assert_eq!(
            manual_child.execution.parent_execution_id.as_deref(),
            Some(parent_b.id.as_str())
        );
        assert_eq!(
            manual_child.execution.work_unit_id.as_deref(),
            Some(work_unit_b)
        );
        let final_task = TaskRepo::get_by_id(&*fixture.db, &task.id, false)
            .await
            .expect("final Task loads")
            .expect("final Task remains");
        assert_eq!(final_task.lifecycle_state, TaskLifecycleState::Active);
        assert!(final_task.error_annotation.is_none());
        assert!(final_task.blocked_json.is_none());
        assert_eq!(fixture.agents.len(), 2);
    }

    #[tokio::test]
    async fn paused_agent_keeps_auto_candidate_pending_until_unpaused() {
        let fixture =
            seed_parallel_recovery_fixture(&["codex", "shell"], &[Some("codex-session-a"), None])
                .await;
        CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .run_recovery()
            .await
            .expect("CrashRecovery records an exact resumable Auto candidate");

        let parent = ExecutionRepo::get_by_id(&*fixture.db, &fixture.executions[0].id)
            .await
            .expect("Auto parent reloads")
            .expect("Auto parent remains historical");
        assert_eq!(parent.resume_policy, Some(ResumePolicy::Auto));
        assert_eq!(parent.status, ExecutionStatus::Cancelled);

        let task_service =
            TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        let dispatcher = TaskDispatcher::new(
            Arc::clone(&fixture.db),
            Arc::clone(&fixture.event_bus),
            Arc::new(task_service.clone()),
        );

        AgentRepo::set_paused(&*fixture.db, &fixture.agents[0].id, true)
            .await
            .expect("Agent pauses");
        assert_eq!(
            dispatcher
                .check_once()
                .await
                .expect("dispatcher tick succeeds"),
            0,
            "a paused Agent must not admit or start an Auto child"
        );
        let child_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
                .bind(&parent.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("child count loads while paused");
        assert_eq!(child_count, 0);
        let still_pending = ExecutionRepo::list_work_unit_auto_recovery_candidates(&*fixture.db)
            .await
            .expect("paused Auto candidate remains queryable");
        assert!(still_pending
            .iter()
            .any(|candidate| candidate.id == parent.id));
        let parent_while_paused = ExecutionRepo::get_by_id(&*fixture.db, &parent.id)
            .await
            .expect("parent reloads while paused")
            .expect("parent remains historical");
        assert_eq!(parent_while_paused.status, parent.status);
        assert_eq!(parent_while_paused.stop_reason, parent.stop_reason);
        assert_eq!(parent_while_paused.resume_policy, parent.resume_policy);
        assert_eq!(
            parent_while_paused.harness_session_id,
            parent.harness_session_id
        );
        AgentRepo::set_paused(&*fixture.db, &fixture.agents[0].id, false)
            .await
            .expect("Agent resumes");
        let resumed_agent = AgentRepo::get_by_id(&*fixture.db, &fixture.agents[0].id)
            .await
            .expect("Agent reloads after resume")
            .expect("Agent remains");
        assert!(!resumed_agent.paused);
        assert!(crate::task_service::execution::has_exact_work_unit_target(
            &fixture.db,
            &parent.task_id,
            &parent
        )
        .await
        .expect("exact Auto WorkUnit target check succeeds"));
        assert_eq!(
            crate::task_service::resumable_external_session(
                &fixture.db,
                &parent,
                parent.agent_id.as_deref(),
                parent.workspace_id.as_deref(),
            )
            .await
            .expect("resumable HarnessSession preflight succeeds")
            .as_deref(),
            Some("codex-session-a")
        );
        let admitted = task_service
            .re_execute_automatic_recovery_execution(parent.id.clone())
            .await
            .expect("unpaused Agent admits one exact Auto child");
        assert!(!admitted.idempotent_replay);
        assert_eq!(
            admitted.execution.parent_execution_id.as_deref(),
            Some(parent.id.as_str())
        );
        let children: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
                .bind(&parent.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("child count loads after unpause");
        assert_eq!(children, 1);
        let parent_after_admission = ExecutionRepo::get_by_id(&*fixture.db, &parent.id)
            .await
            .expect("parent reloads after child admission")
            .expect("parent remains historical");
        assert_eq!(parent_after_admission.status, parent.status);
        assert_eq!(parent_after_admission.stop_reason, parent.stop_reason);
        assert_eq!(parent_after_admission.resume_policy, parent.resume_policy);
    }

    #[tokio::test]
    async fn automatic_recovery_rechecks_pause_after_preflight_before_commit() {
        let fixture =
            seed_parallel_recovery_fixture(&["codex", "shell"], &[Some("codex-session-a"), None])
                .await;
        CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .run_recovery()
            .await
            .expect("CrashRecovery records an exact resumable Auto candidate");
        let parent_id = fixture.executions[0].id.clone();
        let parent_before = ExecutionRepo::get_by_id(&*fixture.db, &parent_id)
            .await
            .expect("Auto parent reloads")
            .expect("Auto parent remains");
        let task_before = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task reloads")
            .expect("Task remains");
        let lifecycle_before =
            TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task.id)
                .await
                .expect("TaskLifecycle reloads")
                .expect("TaskLifecycle remains");

        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let task_service =
            TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
                .with_automatic_recovery_precommit_barrier(Arc::clone(&barrier));
        let recovery_service = task_service.clone();
        let recovery = tokio::spawn(async move {
            recovery_service
                .re_execute_automatic_recovery_execution(parent_id.clone())
                .await
        });

        barrier.wait().await;
        AgentRepo::set_paused(&*fixture.db, &fixture.agents[0].id, true)
            .await
            .expect("Agent pauses after service preflight");
        barrier.wait().await;

        let error = recovery
            .await
            .expect("automatic recovery task joins")
            .err()
            .expect("durable admission rejects the stale unpaused snapshot");
        assert!(matches!(
            error,
            ServiceError::Db(db::DbError::VersionConflict)
        ));
        let child_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
                .bind(&parent_before.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("child count remains zero after stale admission");
        assert_eq!(child_count, 0);
        let parent_after = ExecutionRepo::get_by_id(&*fixture.db, &parent_before.id)
            .await
            .expect("parent reloads after rejected admission")
            .expect("parent remains historical");
        assert_eq!(parent_after.status, parent_before.status);
        assert_eq!(parent_after.stop_reason, parent_before.stop_reason);
        assert_eq!(parent_after.resume_policy, parent_before.resume_policy);
        assert_eq!(
            parent_after.harness_session_id,
            parent_before.harness_session_id
        );
        let task_after = TaskRepo::get_by_id(&*fixture.db, &task_before.id, false)
            .await
            .expect("Task reloads after rejected admission")
            .expect("Task remains");
        let lifecycle_after = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task_before.id)
            .await
            .expect("TaskLifecycle reloads after rejected admission")
            .expect("TaskLifecycle remains");
        assert_eq!(task_after.version, task_before.version);
        assert_eq!(task_after.error_annotation, task_before.error_annotation);
        assert_eq!(task_after.blocked_json, task_before.blocked_json);
        assert_eq!(lifecycle_after, lifecycle_before);
        assert!(
            ExecutionRepo::list_work_unit_auto_recovery_candidates(&*fixture.db)
                .await
                .expect("Auto candidate remains after rejection")
                .iter()
                .any(|candidate| candidate.id == parent_before.id)
        );
    }

    #[tokio::test]
    async fn automatic_recovery_capacity_is_enforced_by_sqlite_admission() {
        let fixture =
            seed_parallel_recovery_fixture(&["codex", "shell"], &[Some("codex-session-a"), None])
                .await;
        CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .run_recovery()
            .await
            .expect("CrashRecovery records an exact resumable Auto candidate");
        let parent = fixture.executions[0].clone();
        let other_task = seed_task(
            &fixture.db,
            fixture.task.project_id.clone(),
            fixture
                .task
                .repo_id
                .clone()
                .expect("fixture Task has its Repo"),
            "active",
            Some(fixture.agents[0].id.clone()),
        )
        .await;
        let live_execution = seed_running_execution(
            &fixture.db,
            other_task.id,
            fixture.agents[0].id.clone(),
            None,
        )
        .await;
        assert_eq!(
            crate::agent_capacity::count_running_executions(&fixture.db, &fixture.agents[0].id)
                .await
                .expect("Agent running count loads"),
            1
        );

        let task_service =
            TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        let error = task_service
            .re_execute_automatic_recovery_execution(parent.id.clone())
            .await
            .err()
            .expect("SQLite rejects auto recovery at the Agent capacity limit");
        assert!(matches!(
            error,
            ServiceError::Db(db::DbError::AgentAtCapacity)
        ));
        let child_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
                .bind(&parent.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("capacity-rejected child count loads");
        assert_eq!(child_count, 0);
        let parent_after = ExecutionRepo::get_by_id(&*fixture.db, &parent.id)
            .await
            .expect("parent reloads after capacity rejection")
            .expect("parent remains historical");
        assert_eq!(parent_after.status, ExecutionStatus::Cancelled);
        assert_eq!(parent_after.resume_policy, Some(ResumePolicy::Auto));
        assert!(
            ExecutionRepo::list_work_unit_auto_recovery_candidates(&*fixture.db)
                .await
                .expect("capacity-rejected candidate reloads")
                .iter()
                .any(|candidate| candidate.id == parent.id)
        );
        assert_eq!(
            ExecutionRepo::get_by_id(&*fixture.db, &live_execution.id)
                .await
                .expect("live capacity holder reloads")
                .expect("live execution remains")
                .status,
            ExecutionStatus::Running
        );
    }

    #[tokio::test]
    async fn start_execution_rejects_paused_agent_before_provider_dispatch() {
        let fixture = seed_parallel_recovery_fixture(&["codex", "shell"], &[None, None]).await;
        let execution = fixture.executions[0].clone();
        let executor = Arc::new(RecoveryStartExecutor::default());
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .with_task_executor(executor.clone());
        AgentRepo::set_paused(&*fixture.db, &fixture.agents[0].id, true)
            .await
            .expect("Agent pauses before the provider boundary");

        let error = service
            .start_execution(execution.id.clone())
            .await
            .expect_err("paused Agent cannot reach the provider");
        assert!(matches!(
            error,
            ServiceError::AgentPaused { ref agent_id } if agent_id == &fixture.agents[0].id
        ));
        assert_eq!(executor.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            ExecutionRepo::get_by_id(&*fixture.db, &execution.id)
                .await
                .expect("Execution reloads after paused dispatch")
                .expect("Execution remains")
                .status,
            ExecutionStatus::Running,
            "the pause guard does not rewrite the historical/current Execution"
        );
    }

    #[tokio::test]
    async fn graceful_shutdown_without_reusable_session_requires_an_exact_human_choice() {
        let fixture = seed_parallel_recovery_fixture(&["shell", "shell"], &[None, None]).await;
        let cancelled = cancel_running_executions(
            &fixture.db,
            &fixture.task.id,
            StopReason::GracefulShutdown,
            &api_types::Actor::system(api_types::SystemComponent::GracefulShutdown),
            ResumePolicy::Auto,
        )
        .await
        .expect("graceful shutdown cancels exact running WorkUnit attempts");
        assert_eq!(cancelled.len(), 2);

        let parents = &fixture.executions;
        let work_unit_id = parents[0]
            .work_unit_id
            .as_deref()
            .expect("A has an exact WorkUnit");
        let stopped = ExecutionRepo::get_by_id(&*fixture.db, &parents[0].id)
            .await
            .expect("A reloads")
            .expect("A remains historical");
        assert_eq!(stopped.status, ExecutionStatus::Cancelled);
        assert_eq!(stopped.stop_reason, Some(StopReason::GracefulShutdown));
        assert_eq!(stopped.resume_policy, Some(ResumePolicy::Manual));
        assert!(stopped.harness_session_id.is_none());
        assert!(
            ExecutionRepo::list_work_unit_auto_recovery_candidates(&*fixture.db)
                .await
                .expect("Auto candidates load")
                .is_empty()
        );

        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task reloads")
            .expect("Task remains");
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle remains");
        let work_unit = WorkUnitRepo::get_by_id(&*fixture.db, work_unit_id)
            .await
            .expect("A WorkUnit loads")
            .expect("A WorkUnit remains");
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        let recovered = service
            .recover_work_unit_execution(
                task.id.clone(),
                stopped.id.clone(),
                api_types::RecoverWorkUnitExecutionRequest {
                    work_unit_id: work_unit_id.to_owned(),
                    expected_task_version: task.version,
                    expected_lifecycle_version: lifecycle.version,
                    expected_work_unit_version: work_unit.version,
                    idempotency_key: "graceful-exact-recovery".to_owned(),
                    context: None,
                },
                "recovery-operator",
            )
            .await
            .expect("Human can explicitly admit the exact non-resumable graceful stop");
        assert_eq!(
            recovered.execution.parent_execution_id.as_deref(),
            Some(stopped.id.as_str())
        );
        assert_eq!(
            recovered.execution.work_unit_id.as_deref(),
            Some(work_unit_id)
        );
        assert!(recovered.execution.harness_session_id.is_none());
        assert!(!recovered.idempotent_replay);

        let start_receipt: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'execution.started' AND entity_id = ?",
        )
        .bind(&recovered.execution.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("new child start receipt loads");
        let operator_receipt: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'execution.recovery_admitted' AND entity_id = ?",
        )
        .bind(&recovered.execution.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("Human decision receipt loads");
        assert_eq!(start_receipt, 1);
        assert_eq!(operator_receipt, 1);
    }

    #[tokio::test]
    async fn human_may_explicitly_choose_a_fresh_child_for_an_auto_candidate() {
        let fixture =
            seed_parallel_recovery_fixture(&["codex", "shell"], &[Some("codex-session-a"), None])
                .await;
        CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .run_recovery()
            .await
            .expect("CrashRecovery keeps independently recoverable candidates");
        let parent = &fixture.executions[0];
        let stopped_parent = ExecutionRepo::get_by_id(&*fixture.db, &parent.id)
            .await
            .expect("Auto parent reloads")
            .expect("Auto parent remains historical");
        assert_eq!(stopped_parent.resume_policy, Some(ResumePolicy::Auto));
        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task reloads")
            .expect("Task remains");
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle remains");
        let work_unit_id = parent.work_unit_id.as_deref().expect("A exact WorkUnit");
        let work_unit = WorkUnitRepo::get_by_id(&*fixture.db, work_unit_id)
            .await
            .expect("A WorkUnit loads")
            .expect("A WorkUnit exists");
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        let human_choice = service
            .recover_work_unit_execution(
                task.id.clone(),
                parent.id.clone(),
                api_types::RecoverWorkUnitExecutionRequest {
                    work_unit_id: work_unit.id.clone(),
                    expected_task_version: task.version,
                    expected_lifecycle_version: lifecycle.version,
                    expected_work_unit_version: work_unit.version,
                    idempotency_key: "human-fresh-choice-for-auto".to_owned(),
                    context: Some("operator chose a fresh attempt".to_owned()),
                },
                "recovery-operator",
            )
            .await
            .expect("exact Human decision may authorize a fresh child from Auto");
        assert!(!human_choice.idempotent_replay);
        assert_eq!(
            human_choice.execution.parent_execution_id.as_deref(),
            Some(parent.id.as_str())
        );
        assert_eq!(
            human_choice.execution.work_unit_id.as_deref(),
            Some(work_unit_id)
        );
        let fresh_session_id = human_choice
            .execution
            .harness_session_id
            .as_deref()
            .expect("fresh child receives its own pending HarnessSession");
        assert_ne!(
            Some(fresh_session_id),
            stopped_parent.harness_session_id.as_deref()
        );
        let external_session_id: Option<String> =
            sqlx::query_scalar("SELECT external_session_id FROM harness_session WHERE id = ?")
                .bind(fresh_session_id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("fresh HarnessSession loads");
        assert!(external_session_id.is_none());
        let invocation = crate::task_service::execution::harness_invocation_for_execution(
            &fixture.db,
            &human_choice.execution,
            human_choice.execution.workspace_id.as_deref(),
        )
        .await
        .expect("explicit fresh recovery resolves its new HarnessSession");
        assert_eq!(invocation, api_types::HarnessInvocation::Start);
        assert!(
            ExecutionRepo::list_work_unit_auto_recovery_candidates(&*fixture.db)
                .await
                .expect("Auto candidates reload after the explicit choice")
                .iter()
                .all(|candidate| candidate.id != parent.id)
        );
        let receipt: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'execution.recovery_admitted'
               AND entity_id = ? AND actor_type = 'human'
               AND actor_id = 'recovery-operator'",
        )
        .bind(&human_choice.execution.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("Human override receipt loads");
        assert_eq!(receipt, 1);
    }

    #[tokio::test]
    async fn three_work_unit_recovery_keeps_each_automatic_and_manual_identity() {
        let fixture = seed_parallel_recovery_fixture(
            &["codex", "shell", "shell"],
            &[Some("codex-session-a"), None, None],
        )
        .await;
        let parents = &fixture.executions;
        let work_units = parents
            .iter()
            .map(|execution| execution.work_unit_id.clone().expect("exact WorkUnit"))
            .collect::<Vec<_>>();
        assert_eq!(work_units.len(), 3);
        assert_ne!(work_units[0], work_units[1]);
        assert_ne!(work_units[0], work_units[2]);
        assert_ne!(work_units[1], work_units[2]);

        CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .run_recovery()
            .await
            .expect("three-target CrashRecovery completes");

        let current = parents
            .iter()
            .map(|parent| parent.id.as_str())
            .collect::<Vec<_>>();
        let stopped = ExecutionRepo::list_work_unit_auto_recovery_candidates(&*fixture.db)
            .await
            .expect("automatic candidate list loads");
        assert_eq!(
            stopped
                .iter()
                .map(|execution| execution.id.as_str())
                .collect::<Vec<_>>(),
            vec![current[0]],
            "only the exact resumable A session is automatic"
        );
        for (index, parent) in parents.iter().enumerate() {
            let persisted = ExecutionRepo::get_by_id(&*fixture.db, &parent.id)
                .await
                .expect("parent reloads")
                .expect("parent remains historical");
            assert_eq!(persisted.status, ExecutionStatus::Cancelled);
            assert_eq!(
                persisted.work_unit_id.as_deref(),
                Some(work_units[index].as_str())
            );
            assert_eq!(
                persisted.resume_policy,
                Some(if index == 0 {
                    ResumePolicy::Auto
                } else {
                    ResumePolicy::Manual
                })
            );
        }

        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task reloads")
            .expect("Task remains");
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle reloads")
            .expect("TaskLifecycle remains");
        assert_eq!(task.lifecycle_state, TaskLifecycleState::Active);
        assert!(task.error_annotation.is_none());
        assert!(task.blocked_json.is_none());

        let work_unit_b = WorkUnitRepo::get_by_id(&*fixture.db, &work_units[1])
            .await
            .expect("B WorkUnit reloads")
            .expect("B WorkUnit remains");
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        let child_b = service
            .recover_work_unit_execution(
                task.id.clone(),
                parents[1].id.clone(),
                api_types::RecoverWorkUnitExecutionRequest {
                    work_unit_id: work_units[1].clone(),
                    expected_task_version: task.version,
                    expected_lifecycle_version: lifecycle.version,
                    expected_work_unit_version: work_unit_b.version,
                    idempotency_key: "three-target-b-decision".to_owned(),
                    context: Some("operator selected WorkUnit B".to_owned()),
                },
                "recovery-operator",
            )
            .await
            .expect("B can be selected without consuming C");
        assert_eq!(
            child_b.execution.parent_execution_id.as_deref(),
            Some(parents[1].id.as_str())
        );
        assert_eq!(
            child_b.execution.work_unit_id.as_deref(),
            Some(work_units[1].as_str())
        );
        let still_pending_c = ExecutionRepo::get_by_id(&*fixture.db, &parents[2].id)
            .await
            .expect("C reloads after B choice")
            .expect("C remains historical");
        assert_eq!(still_pending_c.status, ExecutionStatus::Cancelled);
        assert_eq!(still_pending_c.resume_policy, Some(ResumePolicy::Manual));
        assert_eq!(
            still_pending_c.work_unit_id.as_deref(),
            Some(work_units[2].as_str())
        );
        assert!(ExecutionRepo::get_by_id(&*fixture.db, &parents[0].id)
            .await
            .expect("A remains readable")
            .is_some());
    }

    #[tokio::test]
    async fn explicit_recovery_rejects_wrong_task_work_unit_and_stale_versions() {
        let fixture = seed_parallel_recovery_fixture(&["shell", "shell"], &[None, None]).await;
        CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .run_recovery()
            .await
            .expect("CrashRecovery commits the ambiguous block");
        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task loads")
            .expect("Task exists");
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists");
        let work_unit_a = WorkUnitRepo::get_by_id(
            &*fixture.db,
            fixture.executions[0]
                .work_unit_id
                .as_deref()
                .expect("A exact WorkUnit"),
        )
        .await
        .expect("A WorkUnit loads")
        .expect("A WorkUnit exists");
        let work_unit_b = WorkUnitRepo::get_by_id(
            &*fixture.db,
            fixture.executions[1]
                .work_unit_id
                .as_deref()
                .expect("B exact WorkUnit"),
        )
        .await
        .expect("B WorkUnit loads")
        .expect("B WorkUnit exists");
        let foreign_task = seed_task(
            &fixture.db,
            task.project_id.clone(),
            task.repo_id.clone().expect("Task Repo remains exact"),
            "active".to_owned(),
            None,
        )
        .await;
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        let make_request = |unit: &db::WorkUnit, key: &str, task_version, lifecycle_version| {
            api_types::RecoverWorkUnitExecutionRequest {
                work_unit_id: unit.id.clone(),
                expected_task_version: task_version,
                expected_lifecycle_version: lifecycle_version,
                expected_work_unit_version: unit.version,
                idempotency_key: key.to_owned(),
                context: None,
            }
        };

        assert!(service
            .recover_work_unit_execution(
                foreign_task.id,
                fixture.executions[0].id.clone(),
                make_request(
                    &work_unit_a,
                    "foreign-task",
                    task.version,
                    lifecycle.version
                ),
                "recovery-operator",
            )
            .await
            .is_err());
        assert!(service
            .recover_work_unit_execution(
                task.id.clone(),
                fixture.executions[0].id.clone(),
                make_request(
                    &work_unit_b,
                    "wrong-work-unit",
                    task.version,
                    lifecycle.version
                ),
                "recovery-operator",
            )
            .await
            .is_err());
        assert!(service
            .recover_work_unit_execution(
                task.id.clone(),
                fixture.executions[0].id.clone(),
                make_request(
                    &work_unit_a,
                    "stale-task-version",
                    task.version - 1,
                    lifecycle.version
                ),
                "recovery-operator",
            )
            .await
            .is_err());
        assert!(service
            .recover_work_unit_execution(
                task.id.clone(),
                fixture.executions[0].id.clone(),
                make_request(
                    &work_unit_a,
                    "stale-lifecycle-version",
                    task.version,
                    lifecycle.version - 1
                ),
                "recovery-operator",
            )
            .await
            .is_err());

        let task_after = TaskRepo::get_by_id(&*fixture.db, &task.id, false)
            .await
            .expect("Task reloads after rejected choices")
            .expect("Task remains");
        assert_eq!(task_after.version, task.version);
        assert_eq!(task_after.lifecycle_state, TaskLifecycleState::Blocked);
        assert_eq!(task_after.error_annotation, task.error_annotation);
        let transition_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_lifecycle_transition WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("transition count loads");
        assert_eq!(transition_count, 1);
        let child_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution WHERE task_id = ? AND parent_execution_id IS NOT NULL",
        )
        .bind(&task.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("child count loads");
        assert_eq!(child_count, 0);
        let receipt_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'execution.recovery_admitted' AND scope_id = ?",
        )
        .bind(&task.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("decision receipt count loads");
        assert_eq!(receipt_count, 0);
    }

    #[tokio::test]
    async fn explicit_recovery_rejects_suspended_work_unit_actor_membership() {
        let fixture = seed_parallel_recovery_fixture(&["shell", "shell"], &[None, None]).await;
        CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .run_recovery()
            .await
            .expect("CrashRecovery commits the ambiguous block");
        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task loads")
            .expect("Task exists");
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists");
        let parent = &fixture.executions[0];
        let work_unit_id = parent.work_unit_id.as_deref().expect("A exact WorkUnit");
        let work_unit = WorkUnitRepo::get_by_id(&*fixture.db, work_unit_id)
            .await
            .expect("A WorkUnit loads")
            .expect("A WorkUnit exists");
        let role = TaskRoleRepo::get_by_task_and_role(&*fixture.db, &task.id, &work_unit.role)
            .await
            .expect("TaskRole loads")
            .expect("TaskRole exists");
        let membership = RoleMembershipRepo::list_by_role(&*fixture.db, &role.id, true)
            .await
            .expect("membership history loads")
            .into_iter()
            .find(|membership| {
                membership.actor_kind == db::ActorKind::Agent
                    && membership.actor_id == parent.agent_id.as_deref().unwrap_or_default()
                    && membership.status == RoleMembershipStatus::Active
            })
            .expect("A parent Agent has its current active membership");
        RoleMembershipRepo::update(
            &*fixture.db,
            UpdateRoleMembership {
                id: membership.id,
                expected_version: membership.version,
                status: RoleMembershipStatus::Suspended,
                updated_at: now_rfc3339(),
                ended_at: None,
            },
        )
        .await
        .expect("A Agent membership is suspended before admission");

        let task_before = TaskRepo::get_by_id(&*fixture.db, &task.id, false)
            .await
            .expect("Task reloads")
            .expect("Task remains");
        let execution_count_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("Execution count loads");
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        assert!(service
            .recover_work_unit_execution(
                task.id.clone(),
                parent.id.clone(),
                api_types::RecoverWorkUnitExecutionRequest {
                    work_unit_id: work_unit.id.clone(),
                    expected_task_version: task_before.version,
                    expected_lifecycle_version: lifecycle.version,
                    expected_work_unit_version: work_unit.version,
                    idempotency_key: "suspended-recovery-actor".to_owned(),
                    context: None,
                },
                "recovery-operator",
            )
            .await
            .is_err());
        let task_after = TaskRepo::get_by_id(&*fixture.db, &task.id, false)
            .await
            .expect("Task reloads after rejected selection")
            .expect("Task remains");
        assert_eq!(task_after.version, task_before.version);
        assert_eq!(task_after.lifecycle_state, TaskLifecycleState::Blocked);
        assert_eq!(task_after.error_annotation, task.error_annotation);
        let execution_count_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("Execution count reloads");
        assert_eq!(execution_count_after, execution_count_before);
        let decision_receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'execution.recovery_admitted' AND scope_id = ?",
        )
        .bind(&task.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("decision receipts load");
        assert_eq!(decision_receipts, 0);
    }

    #[tokio::test]
    async fn ambiguous_recovery_remains_selectable_if_annotation_write_is_interrupted() {
        let fixture = seed_parallel_recovery_fixture(&["shell", "shell"], &[None, None]).await;
        sqlx::query(
            "CREATE TRIGGER fail_recovery_annotation
             BEFORE UPDATE OF error_annotation ON task
             WHEN NEW.error_annotation IS NOT NULL
             BEGIN SELECT RAISE(ABORT, 'injected recovery annotation failure'); END",
        )
        .execute(fixture.db.pool())
        .await
        .expect("failure injection trigger creates");
        let recovery_result =
            CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
                .run_recovery()
                .await;
        assert!(
            recovery_result.is_err(),
            "the injected annotation write fails"
        );
        sqlx::query("DROP TRIGGER fail_recovery_annotation")
            .execute(fixture.db.pool())
            .await
            .expect("failure injection trigger drops");

        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task loads after interrupted write")
            .expect("Task remains");
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle loads after interrupted write")
            .expect("TaskLifecycle remains");
        assert_eq!(lifecycle.state, TaskLifecycleState::Blocked);
        assert_eq!(
            lifecycle.reason_kind.as_deref(),
            Some("recovery_target_ambiguous")
        );
        assert!(lifecycle
            .reason_ref
            .as_deref()
            .unwrap()
            .starts_with("task-version:"));
        assert!(task.error_annotation.is_none());
        assert!(task.blocked_json.is_none());
        for parent in &fixture.executions {
            let stopped = ExecutionRepo::get_by_id(&*fixture.db, &parent.id)
                .await
                .expect("cancelled attempt loads")
                .expect("cancelled attempt remains");
            assert_eq!(stopped.status, ExecutionStatus::Cancelled);
            assert_eq!(stopped.resume_policy, Some(ResumePolicy::Manual));
            assert!(stopped.work_unit_id.is_some());
        }
        let parent = &fixture.executions[0];
        let work_unit_id = parent.work_unit_id.as_deref().expect("A exact WorkUnit");
        let work_unit = WorkUnitRepo::get_by_id(&*fixture.db, work_unit_id)
            .await
            .expect("A WorkUnit loads")
            .expect("A WorkUnit remains");
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        let selected = service
            .recover_work_unit_execution(
                task.id.clone(),
                parent.id.clone(),
                api_types::RecoverWorkUnitExecutionRequest {
                    work_unit_id: work_unit.id,
                    expected_task_version: task.version,
                    expected_lifecycle_version: lifecycle.version,
                    expected_work_unit_version: work_unit.version,
                    idempotency_key: "select-after-annotation-interruption".to_owned(),
                    context: None,
                },
                "recovery-operator",
            )
            .await
            .expect("exact durable identities resolve the ambiguous lifecycle without annotation");
        assert_eq!(
            selected.execution.parent_execution_id.as_deref(),
            Some(parent.id.as_str())
        );
        assert_eq!(selected.task.lifecycle_state, TaskLifecycleState::Active);
        assert!(selected.task.error_annotation.is_none());
        let sibling = ExecutionRepo::get_by_id(&*fixture.db, &fixture.executions[1].id)
            .await
            .expect("unselected sibling loads")
            .expect("unselected sibling remains");
        assert_eq!(sibling.status, ExecutionStatus::Cancelled);
        assert_eq!(sibling.resume_policy, Some(ResumePolicy::Manual));
    }

    #[tokio::test]
    async fn ambiguous_agent_timeout_recovery_uses_the_same_exact_selection_boundary() {
        let fixture = seed_parallel_recovery_fixture(&["shell", "shell"], &[None, None]).await;
        let outcome = recover_executions(
            &fixture.db,
            &fixture.event_bus,
            fixture.task.clone(),
            fixture.executions.clone(),
            StopReason::AgentTimeout,
            &api_types::Actor::system(api_types::SystemComponent::HeartbeatMonitor),
        )
        .await
        .expect("timeout reconciliation persists its exact stopped attempts");
        assert!(outcome.annotated);
        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task reloads")
            .expect("Task remains");
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle reloads")
            .expect("TaskLifecycle remains");
        assert_eq!(
            lifecycle.reason_kind.as_deref(),
            Some("recovery_target_ambiguous")
        );
        for parent in &fixture.executions {
            let stopped = ExecutionRepo::get_by_id(&*fixture.db, &parent.id)
                .await
                .expect("timeout-stopped attempt loads")
                .expect("timeout-stopped attempt remains");
            assert_eq!(stopped.stop_reason, Some(StopReason::AgentTimeout));
            assert_eq!(stopped.resume_policy, Some(ResumePolicy::Manual));
            assert!(stopped.work_unit_id.is_some());
        }
        let parent = &fixture.executions[1];
        let work_unit = WorkUnitRepo::get_by_id(
            &*fixture.db,
            parent.work_unit_id.as_deref().expect("B exact WorkUnit"),
        )
        .await
        .expect("B WorkUnit loads")
        .expect("B WorkUnit exists");
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        let selected = service
            .recover_work_unit_execution(
                task.id.clone(),
                parent.id.clone(),
                api_types::RecoverWorkUnitExecutionRequest {
                    work_unit_id: work_unit.id.clone(),
                    expected_task_version: task.version,
                    expected_lifecycle_version: lifecycle.version,
                    expected_work_unit_version: work_unit.version,
                    idempotency_key: "select-timeout-target-b".to_owned(),
                    context: None,
                },
                "recovery-operator",
            )
            .await
            .expect("exact timed-out B may be selected without inferring A");
        assert_eq!(
            selected.execution.parent_execution_id.as_deref(),
            Some(parent.id.as_str())
        );
        assert_eq!(
            selected.execution.work_unit_id.as_deref(),
            Some(work_unit.id.as_str())
        );
    }

    #[tokio::test]
    async fn explicit_recovery_rolls_back_lifecycle_child_and_receipt_together() {
        let fixture = seed_parallel_recovery_fixture(&["shell", "shell"], &[None, None]).await;
        CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .run_recovery()
            .await
            .expect("CrashRecovery commits the ambiguous block");
        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task loads")
            .expect("Task exists");
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists");
        let parent = &fixture.executions[0];
        let work_unit_id = parent
            .work_unit_id
            .as_deref()
            .expect("parent exact WorkUnit");
        let work_unit = WorkUnitRepo::get_by_id(&*fixture.db, work_unit_id)
            .await
            .expect("WorkUnit loads")
            .expect("WorkUnit exists");
        let request = api_types::RecoverWorkUnitExecutionRequest {
            work_unit_id: work_unit.id.clone(),
            expected_task_version: task.version,
            expected_lifecycle_version: lifecycle.version,
            expected_work_unit_version: work_unit.version,
            idempotency_key: "rollback-recovery".to_owned(),
            context: None,
        };
        let execution_count_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("execution count loads");
        let event_count_before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event WHERE scope_type = 'task' AND scope_id = ?",
        )
        .bind(&task.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("event count loads");
        sqlx::query(
            "CREATE TRIGGER fail_recovery_receipt
             BEFORE INSERT ON domain_event
             WHEN NEW.event_type = 'execution.recovery_admitted'
             BEGIN SELECT RAISE(ABORT, 'injected recovery receipt failure'); END",
        )
        .execute(fixture.db.pool())
        .await
        .expect("failure injection trigger creates");
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        assert!(service
            .recover_work_unit_execution(
                task.id.clone(),
                parent.id.clone(),
                request.clone(),
                "recovery-operator",
            )
            .await
            .is_err());
        sqlx::query("DROP TRIGGER fail_recovery_receipt")
            .execute(fixture.db.pool())
            .await
            .expect("failure injection trigger drops");

        let task_after_failure = TaskRepo::get_by_id(&*fixture.db, &task.id, false)
            .await
            .expect("Task reloads after rollback")
            .expect("Task remains");
        let lifecycle_after_failure = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("lifecycle reloads after rollback")
            .expect("lifecycle remains");
        assert_eq!(task_after_failure.version, task.version);
        assert_eq!(
            task_after_failure.lifecycle_state,
            TaskLifecycleState::Blocked
        );
        assert_eq!(task_after_failure.error_annotation, task.error_annotation);
        assert_eq!(lifecycle_after_failure.version, lifecycle.version);
        assert_eq!(lifecycle_after_failure.state, lifecycle.state);
        let execution_count_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("execution count reloads");
        assert_eq!(execution_count_after, execution_count_before);
        let event_count_after: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event WHERE scope_type = 'task' AND scope_id = ?",
        )
        .bind(&task.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("event count reloads");
        assert_eq!(event_count_after, event_count_before);
        let active_leases: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workspace_lease WHERE status = 'active' AND task_id = ?",
        )
        .bind(&task.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("lease count loads");
        assert_eq!(active_leases, 0);

        let retried = service
            .recover_work_unit_execution(
                task.id.clone(),
                parent.id.clone(),
                request,
                "recovery-operator",
            )
            .await
            .expect("same request can commit after complete rollback");
        assert!(!retried.idempotent_replay);
        assert_eq!(
            retried.execution.parent_execution_id.as_deref(),
            Some(parent.id.as_str())
        );
    }

    #[tokio::test]
    async fn competing_ambiguous_recovery_choices_commit_only_one_decision() {
        let fixture = seed_parallel_recovery_fixture(&["shell", "shell"], &[None, None]).await;
        CrashRecovery::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus))
            .run_recovery()
            .await
            .expect("CrashRecovery commits the ambiguous block");
        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
            .await
            .expect("Task loads")
            .expect("Task exists");
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists");
        let work_unit_a = WorkUnitRepo::get_by_id(
            &*fixture.db,
            fixture.executions[0].work_unit_id.as_deref().unwrap(),
        )
        .await
        .expect("A WorkUnit loads")
        .expect("A WorkUnit exists");
        let work_unit_b = WorkUnitRepo::get_by_id(
            &*fixture.db,
            fixture.executions[1].work_unit_id.as_deref().unwrap(),
        )
        .await
        .expect("B WorkUnit loads")
        .expect("B WorkUnit exists");
        let service = TaskService::new(Arc::clone(&fixture.db), Arc::clone(&fixture.event_bus));
        let request_a = api_types::RecoverWorkUnitExecutionRequest {
            work_unit_id: work_unit_a.id.clone(),
            expected_task_version: task.version,
            expected_lifecycle_version: lifecycle.version,
            expected_work_unit_version: work_unit_a.version,
            idempotency_key: "competing-a".to_owned(),
            context: None,
        };
        let request_b = api_types::RecoverWorkUnitExecutionRequest {
            work_unit_id: work_unit_b.id.clone(),
            expected_task_version: task.version,
            expected_lifecycle_version: lifecycle.version,
            expected_work_unit_version: work_unit_b.version,
            idempotency_key: "competing-b".to_owned(),
            context: None,
        };
        let a = service.recover_work_unit_execution(
            task.id.clone(),
            fixture.executions[0].id.clone(),
            request_a,
            "recovery-operator",
        );
        let b = service.recover_work_unit_execution(
            task.id.clone(),
            fixture.executions[1].id.clone(),
            request_b,
            "recovery-operator",
        );
        let (a, b) = tokio::join!(a, b);
        assert_ne!(
            a.is_ok(),
            b.is_ok(),
            "only one stale aggregate decision wins"
        );
        let successful = a.or(b).expect("one exact decision commits");
        assert_eq!(successful.task.lifecycle_state, TaskLifecycleState::Active);
        let children: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution WHERE task_id = ? AND parent_execution_id IS NOT NULL",
        )
        .bind(&task.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("child count loads");
        assert_eq!(children, 1);
        let receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'execution.recovery_admitted' AND scope_id = ?",
        )
        .bind(&task.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("receipt count loads");
        assert_eq!(receipts, 1);
        let current = TaskRepo::get_by_id(&*fixture.db, &task.id, false)
            .await
            .expect("Task reloads")
            .expect("Task remains");
        assert!(current.error_annotation.is_none());
        assert!(current.blocked_json.is_none());
    }

    #[tokio::test]
    async fn cancel_running_executions_returns_cancelled_execution_metadata() {
        let db = Arc::new(sqlite_db().await);
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent =
            seed_agent_with_harness(&db, AgentStatus::Busy, Some(now_rfc3339()), "codex").await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent.id.clone()),
        )
        .await;
        let execution = seed_running_execution(
            &db,
            task.id.clone(),
            agent.id.clone(),
            Some("session-789".to_owned()),
        )
        .await;

        let cancelled = cancel_running_executions(
            &db,
            &task.id,
            StopReason::CrashRecovery,
            &api_types::Actor::system(api_types::SystemComponent::CrashRecovery),
            ResumePolicy::Manual,
        )
        .await
        .expect("cancellation succeeds");

        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].execution_id, execution.id);
        assert_eq!(
            cancelled[0].resumable_external_session_id.as_deref(),
            Some("session-789")
        );
    }

    #[tokio::test]
    async fn crash_recovery_keeps_active_task_with_resumable_execution() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(1024));
        let _task_service = Arc::new(TaskService::new(Arc::clone(&db), Arc::clone(&event_bus)));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent =
            seed_agent_with_harness(&db, AgentStatus::Busy, Some(now_rfc3339()), "codex").await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent.id.clone()),
        )
        .await;
        let execution = seed_running_execution(
            &db,
            task.id.clone(),
            agent.id,
            Some("session-123".to_owned()),
        )
        .await;

        CrashRecovery::new(Arc::clone(&db), event_bus)
            .run_recovery()
            .await
            .expect("recovery runs");

        let updated_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(updated_task.lifecycle_state, TaskLifecycleState::Active);

        let updated_execution = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(updated_execution.status, ExecutionStatus::Cancelled);
        let external_session_id: Option<String> =
            sqlx::query_scalar("SELECT external_session_id FROM harness_session WHERE id = ?")
                .bind(
                    updated_execution
                        .harness_session_id
                        .as_deref()
                        .expect("exact HarnessSession link"),
                )
                .fetch_one(db.pool())
                .await
                .expect("HarnessSession identity loads");
        assert_eq!(external_session_id.as_deref(), Some("session-123"));
        assert_eq!(updated_execution.resume_policy, Some(ResumePolicy::Auto));
    }

    #[tokio::test]
    async fn crash_recovery_does_not_follow_up_resumable_execution() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(1024));
        let _task_service = Arc::new(TaskService::new(Arc::clone(&db), Arc::clone(&event_bus)));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent =
            seed_agent_with_harness(&db, AgentStatus::Busy, Some(now_rfc3339()), "codex").await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent.id.clone()),
        )
        .await;
        let execution = seed_running_execution(
            &db,
            task.id.clone(),
            agent.id,
            Some("session-456".to_owned()),
        )
        .await;

        CrashRecovery::new(Arc::clone(&db), event_bus)
            .run_recovery()
            .await
            .expect("recovery runs");

        let updated_task = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(updated_task.lifecycle_state, TaskLifecycleState::Active);

        let executions = ExecutionRepo::list_by_task(
            &*db,
            &task.id,
            PageRequest {
                cursor: None,
                limit: 100,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await
        .expect("executions load");
        assert_eq!(executions.items.len(), 1);
        assert_eq!(executions.items[0].id, execution.id);
        assert_eq!(executions.items[0].status, ExecutionStatus::Cancelled);
    }

    async fn stamp_recovery_annotation(
        db: &SqliteDb,
        task: &Task,
        blocked_execution_id: Option<&str>,
    ) -> Task {
        let annotation = json!({
            "type": api_types::FailureKind::RecoveryRequired,
            "blocking_reason": "crash_recovery",
            "blocked_by": "system:crash_recovery",
            "blocked_at": now_rfc3339(),
            "blocked_execution_id": blocked_execution_id,
            "artifact": blocked_execution_id.map(|execution_id| json!({
                "kind": "execution",
                "id": execution_id,
                "log_path": null,
            })),
            "message": "Recovered after server restart",
            "recovery_actions": ["reexecute", "cancel_task"],
        })
        .to_string();
        TaskRepo::update(
            db,
            UpdateTask {
                id: task.id.clone(),
                expected_version: task.version,
                title: None,
                description: None,
                priority: None,
                merge_config: None,
                error_annotation: Some(Some(annotation)),
                blocked_json: None,
                failed_json: None,
                parent_task_id: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("annotation stamps")
    }

    #[tokio::test]
    async fn crash_recovery_preserves_recovery_annotations_without_an_exact_execution() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let task = seed_task(&db, project_id, repo_id, "active".to_owned(), None).await;
        stamp_recovery_annotation(&db, &task, None).await;

        let recovered = CrashRecovery::new(Arc::clone(&db), event_bus)
            .run_recovery()
            .await
            .expect("recovery runs");
        assert_eq!(recovered, 0);

        let updated = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert!(updated.error_annotation.is_some());
    }

    #[tokio::test]
    async fn crash_recovery_preserves_legitimate_pending_recovery_annotations() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent = seed_agent(&db, AgentStatus::Busy, Some(now_rfc3339())).await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent.id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent.id, None).await;
        ExecutionRepo::update(
            &*db,
            UpdateExecution {
                id: execution.id.clone(),
                status: Some(ExecutionStatus::Cancelled),
                stop_reason: Some(Some(StopReason::CrashRecovery)),
                stopped_by: Some(Some("system:crash_recovery".to_owned())),
                resume_policy: Some(Some(ResumePolicy::Manual)),
                stopped_at: Some(Some(now_rfc3339())),
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: Some(Some("Recovered".to_owned())),
                executor_config_snapshot_json: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("execution updates");
        let annotated = stamp_recovery_annotation(&db, &task, Some(&execution.id)).await;

        let recovered = CrashRecovery::new(Arc::clone(&db), event_bus)
            .run_recovery()
            .await
            .expect("recovery runs");
        assert_eq!(recovered, 0);

        let updated = TaskRepo::get_by_id(&*db, &annotated.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(
            updated.error_annotation.as_deref(),
            annotated.error_annotation.as_deref()
        );
        let annotation: Value =
            serde_json::from_str(updated.error_annotation.as_deref().unwrap()).unwrap();
        assert_eq!(annotation["blocked_execution_id"], execution.id);
    }

    #[tokio::test]
    async fn crash_recovery_clears_stale_recovery_annotations_for_missing_execution() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let task = seed_task(&db, project_id, repo_id, "active".to_owned(), None).await;
        stamp_recovery_annotation(&db, &task, Some("missing-execution-id")).await;

        let recovered = CrashRecovery::new(Arc::clone(&db), event_bus)
            .run_recovery()
            .await
            .expect("recovery runs");
        assert_eq!(recovered, 1);

        let updated = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(updated.error_annotation, None);
    }

    async fn seed_agent_with_daemon(
        db: &SqliteDb,
        machine_id: &str,
        status: AgentStatus,
    ) -> (String, Agent) {
        let now = now_rfc3339();
        let daemon_id = new_uuid_v4();
        DaemonRepo::upsert_by_machine_id(
            db,
            UpsertDaemon {
                id: daemon_id.clone(),
                machine_id: machine_id.to_owned(),
                hostname: "test-host".to_owned(),
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                agent_version: None,
                labels_json: "{}".to_owned(),
                status: DaemonStatus::Online,
                registration_token_hash: None,
                owner_id: None,
                visibility: "global".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("daemon creates");

        let agent = AgentRepo::create(
            db,
            CreateAgent {
                id: new_uuid_v4(),
                name: "remote".to_owned(),
                description: None,
                executor_type: "shell".to_owned(),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                capabilities_json: "[]".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: None,
                daemon_id: Some(daemon_id),
                max_concurrent_tasks: 1,
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: None,
                visibility: "global".to_owned(),
                prompt_template: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("agent creates");
        (agent.id.clone(), agent)
    }

    async fn seed_running_execution_with_created_at(
        db: &SqliteDb,
        task_id: String,
        agent_id: String,
        created_at: &str,
    ) -> db::Execution {
        let execution = seed_running_execution(db, task_id, agent_id, None).await;
        ExecutionRepo::update(
            db,
            UpdateExecution {
                id: execution.id.clone(),
                status: None,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                updated_at: created_at.to_owned(),
            },
        )
        .await
        .expect("execution timestamp updates");
        sqlx::query("UPDATE execution SET created_at = ? WHERE id = ?")
            .bind(created_at)
            .bind(&execution.id)
            .execute(db.pool())
            .await
            .expect("execution created_at updates");
        ExecutionRepo::get_by_id(db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists")
    }

    #[tokio::test]
    async fn heartbeat_monitor_fails_remote_execution_when_daemon_stays_disconnected() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let registry = Arc::new(DaemonConnectionRegistry::without_handlers());
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, _) =
            seed_agent_with_daemon(&db, "remote-machine-a", AgentStatus::Idle).await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent_id, None).await;

        let monitor = HeartbeatMonitor::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_daemon_connections(Arc::clone(&registry))
            .with_daemon_disconnect_grace(Duration::from_millis(1));

        monitor.check_once().await.expect("first check");
        tokio::time::sleep(Duration::from_millis(5)).await;
        let interrupted = monitor.check_once().await.expect("second check");
        assert_eq!(interrupted, 1);

        let updated = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(updated.status, ExecutionStatus::Failed);
        assert_eq!(updated.stop_reason, Some(StopReason::DaemonDisconnected));
    }

    #[tokio::test]
    async fn daemon_report_does_not_fail_paused_pending_provider_start() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let mut events = event_bus.subscribe();
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, agent) =
            seed_agent_with_daemon(&db, "remote-machine-pending-report", AgentStatus::Idle).await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution =
            seed_pending_provider_execution(&db, task.id.clone(), agent_id.clone()).await;
        AgentRepo::set_paused(&*db, &agent_id, true)
            .await
            .expect("Agent pauses after durable Execution admission");
        sqlx::query("UPDATE execution SET created_at = ? WHERE id = ?")
            .bind("1970-01-01T00:00:00+00:00")
            .bind(&execution.id)
            .execute(db.pool())
            .await
            .expect("Execution ages beyond the daemon report grace window");

        let daemon = DaemonRepo::get_by_id(
            &*db,
            agent.daemon_id.as_deref().expect("remote Agent daemon"),
        )
        .await
        .expect("daemon loads")
        .expect("daemon exists");
        let pending_before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE entity_type = 'execution' AND entity_id = ?
               AND event_type = 'execution.provider_start_pending'
               AND dedupe_key = 'execution.provider_start_pending:' || ?",
        )
        .bind(&execution.id)
        .bind(&execution.id)
        .fetch_one(db.pool())
        .await
        .expect("pending receipt count loads");
        let attempts_before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE entity_type = 'execution' AND entity_id = ?
               AND event_type = 'execution.provider_start_attempted'",
        )
        .bind(&execution.id)
        .fetch_one(db.pool())
        .await
        .expect("attempt receipt count loads");
        assert_eq!(pending_before, 1);
        assert_eq!(attempts_before, 0);

        let interrupted = reconcile_daemon_report_executions(&db, &event_bus, None, &daemon, &[])
            .await
            .expect("empty remote daemon report reconciles");

        assert_eq!(interrupted, 0);
        let unchanged = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("Execution reloads")
            .expect("Execution exists");
        assert_eq!(unchanged.status, ExecutionStatus::Running);
        assert_eq!(unchanged.stop_reason, None);
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists");
        assert_eq!(lifecycle.state, TaskLifecycleState::Active);
        let attempts_after: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE entity_type = 'execution' AND entity_id = ?
               AND event_type = 'execution.provider_start_attempted'",
        )
        .bind(&execution.id)
        .fetch_one(db.pool())
        .await
        .expect("attempt receipt count reloads");
        assert_eq!(attempts_after, 0);
        let failed_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE entity_type = 'execution' AND entity_id = ?
               AND event_type = 'execution.failed'",
        )
        .bind(&execution.id)
        .fetch_one(db.pool())
        .await
        .expect("failed event count loads");
        assert_eq!(failed_events, 0);
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn disconnected_daemon_does_not_fail_pending_provider_start() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let mut events = event_bus.subscribe();
        let registry = Arc::new(DaemonConnectionRegistry::without_handlers());
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, _) =
            seed_agent_with_daemon(&db, "remote-machine-pending-disconnect", AgentStatus::Idle)
                .await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution =
            seed_pending_provider_execution(&db, task.id.clone(), agent_id.clone()).await;
        AgentRepo::set_paused(&*db, &agent_id, true)
            .await
            .expect("Agent pauses while provider Start is pending");

        let monitor = HeartbeatMonitor::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_daemon_connections(registry)
            .with_daemon_disconnect_grace(Duration::ZERO);
        assert_eq!(
            monitor
                .check_once()
                .await
                .expect("monitor checks disconnect"),
            0,
            "queued work is excluded from daemon disconnect reconciliation"
        );
        let agent = AgentRepo::get_by_id(&*db, &agent_id)
            .await
            .expect("Agent reloads")
            .expect("Agent exists");
        let daemon_id = agent.daemon_id.expect("Agent has its remote daemon");
        let daemon = DaemonRepo::get_by_id(&*db, &daemon_id)
            .await
            .expect("daemon loads")
            .expect("daemon exists");
        let stopped_by =
            api_types::Actor::system(api_types::SystemComponent::HeartbeatMonitor).display();
        let unchanged_by_racing_report = fail_execution_daemon_disconnected(
            &db,
            &event_bus,
            None,
            FailDaemonDisconnectedExecution {
                execution: &execution,
                daemon_id: &daemon.id,
                error_message: "remote daemon report raced a queued Start".to_owned(),
                stopped_by: &stopped_by,
                reconciliation_reason: "daemon_disconnected",
            },
        )
        .await
        .expect("atomic disconnect reconciliation preserves queued work");
        assert_eq!(unchanged_by_racing_report.status, ExecutionStatus::Running);
        let unchanged = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("Execution reloads")
            .expect("Execution exists");
        assert_eq!(unchanged.status, ExecutionStatus::Running);
        assert_eq!(unchanged.stop_reason, None);
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists");
        assert_eq!(lifecycle.state, TaskLifecycleState::Active);
        let attempts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE entity_id = ? AND event_type = 'execution.provider_start_attempted'",
        )
        .bind(&execution.id)
        .fetch_one(db.pool())
        .await
        .expect("provider attempt count loads");
        assert_eq!(attempts, 0);
        let failed_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE entity_type = 'execution' AND entity_id = ?
               AND event_type = 'execution.failed'",
        )
        .bind(&execution.id)
        .fetch_one(db.pool())
        .await
        .expect("failed event count loads");
        assert_eq!(failed_events, 0);
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn busy_agent_timeout_does_not_recover_pending_provider_start() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let mut events = event_bus.subscribe();
        let agent = seed_agent_with_harness(
            &db,
            AgentStatus::Busy,
            Some("1970-01-01T00:00:00+00:00".to_owned()),
            "shell",
        )
        .await;
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent.id.clone()),
        )
        .await;
        let execution =
            seed_pending_provider_execution(&db, task.id.clone(), agent.id.clone()).await;
        AgentRepo::set_paused(&*db, &agent.id, true)
            .await
            .expect("Agent pauses while provider Start is pending");

        let monitor = HeartbeatMonitor::with_check_interval(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            Duration::from_millis(1),
        );
        assert_eq!(
            monitor
                .check_once()
                .await
                .expect("monitor checks heartbeat"),
            0,
            "a queued provider Start does not count as an Agent heartbeat timeout"
        );

        let updated_agent = AgentRepo::get_by_id(&*db, &agent.id)
            .await
            .expect("Agent reloads")
            .expect("Agent exists");
        assert_eq!(updated_agent.status, AgentStatus::Busy);
        assert!(updated_agent.paused);
        let unchanged = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("Execution reloads")
            .expect("Execution exists");
        assert_eq!(unchanged.status, ExecutionStatus::Running);
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists");
        assert_eq!(lifecycle.state, TaskLifecycleState::Active);
        assert!(
            events.try_recv().is_err(),
            "no timeout/recovery event is emitted"
        );
    }

    #[tokio::test]
    async fn task_cancellation_still_resolves_pending_provider_start() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, _) =
            seed_agent_with_daemon(&db, "remote-machine-pending-cancel", AgentStatus::Idle).await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_pending_provider_execution(&db, task.id.clone(), agent_id).await;
        let executor = Arc::new(RecordingCancelExecutor::default());
        let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_task_executor(executor.clone());

        service
            .cancel_task(task.id.clone())
            .await
            .expect("explicit Task cancellation resolves queued work");

        let cancelled = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("Execution reloads")
            .expect("Execution exists");
        assert_eq!(cancelled.status, ExecutionStatus::Cancelled);
        assert_eq!(cancelled.stop_reason, Some(StopReason::TaskCancelled));
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists");
        assert_eq!(lifecycle.state, TaskLifecycleState::Cancelled);
        let attempt_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE entity_id = ? AND event_type = 'execution.provider_start_attempted'",
        )
        .bind(&execution.id)
        .fetch_one(db.pool())
        .await
        .expect("provider attempt count loads");
        assert_eq!(attempt_count, 0);
        assert!(executor
            .cancelled
            .lock()
            .expect("provider cancel log loads")
            .is_empty());
        let lease_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workspace_lease WHERE execution_id = ?")
                .bind(&execution.id)
                .fetch_one(db.pool())
                .await
                .expect("WorkspaceLease count loads");
        assert_eq!(lease_count, 0);
        let cancelled_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE entity_type = 'execution' AND entity_id = ?
               AND event_type = 'execution.cancelled'",
        )
        .bind(&execution.id)
        .fetch_one(db.pool())
        .await
        .expect("Execution cancellation events load");
        assert_eq!(cancelled_events, 1);
    }

    #[tokio::test]
    async fn heartbeat_monitor_leaves_remote_execution_alone_within_disconnect_grace() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let registry = Arc::new(DaemonConnectionRegistry::without_handlers());
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, _) =
            seed_agent_with_daemon(&db, "remote-machine-b", AgentStatus::Idle).await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent_id, None).await;

        let monitor = HeartbeatMonitor::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_daemon_connections(Arc::clone(&registry))
            .with_daemon_disconnect_grace(Duration::from_secs(120));

        let interrupted = monitor.check_once().await.expect("monitor checks");
        assert_eq!(interrupted, 0);

        let updated = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(updated.status, ExecutionStatus::Running);
    }

    #[tokio::test]
    async fn heartbeat_monitor_clears_disconnect_tracking_when_daemon_reconnects() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let registry = Arc::new(DaemonConnectionRegistry::without_handlers());
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, agent) =
            seed_agent_with_daemon(&db, "remote-machine-c", AgentStatus::Idle).await;
        let daemon_id = agent.daemon_id.clone().expect("daemon id");
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent_id, None).await;

        let monitor = HeartbeatMonitor::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_daemon_connections(Arc::clone(&registry))
            .with_daemon_disconnect_grace(Duration::from_secs(120));

        monitor.check_once().await.expect("first check");
        let (connection, _rx) = DaemonConnection::new(daemon_id.clone());
        registry.register(daemon_id, connection);
        let interrupted = monitor.check_once().await.expect("second check");
        assert_eq!(interrupted, 0);

        let updated = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(updated.status, ExecutionStatus::Running);
    }

    #[tokio::test]
    async fn heartbeat_monitor_skips_embedded_daemon_executions_for_disconnect_check() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let registry = Arc::new(DaemonConnectionRegistry::without_handlers());
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, _) = seed_agent_with_daemon(
            &db,
            &crate::embedded_daemon::embedded_machine_id(),
            AgentStatus::Idle,
        )
        .await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent_id, None).await;

        let monitor = HeartbeatMonitor::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_daemon_connections(Arc::clone(&registry))
            .with_daemon_disconnect_grace(Duration::from_millis(1));

        monitor.check_once().await.expect("first check");
        tokio::time::sleep(Duration::from_millis(5)).await;
        let interrupted = monitor.check_once().await.expect("second check");
        assert_eq!(interrupted, 0);

        let updated = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(updated.status, ExecutionStatus::Running);
    }

    #[tokio::test]
    async fn daemon_report_reconciliation_uses_frozen_execution_host_after_agent_rebind() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_a_id, agent_a) =
            seed_agent_with_daemon(&db, "reconcile-frozen-host-a", AgentStatus::Idle).await;
        let (_agent_b_id, agent_b) =
            seed_agent_with_daemon(&db, "reconcile-current-host-b", AgentStatus::Idle).await;
        let daemon_a = agent_a.daemon_id.clone().expect("daemon A is bound");
        let daemon_b = agent_b.daemon_id.clone().expect("daemon B is bound");
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_a_id.clone()),
        )
        .await;
        let execution = seed_running_execution_with_created_at(
            &db,
            task.id.clone(),
            agent_a_id.clone(),
            "2000-01-01T00:00:00Z",
        )
        .await;
        let current_agent = AgentRepo::get_by_id(&*db, &agent_a_id)
            .await
            .expect("current Agent loads")
            .expect("current Agent exists");
        AgentRepo::update(
            &*db,
            UpdateAgent {
                id: current_agent.id,
                expected_version: current_agent.version,
                name: None,
                description: None,
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: None,
                config_json: None,
                daemon_id: Some(Some(daemon_b.clone())),
                max_concurrent_tasks: None,
                heartbeat_interval_seconds: None,
                max_missed_heartbeats: None,
                status: None,
                last_heartbeat_at: None,
                is_default: None,
                paused: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("Agent is rebound after Execution admission");

        let daemon_b_row = DaemonRepo::get_by_id(&*db, &daemon_b)
            .await
            .expect("daemon B query succeeds")
            .expect("daemon B exists");
        assert_eq!(
            reconcile_daemon_report_executions(&db, &event_bus, None, &daemon_b_row, &[],)
                .await
                .expect("daemon B report reconciliation succeeds"),
            0
        );
        let still_running = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("Execution loads")
            .expect("Execution exists");
        assert_eq!(still_running.status, ExecutionStatus::Running);

        let daemon_a_row = DaemonRepo::get_by_id(&*db, &daemon_a)
            .await
            .expect("daemon A query succeeds")
            .expect("daemon A exists");
        assert_eq!(
            reconcile_daemon_report_executions(&db, &event_bus, None, &daemon_a_row, &[],)
                .await
                .expect("frozen daemon A report reconciliation succeeds"),
            1
        );
        let failed = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("Execution reloads")
            .expect("Execution exists");
        assert_eq!(failed.status, ExecutionStatus::Failed);
        assert_eq!(failed.stop_reason, Some(StopReason::DaemonDisconnected));
    }

    #[tokio::test]
    async fn heartbeat_monitor_does_not_cancel_remote_stalled_executions_via_embedded_executor() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, _) =
            seed_agent_with_daemon(&db, "remote-machine-d", AgentStatus::Idle).await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent_id, None).await;
        ExecutionRepo::update(
            &*db,
            UpdateExecution {
                id: execution.id.clone(),
                status: None,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: Some(Some("1970-01-01T00:00:00+00:00".to_owned())),
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("execution activity updates");

        let executor = Arc::new(RecordingCancelExecutor::default());
        let monitor = HeartbeatMonitor::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_task_executor(executor.clone())
            .with_execution_stall_timeout(Duration::from_secs(1));

        let stalled = monitor.check_once().await.expect("monitor checks");
        assert_eq!(stalled, 1);
        assert!(executor
            .cancelled
            .lock()
            .expect("cancel log lock")
            .is_empty());
    }

    #[tokio::test]
    async fn heartbeat_monitor_cancels_stalled_executions_of_daemonless_agents() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let now = now_rfc3339();
        let agent = AgentRepo::create(
            &*db,
            CreateAgent {
                id: new_uuid_v4(),
                name: "embedded-shell".to_owned(),
                description: None,
                executor_type: "shell".to_owned(),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                capabilities_json: "[]".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: None,
                daemon_id: None,
                max_concurrent_tasks: 1,
                heartbeat_interval_seconds: 1,
                max_missed_heartbeats: 1,
                status: AgentStatus::Idle,
                last_heartbeat_at: Some(now.clone()),
                is_default: false,
                paused: false,
                owner_id: None,
                visibility: "global".to_owned(),
                prompt_template: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("daemonless agent creates");
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent.id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent.id.clone(), None).await;
        ExecutionRepo::update(
            &*db,
            UpdateExecution {
                id: execution.id.clone(),
                status: None,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: Some(Some("1970-01-01T00:00:00+00:00".to_owned())),
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("execution activity updates");

        let executor = Arc::new(RecordingCancelExecutor::default());
        let monitor = HeartbeatMonitor::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_task_executor(executor.clone())
            .with_execution_stall_timeout(Duration::from_secs(1));

        let stalled = monitor.check_once().await.expect("monitor checks");
        assert_eq!(stalled, 1);
        assert_eq!(
            executor
                .cancelled
                .lock()
                .expect("cancel log lock")
                .as_slice(),
            std::slice::from_ref(&execution.id)
        );
    }

    #[tokio::test]
    async fn daemon_report_reconcile_interrupts_missing_old_running_execution() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let task_service = Arc::new(TaskService::new(Arc::clone(&db), Arc::clone(&event_bus)));
        let service = DaemonService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_task_service(task_service);
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, agent) =
            seed_agent_with_daemon(&db, "remote-machine-report", AgentStatus::Idle).await;
        let daemon_id = agent.daemon_id.clone().expect("daemon id");
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_running_execution_with_created_at(
            &db,
            task.id.clone(),
            agent_id,
            "1970-01-01T00:00:00+00:00",
        )
        .await;

        service
            .ingest_report(
                &daemon_id,
                DaemonReportInput {
                    detected_clis: vec![DetectedCliInput {
                        kind: "shell".to_owned(),
                        availability: "authenticated".to_owned(),
                        config_path: None,
                        version: None,
                        path: None,
                    }],
                    runtimes: Vec::new(),
                    labels: None,
                    active_execution_ids: Some(Vec::new()),
                },
            )
            .await
            .expect("report ingests");

        let updated = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(updated.status, ExecutionStatus::Failed);
        assert_eq!(updated.stop_reason, Some(StopReason::DaemonDisconnected));
    }

    #[tokio::test]
    async fn daemon_report_reconcile_leaves_fresh_running_execution_untouched() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let service = DaemonService::new(Arc::clone(&db), Arc::clone(&event_bus));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, agent) =
            seed_agent_with_daemon(&db, "remote-machine-fresh", AgentStatus::Idle).await;
        let daemon_id = agent.daemon_id.clone().expect("daemon id");
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent_id, None).await;

        service
            .ingest_report(
                &daemon_id,
                DaemonReportInput {
                    detected_clis: vec![DetectedCliInput {
                        kind: "shell".to_owned(),
                        availability: "authenticated".to_owned(),
                        config_path: None,
                        version: None,
                        path: None,
                    }],
                    runtimes: Vec::new(),
                    labels: None,
                    active_execution_ids: Some(Vec::new()),
                },
            )
            .await
            .expect("report ingests");

        let updated = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(updated.status, ExecutionStatus::Running);
    }

    #[tokio::test]
    async fn daemon_report_without_active_execution_ids_does_not_reconcile() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let service = DaemonService::new(Arc::clone(&db), Arc::clone(&event_bus));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let (agent_id, agent) =
            seed_agent_with_daemon(&db, "remote-machine-none", AgentStatus::Idle).await;
        let daemon_id = agent.daemon_id.clone().expect("daemon id");
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent_id.clone()),
        )
        .await;
        let execution = seed_running_execution_with_created_at(
            &db,
            task.id.clone(),
            agent_id,
            "1970-01-01T00:00:00+00:00",
        )
        .await;

        service
            .ingest_report(
                &daemon_id,
                DaemonReportInput {
                    detected_clis: vec![DetectedCliInput {
                        kind: "shell".to_owned(),
                        availability: "authenticated".to_owned(),
                        config_path: None,
                        version: None,
                        path: None,
                    }],
                    runtimes: Vec::new(),
                    labels: None,
                    active_execution_ids: None,
                },
            )
            .await
            .expect("report ingests");

        let updated = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution loads")
            .expect("execution exists");
        assert_eq!(updated.status, ExecutionStatus::Running);
    }

    #[tokio::test]
    async fn crash_recovery_clears_stale_recovery_annotations_for_completed_execution() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent = seed_agent(&db, AgentStatus::Busy, Some(now_rfc3339())).await;
        let task = seed_task(
            &db,
            project_id,
            repo_id,
            "active".to_owned(),
            Some(agent.id.clone()),
        )
        .await;
        let execution = seed_running_execution(&db, task.id.clone(), agent.id, None).await;
        ExecutionRepo::update(
            &*db,
            UpdateExecution {
                id: execution.id.clone(),
                status: Some(ExecutionStatus::Completed),
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: Some(Some(now_rfc3339())),
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("execution updates");
        stamp_recovery_annotation(&db, &task, Some(&execution.id)).await;

        let recovered = CrashRecovery::new(Arc::clone(&db), event_bus)
            .run_recovery()
            .await
            .expect("recovery runs");
        assert_eq!(recovered, 1);

        let updated = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(updated.error_annotation, None);
    }
}
