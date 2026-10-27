use crate::{recovery::cancel_running_executions, Result, TaskService};
use db::{ExecutionRepo, ResumePolicy, SqliteDb, StopReason};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use executors::TaskExecutor;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{task::JoinHandle, time::sleep};
use tracing::Instrument;

const SHUTDOWN_ERROR_TYPE: &str = "shutdown";

#[derive(Clone)]
pub struct GracefulShutdown {
    accepting_new_work: Arc<AtomicBool>,
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    task_executor: Option<Arc<dyn TaskExecutor>>,
    task_service: Option<Arc<TaskService>>,
}

impl GracefulShutdown {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            accepting_new_work: Arc::new(AtomicBool::new(true)),
            db,
            event_bus,
            task_executor: None,
            task_service: None,
        }
    }

    pub fn with_task_executor(mut self, task_executor: Arc<dyn TaskExecutor>) -> Self {
        self.task_executor = Some(task_executor);
        self
    }

    pub fn with_task_service(mut self, task_service: Arc<TaskService>) -> Self {
        self.task_service = Some(task_service);
        self
    }

    pub fn is_accepting_work(&self) -> bool {
        self.accepting_new_work.load(Ordering::SeqCst)
    }

    #[tracing::instrument(skip(self))]
    pub async fn shutdown(&self) -> Result<()> {
        self.accepting_new_work.store(false, Ordering::SeqCst);
        tracing::info!("graceful shutdown started");
        sleep(transaction_drain_wait()).await;

        let rows = sqlx::query(
            "SELECT DISTINCT task.id, task.project_id
             FROM task
             JOIN execution ON execution.task_id = task.id AND execution.status = 'running'
             WHERE task.deleted_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM domain_event pending
                   WHERE pending.entity_type = 'execution'
                     AND pending.entity_id = execution.id
                     AND pending.event_type = 'execution.provider_start_pending'
                     AND pending.dedupe_key = 'execution.provider_start_pending:' || execution.id
                     AND NOT EXISTS (
                         SELECT 1 FROM domain_event attempt
                         WHERE attempt.entity_type = 'execution'
                           AND attempt.entity_id = execution.id
                           AND attempt.event_type = 'execution.provider_start_attempted'
                           AND attempt.dedupe_key = 'execution.provider_start_attempted:' || execution.id
                     )
               )
             ORDER BY task.id",
        )
        .fetch_all(self.db.pool())
        .await?;
        let in_progress_count = rows.len();

        for row in rows {
            use sqlx::Row;

            let id: String = row.get("id");
            let project_id: String = row.get("project_id");

            self.cancel_executor_processes_for_task(&id).await?;
            let _cancelled = cancel_running_executions(
                &self.db,
                &id,
                StopReason::GracefulShutdown,
                &api_types::Actor::system(api_types::SystemComponent::GracefulShutdown),
                ResumePolicy::Auto,
            )
            .await
            .unwrap_or_default();

            self.event_bus.publish(ForgeEvent {
                event_type: "task.recovered".to_owned(),
                entity_id: id,
                timestamp: event_timestamp(),
                context: EventContext::TaskRecovered {
                    project_id,
                    reason: SHUTDOWN_ERROR_TYPE.to_owned(),
                },
            });
        }

        tracing::info!(
            recovered_tasks = in_progress_count,
            "graceful shutdown completed"
        );
        Ok(())
    }

    async fn cancel_executor_processes_for_task(&self, task_id: &str) -> Result<()> {
        if self.task_executor.is_none() && self.task_service.is_none() {
            return Ok(());
        }
        let pending_provider_starts: HashSet<String> =
            ExecutionRepo::list_pending_provider_starts(&*self.db)
                .await?
                .into_iter()
                .map(|execution| execution.id)
                .collect();
        let rows = sqlx::query("SELECT id FROM execution WHERE task_id = ? AND status = 'running'")
            .bind(task_id)
            .fetch_all(self.db.pool())
            .await?;
        for row in rows {
            use sqlx::Row;

            let execution_id: String = row.get("id");
            if pending_provider_starts.contains(&execution_id) {
                continue;
            }
            let Some(execution) = ExecutionRepo::get_by_id(&*self.db, &execution_id).await? else {
                continue;
            };
            if let Err(error) = crate::recovery::cancel_execution_on_frozen_host(
                &self.db,
                &execution,
                "graceful shutdown",
                None,
                self.task_service.as_deref(),
                self.task_executor.as_deref(),
            )
            .await
            {
                tracing::warn!(
                    task_id = %task_id,
                    execution_id = %execution_id,
                    %error,
                    "execution cancellation failed on its admitted daemon during graceful shutdown"
                );
            }
        }
        Ok(())
    }
}

pub fn install_signal_handler(shutdown: Arc<GracefulShutdown>) -> JoinHandle<()> {
    tokio::spawn(
        async move {
            tracing::info!("shutdown signal handler installed");
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("shutdown signal received");
                let _ = shutdown.shutdown().await;
            }
        }
        .instrument(tracing::info_span!("shutdown.signal_handler")),
    )
}

#[cfg(not(test))]
fn transaction_drain_wait() -> Duration {
    Duration::from_secs(2)
}

#[cfg(test)]
fn transaction_drain_wait() -> Duration {
    Duration::from_millis(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{
        create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, AgentRepo, AgentStatus,
        CreateAgent, CreateExecution, CreateHarnessSession, CreateProject, CreateRepo, CreateTask,
        DaemonRepo, DaemonStatus, ExecutionRepo, ExecutionStatus, HarnessSessionRepo,
        HarnessSessionStatus, ProjectRepo, RepoRepo, TaskLifecycleState, TaskRepo, UpdateAgent,
        UpdateDaemonReport, UpdateProject, UpsertDaemon, User, UserRepo,
    };
    use executors::{ExecutionContext, ExecutionResult, ExecutorError, TaskExecutor};
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingCancelExecutor {
        cancelled: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl TaskExecutor for RecordingCancelExecutor {
        async fn execute(
            &self,
            _ctx: ExecutionContext,
        ) -> std::result::Result<ExecutionResult, ExecutorError> {
            Err(ExecutorError::Other("not used".to_owned()))
        }

        async fn cancel(&self, execution_id: &str) -> std::result::Result<(), ExecutorError> {
            self.cancelled
                .lock()
                .expect("cancelled lock")
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

    async fn seed_shutdown_owner(db: &SqliteDb) -> String {
        if let Some(owner_id) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM user WHERE is_admin = 1 ORDER BY created_at, id LIMIT 1",
        )
        .fetch_optional(db.pool())
        .await
        .expect("shutdown fixture owner lookup succeeds")
        {
            return owner_id;
        }
        let now = now_rfc3339();
        let owner_id = new_uuid_v4();
        UserRepo::create_user(
            db,
            &User {
                id: owner_id.clone(),
                email: format!("{owner_id}@shutdown.test"),
                password_hash: "test-only".to_owned(),
                display_name: Some("Shutdown fixture owner".to_owned()),
                is_admin: true,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("durable shutdown fixture owner creates");
        owner_id
    }

    async fn seed_project_repo(db: &SqliteDb) -> (String, String) {
        let now = now_rfc3339();
        let owner_id = seed_shutdown_owner(db).await;
        let project_id = new_uuid_v4();
        let repo_id = new_uuid_v4();
        ProjectRepo::create(
            db,
            CreateProject {
                id: project_id.clone(),
                name: "Forge".to_owned(),
                settings: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(owner_id),
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
                updated_at: now,
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

    async fn seed_agent(db: &SqliteDb) -> String {
        let now = now_rfc3339();
        let owner_id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM user WHERE is_admin = 1 ORDER BY created_at, id LIMIT 1",
        )
        .fetch_one(db.pool())
        .await
        .expect("shutdown fixture owner exists");
        let daemon_id = new_uuid_v4();
        DaemonRepo::upsert_by_machine_id(
            db,
            UpsertDaemon {
                id: daemon_id.clone(),
                machine_id: format!("shutdown-machine-{daemon_id}"),
                hostname: "test-host".to_owned(),
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                agent_version: None,
                labels_json: "{}".to_owned(),
                status: DaemonStatus::Online,
                registration_token_hash: Some(format!("shutdown-registration-{daemon_id}")),
                owner_id: Some(owner_id.clone()),
                visibility: "account".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("daemon creates");
        DaemonRepo::update_report(
            db,
            UpdateDaemonReport {
                id: daemon_id.clone(),
                last_report_at: now.clone(),
                status: DaemonStatus::Online,
                detected_clis_json: r#"[{"kind":"shell","availability":"authenticated"}]"#
                    .to_owned(),
                labels_json: None,
                updated_at: now.clone(),
            },
        )
        .await
        .expect("daemon advertises its authenticated shell harness");

        let agent_id = new_uuid_v4();
        AgentRepo::create(
            db,
            CreateAgent {
                id: agent_id.clone(),
                name: "codex".to_owned(),
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
                status: AgentStatus::Idle,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: Some(owner_id),
                visibility: "account".to_owned(),
                prompt_template: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("agent creates");
        agent_id
    }

    async fn seed_task(
        db: &SqliteDb,
        project_id: String,
        repo_id: String,
        lifecycle_state: TaskLifecycleState,
    ) -> String {
        let now = now_rfc3339();
        TaskRepo::create(
            db,
            CreateTask {
                id: new_uuid_v4(),
                project_id,
                repo_id: Some(repo_id),
                parent_task_id: None,
                subtask_order: None,
                title: "Shutdown test".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                lifecycle_state,
                is_automation: false,
                priority: 0,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("task creates")
        .id
    }

    async fn seed_running_execution(
        db: &SqliteDb,
        task_id: String,
        agent_id: String,
        agent_session_id: Option<String>,
    ) -> db::Execution {
        let now = now_rfc3339();
        let agent = AgentRepo::get_by_id(db, &agent_id)
            .await
            .expect("agent loads")
            .expect("agent exists");
        let resolved_daemon_id = agent.daemon_id.clone().expect("agent daemon binding");
        let daemon = DaemonRepo::get_by_id(db, &resolved_daemon_id)
            .await
            .expect("agent daemon loads")
            .expect("agent daemon exists");
        let harness_session_id = if agent_session_id.is_some() {
            let id = new_uuid_v4();
            HarnessSessionRepo::create(
                db,
                CreateHarnessSession {
                    id: id.clone(),
                    agent_id: agent_id.clone(),
                    harness_kind: "shell".to_owned(),
                    external_session_id: agent_session_id.clone(),
                    profile_id: None,
                    profile_snapshot_json: "{}".to_owned(),
                    capabilities_snapshot_json: "{}".to_owned(),
                    workspace_id: None,
                    status: HarnessSessionStatus::Active,
                    predecessor_session_id: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                    last_activity_at: agent_session_id.as_ref().map(|_| now.clone()),
                },
            )
            .await
            .expect("exact pending HarnessSession creates");
            Some(id)
        } else {
            None
        };
        let execution = ExecutionRepo::create(
            db,
            CreateExecution {
                id: new_uuid_v4(),
                task_id,
                agent_id: Some(agent_id.clone()),
                actor_ref: harness_session_id
                    .as_ref()
                    .map(|_| db::ActorRef::Agent(agent_id)),
                purpose: Some(db::ExecutionPurpose::Implement),
                harness_session_id,
                role: "coder".to_owned(),
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
                        "agent_id": agent.id,
                        "agent_daemon_id": agent.daemon_id,
                        "resolved_daemon_id": resolved_daemon_id,
                        "daemon_authority": db::daemon_authority_snapshot(&agent, &daemon),
                        "executor_type": agent.executor_type,
                        "config": {}
                    })
                    .to_string(),
                ),
                workspace_id: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("execution creates");
        if let Some(external_session_id) = agent_session_id {
            ExecutionRepo::record_harness_session_result(
                db,
                &execution.id,
                &external_session_id,
                &now_rfc3339(),
            )
            .await
            .expect("exact HarnessSession receives the external session identity")
        } else {
            execution
        }
    }

    #[tokio::test]
    async fn shutdown_stops_accepting_work_and_recovers_in_progress_tasks() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let mut rx = event_bus.subscribe();
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent_id = seed_agent(&db).await;
        let task_id = seed_task(&db, project_id, repo_id, TaskLifecycleState::Active).await;
        let execution = seed_running_execution(&db, task_id.clone(), agent_id, None).await;
        let shutdown = GracefulShutdown::new(Arc::clone(&db), event_bus);

        assert!(shutdown.is_accepting_work());
        shutdown.shutdown().await.expect("shutdown succeeds");
        assert!(!shutdown.is_accepting_work());

        let task = TaskRepo::get_by_id(&*db, &task_id, false)
            .await
            .expect("task fetches")
            .expect("task exists");
        assert_eq!(task.lifecycle_state, TaskLifecycleState::Active);
        assert_eq!(task.error_annotation, None);
        let execution = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution fetches")
            .expect("execution exists");
        assert_eq!(execution.status, ExecutionStatus::Cancelled);

        let event = rx.recv().await.expect("recovery event");
        assert_eq!(event.event_type, "task.recovered");
        assert_eq!(event.entity_id, task_id);
        match event.context {
            EventContext::TaskRecovered { reason, .. } => assert_eq!(reason, SHUTDOWN_ERROR_TYPE),
            _ => panic!("expected task recovered event"),
        }
    }

    #[tokio::test]
    async fn shutdown_cancels_running_executor_processes_before_recovery() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent_id = seed_agent(&db).await;
        let task_id = seed_task(&db, project_id, repo_id, TaskLifecycleState::Active).await;
        let execution = seed_running_execution(&db, task_id, agent_id, None).await;
        let executor = Arc::new(RecordingCancelExecutor::default());

        GracefulShutdown::new(Arc::clone(&db), event_bus)
            .with_task_executor(executor.clone())
            .shutdown()
            .await
            .expect("shutdown succeeds");

        assert_eq!(
            executor
                .cancelled
                .lock()
                .expect("cancelled lock")
                .as_slice(),
            &[execution.id]
        );
    }

    #[tokio::test]
    async fn shutdown_cancels_remote_execution_on_frozen_daemon_after_agent_rebind() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let agent_id = seed_agent(&db).await;
        let agent = AgentRepo::get_by_id(&*db, &agent_id)
            .await
            .expect("Agent loads")
            .expect("Agent exists");
        let daemon_a = agent.daemon_id.clone().expect("daemon A is bound");
        sqlx::query("UPDATE daemon SET machine_id = ? WHERE id = ?")
            .bind("shutdown-frozen-host-a")
            .bind(&daemon_a)
            .execute(db.pool())
            .await
            .expect("daemon A becomes remote");

        let now = now_rfc3339();
        let daemon_b = new_uuid_v4();
        DaemonRepo::upsert_by_machine_id(
            &*db,
            UpsertDaemon {
                id: daemon_b.clone(),
                machine_id: "shutdown-current-host-b".to_owned(),
                hostname: "remote-b".to_owned(),
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
        .expect("daemon B creates");
        let task_id = seed_task(&db, project_id, repo_id, TaskLifecycleState::Active).await;
        let execution = seed_running_execution(&db, task_id, agent_id.clone(), None).await;
        let current_agent = AgentRepo::get_by_id(&*db, &agent_id)
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
        .expect("current Agent rebinds to daemon B");

        let registry =
            Arc::new(crate::daemon_transport::DaemonConnectionRegistry::without_handlers());
        let (connection_a, mut outbound_a) =
            crate::daemon_transport::DaemonConnection::new(daemon_a.clone());
        registry.register(daemon_a.clone(), connection_a);
        let (connection_b, mut outbound_b) =
            crate::daemon_transport::DaemonConnection::new(daemon_b.clone());
        registry.register(daemon_b.clone(), connection_b);
        let dispatcher_a = Arc::clone(&registry);
        let execution_id = execution.id.clone();
        let responder_a = tokio::spawn(async move {
            let api_types::DaemonFrame::Request { id, method, params } = outbound_a
                .recv()
                .await
                .expect("daemon A gets shutdown cancel")
            else {
                panic!("expected cancel request");
            };
            assert_eq!(method, api_types::METHOD_EXECUTION_CANCEL);
            assert_eq!(params["execution_id"], execution_id);
            dispatcher_a.dispatch_incoming(
                &daemon_a,
                api_types::DaemonFrame::Response {
                    id,
                    result: serde_json::json!({
                        "execution_id": execution_id,
                        "cancelled": true
                    }),
                },
            );
        });
        let responder_b = tokio::spawn(async move {
            match tokio::time::timeout(Duration::from_millis(150), outbound_b.recv()).await {
                Err(_) => {}
                Ok(None) => {}
                Ok(Some(frame)) => panic!("mutable daemon B received a frame: {frame:?}"),
            }
        });
        let local_executor = Arc::new(RecordingCancelExecutor::default());
        let task_service = Arc::new(
            crate::TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
                .with_daemon_connections(registry),
        );

        GracefulShutdown::new(Arc::clone(&db), event_bus)
            .with_task_executor(local_executor.clone())
            .with_task_service(task_service)
            .shutdown()
            .await
            .expect("shutdown succeeds");
        responder_a.await.expect("daemon A responder joins");
        responder_b.await.expect("daemon B observer joins");
        assert!(
            local_executor
                .cancelled
                .lock()
                .expect("local cancellation log lock")
                .is_empty(),
            "remote Execution is not cancelled through the local executor"
        );
    }

    #[tokio::test]
    async fn shutdown_keeps_active_task_with_resumable_execution() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let task_agent_id = seed_agent(&db).await;
        let task_id = seed_task(&db, project_id, repo_id, TaskLifecycleState::Active).await;
        let execution = seed_running_execution(
            &db,
            task_id.clone(),
            task_agent_id,
            Some("session-789".to_owned()),
        )
        .await;

        GracefulShutdown::new(Arc::clone(&db), event_bus)
            .shutdown()
            .await
            .expect("shutdown succeeds");

        let task = TaskRepo::get_by_id(&*db, &task_id, false)
            .await
            .expect("task fetches")
            .expect("task exists");
        assert_eq!(task.lifecycle_state, TaskLifecycleState::Active);

        let execution = ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("execution fetches")
            .expect("execution exists");
        assert_eq!(execution.status, ExecutionStatus::Cancelled);
        let external_session_id: Option<String> =
            sqlx::query_scalar("SELECT external_session_id FROM harness_session WHERE id = ?")
                .bind(
                    execution
                        .harness_session_id
                        .as_deref()
                        .expect("HarnessSession link"),
                )
                .fetch_one(db.pool())
                .await
                .expect("HarnessSession identity loads");
        assert_eq!(external_session_id.as_deref(), Some("session-789"));
    }

    #[tokio::test]
    async fn shutdown_leaves_non_running_tasks_unchanged() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let (project_id, repo_id) = seed_project_repo(&db).await;
        let task_id = seed_task(&db, project_id, repo_id, TaskLifecycleState::Ready).await;
        let version = TaskRepo::get_by_id(&*db, &task_id, false)
            .await
            .expect("task fetches")
            .expect("task exists")
            .version;

        GracefulShutdown::new(Arc::clone(&db), event_bus)
            .shutdown()
            .await
            .expect("shutdown succeeds");

        let task = TaskRepo::get_by_id(&*db, &task_id, false)
            .await
            .expect("task fetches")
            .expect("task exists");
        assert_eq!(task.lifecycle_state, TaskLifecycleState::Ready);
        assert_eq!(task.error_annotation, None);
        assert_eq!(task.version, version);
    }
}
