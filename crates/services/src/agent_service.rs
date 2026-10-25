use crate::{agent_capacity::has_running_execution_capacity, Result, ServiceError, TaskService};
use db::{
    new_uuid_v4, now_rfc3339, Agent, AgentListQuery, AgentRepo, AgentStatus, CreateAgent, Daemon,
    DaemonRepo, PageRequest, SortBy, SortOrder, SqliteDb, UpdateAgent,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use std::{fmt, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectiveStatus {
    Active,
    Deactivated,
    DaemonOffline,
    DaemonUnavailable,
    ConnectionDegraded,
    ConnectionUnavailable,
    Busy,
    Error,
    Paused,
}

impl EffectiveStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deactivated => "deactivated",
            Self::DaemonOffline => "daemon_offline",
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::ConnectionDegraded => "connection_degraded",
            Self::ConnectionUnavailable => "connection_unavailable",
            Self::Busy => "busy",
            Self::Error => "error",
            Self::Paused => "paused",
        }
    }
}

impl fmt::Display for EffectiveStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[tracing::instrument(skip(db, agent), fields(agent_id = %agent.id, daemon_id = ?agent.daemon_id))]
pub async fn compute_effective_status(db: &SqliteDb, agent: &Agent) -> Result<EffectiveStatus> {
    if agent.status == AgentStatus::Error {
        return Ok(EffectiveStatus::Error);
    }

    if agent.paused {
        return Ok(EffectiveStatus::Paused);
    }

    if agent.backend_kind.eq_ignore_ascii_case("native")
        || agent.executor_type.eq_ignore_ascii_case("embedded")
    {
        return Ok(EffectiveStatus::Deactivated);
    }

    if let Some(id) = agent.daemon_id.as_deref() {
        match DaemonRepo::get_by_id(db, id).await? {
            None => return Ok(EffectiveStatus::DaemonOffline),
            Some(daemon) if daemon.status == db::DaemonStatus::Offline => {
                return Ok(EffectiveStatus::DaemonOffline)
            }
            Some(daemon) if !executor_capability_present(&daemon, &agent.executor_type) => {
                return Ok(EffectiveStatus::Deactivated)
            }
            _ => {}
        }
    }
    if resolve_daemon_for_agent(db, agent).await.is_err() {
        return Ok(EffectiveStatus::DaemonUnavailable);
    }

    if !has_running_execution_capacity(db, agent).await? {
        return Ok(EffectiveStatus::Busy);
    }

    Ok(EffectiveStatus::Active)
}

fn executor_capability_present(daemon: &Daemon, executor_type: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(&daemon.detected_clis_json)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("kind").and_then(|v| v.as_str()) == Some(executor_type)
                    && item.get("availability").and_then(|v| v.as_str()) == Some("authenticated")
            })
        })
}

pub async fn require_agent_account(db: &SqliteDb, agent_id: &str, account_id: &str) -> Result<()> {
    let agent = AgentRepo::get_by_id(db, agent_id)
        .await?
        .ok_or_else(|| ServiceError::not_found("agent", agent_id.to_owned()))?;
    if agent.owner_id.as_deref() != Some(account_id) {
        return Err(ServiceError::invalid_operation(
            "Agent daemon authority belongs to another account",
        ));
    }
    Ok(())
}

pub async fn resolve_daemon_for_account(
    db: &SqliteDb,
    owner_id: &str,
    executor_type: &str,
    pinned_daemon_id: Option<&str>,
) -> Result<Daemon> {
    let user = db::UserRepo::get_user_by_id(db, owner_id)
        .await?
        .ok_or_else(|| ServiceError::invalid_operation("Agent owner no longer exists"))?;
    if let Some(daemon_id) = pinned_daemon_id {
        let daemon = DaemonRepo::get_by_id(db, daemon_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("daemon", daemon_id.to_owned()))?;
        if !db::daemon_execution_authorized(&daemon, Some(owner_id), user.is_admin, executor_type) {
            return Err(ServiceError::invalid_operation(
                "pinned daemon is not authorized or available for this account",
            ));
        }
        return Ok(daemon);
    }
    DaemonRepo::list_available_for_executor(db, executor_type, owner_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| {
            ServiceError::invalid_operation(
                "No authorized daemon with authenticated executor found",
            )
        })
}

pub async fn resolve_daemon_for_agent(db: &SqliteDb, agent: &Agent) -> Result<Daemon> {
    let owner_id = agent.owner_id.as_deref().ok_or_else(|| {
        ServiceError::invalid_operation("Agent has no verified daemon account authority")
    })?;
    resolve_daemon_for_account(
        db,
        owner_id,
        &agent.executor_type,
        agent.daemon_id.as_deref(),
    )
    .await
}

#[derive(Clone)]
pub struct AgentService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
}

impl AgentService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self { db, event_bus }
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        skip(self, name, config_json, capabilities_json),
        fields(agent_name = tracing::field::Empty, executor_type = %executor_type, daemon_id = ?daemon_id)
    )]
    pub async fn register(
        &self,
        name: impl Into<String>,
        description: Option<String>,
        executor_type: String,
        model: Option<String>,
        reasoning_effort: Option<String>,
        permission_policy: Option<String>,
        prompt_template: Option<String>,
        capabilities_json: String,
        config_json: String,
        credential_ref: Option<String>,
        daemon_id: Option<String>,
        max_concurrent: Option<i64>,
        heartbeat_interval: Option<i64>,
        max_missed: Option<i64>,
        is_default: bool,
        owner_id: Option<String>,
        visibility: Option<String>,
    ) -> Result<Agent> {
        let name = name.into();
        tracing::Span::current().record("agent_name", tracing::field::display(&name));
        if executor_type.trim().eq_ignore_ascii_case("embedded") {
            return Err(ServiceError::invalid_operation(
                "embedded/native Agent profiles are retired; create a new Agent bound to an available HarnessAdapter",
            ));
        }
        let owner_id = owner_id.filter(|id| !id.trim().is_empty()).ok_or_else(|| {
            ServiceError::invalid_operation("Agent registration requires an authenticated owner")
        })?;
        if db::UserRepo::get_user_by_id(&*self.db, &owner_id)
            .await?
            .is_none()
        {
            return Err(ServiceError::invalid_operation(
                "Agent owner does not exist",
            ));
        }
        if let Some(daemon_id) = daemon_id.as_deref() {
            resolve_daemon_for_account(&self.db, &owner_id, &executor_type, Some(daemon_id))
                .await?;
        }
        validate_positive("max_concurrent", max_concurrent.unwrap_or(1))?;
        validate_positive("heartbeat_interval", heartbeat_interval.unwrap_or(30))?;
        validate_positive("max_missed", max_missed.unwrap_or(3))?;

        let now = now_rfc3339();
        let agent = AgentRepo::create(
            &*self.db,
            CreateAgent {
                id: new_uuid_v4(),
                name,
                description,
                executor_type,
                model,
                reasoning_effort,
                permission_policy,
                prompt_template,
                capabilities_json,
                config_json,
                credential_ref,
                daemon_id,
                max_concurrent_tasks: max_concurrent.unwrap_or(1),
                heartbeat_interval_seconds: heartbeat_interval.unwrap_or(30),
                max_missed_heartbeats: max_missed.unwrap_or(3),
                status: AgentStatus::Idle,
                last_heartbeat_at: Some(now.clone()),
                is_default,
                paused: false,
                owner_id: Some(owner_id),
                visibility: visibility.unwrap_or_else(|| "global".to_owned()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await?;

        self.publish(ForgeEvent {
            event_type: "agent.created".to_owned(),
            entity_id: agent.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::AgentCreated {
                name: agent.name.clone(),
            },
        });

        Ok(agent)
    }

    #[tracing::instrument(
        skip(self, agent_id),
        fields(agent_id = tracing::field::Empty, status = %status, version = version)
    )]
    pub async fn update_status(
        &self,
        agent_id: impl Into<String>,
        status: AgentStatus,
        version: i64,
    ) -> Result<Agent> {
        let agent_id = agent_id.into();
        tracing::Span::current().record("agent_id", tracing::field::display(&agent_id));
        validate_required("agent_id", &agent_id)?;
        let old = AgentRepo::get_by_id(&*self.db, &agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;

        let agent = AgentRepo::update(
            &*self.db,
            UpdateAgent {
                id: agent_id,
                expected_version: version,
                name: None,
                description: None,
                max_concurrent_tasks: None,
                heartbeat_interval_seconds: None,
                max_missed_heartbeats: None,
                status: Some(status),
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
            event_type: "agent.status_changed".to_owned(),
            entity_id: agent.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::AgentStatusChanged {
                old_status: old.status.to_string(),
                new_status: agent.status.to_string(),
            },
        });

        Ok(agent)
    }

    #[tracing::instrument(skip(self, agent_id), fields(agent_id = tracing::field::Empty))]
    pub async fn update_heartbeat(&self, agent_id: impl Into<String>) -> Result<()> {
        let agent_id = agent_id.into();
        tracing::Span::current().record("agent_id", tracing::field::display(&agent_id));
        validate_required("agent_id", &agent_id)?;
        let agent = AgentRepo::get_by_id(&*self.db, &agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
        AgentRepo::update(
            &*self.db,
            UpdateAgent {
                id: agent_id,
                expected_version: agent.version,
                name: None,
                description: None,
                max_concurrent_tasks: None,
                heartbeat_interval_seconds: None,
                max_missed_heartbeats: None,
                status: None,
                last_heartbeat_at: Some(Some(now_rfc3339())),
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
        Ok(())
    }

    #[tracing::instrument(
        skip(self, capabilities_filter),
        fields(capabilities_count = capabilities_filter.as_ref().map(Vec::len).unwrap_or_default())
    )]
    pub async fn list_available(
        &self,
        capabilities_filter: Option<Vec<String>>,
    ) -> Result<Vec<Agent>> {
        let page = AgentRepo::list(
            &*self.db,
            AgentListQuery {
                status: Some(AgentStatus::Idle),
                executor_type: None,
                capabilities: capabilities_filter.unwrap_or_default(),
                harness_only: true,
                page: PageRequest {
                    cursor: None,
                    limit: 500,
                    include_total: false,
                    sort_by: SortBy::CreatedAt,
                    sort_order: SortOrder::Asc,
                },
            },
        )
        .await?;

        let mut available = Vec::new();
        for agent in page.items {
            if compute_effective_status(&self.db, &agent).await? == EffectiveStatus::Active
                && has_running_execution_capacity(&self.db, &agent).await?
            {
                available.push(agent);
            }
        }
        Ok(available)
    }

    #[tracing::instrument(skip(self, agent_id), fields(agent_id = tracing::field::Empty))]
    pub async fn archive(&self, agent_id: impl Into<String>) -> Result<()> {
        let agent_id = agent_id.into();
        tracing::Span::current().record("agent_id", tracing::field::display(&agent_id));
        validate_required("agent_id", &agent_id)?;
        let task_service = TaskService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let mut transaction = self.db.pool().begin().await?;
        let role_events = task_service
            .on_agent_deleted_in_tx(&mut transaction, &agent_id)
            .await?;
        let now = now_rfc3339();
        let result = sqlx::query(
            "UPDATE agent_identity
             SET archived_at = ?, paused = 1, is_default = 0, status = 'offline',
                 version = version + 1, updated_at = ?
             WHERE id = ? AND archived_at IS NULL",
        )
        .bind(&now)
        .bind(&now)
        .bind(&agent_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(ServiceError::not_found("agent", agent_id));
        }
        transaction.commit().await?;
        task_service.publish_role_sweep_events(role_events);
        self.publish(ForgeEvent {
            event_type: "agent.archived".to_owned(),
            entity_id: agent_id,
            timestamp: event_timestamp(),
            context: EventContext::AgentArchived {},
        });
        Ok(())
    }

    fn publish(&self, event: ForgeEvent) {
        self.event_bus.publish(event);
    }
}

fn validate_required(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(ServiceError::invalid_operation(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn validate_positive(field: &str, value: i64) -> Result<()> {
    if value <= 0 {
        return Err(ServiceError::invalid_operation(format!(
            "{field} must be positive"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{
        create_sqlite_pool, run_migrations, CreateExecution, CreateProject, CreateRepo,
        CreateRoleMembership, CreateTask, CreateTaskRole, DaemonStatus, ExecutionRepo,
        ExecutionStatus, ProjectRepo, RepoRepo, RoleMembershipRepo, TaskRepo, TaskRoleRepo,
        UpdateProject, UpsertDaemon, User, UserRepo,
    };

    async fn sqlite_db() -> SqliteDb {
        let pool = create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        run_migrations(&pool).await.expect("migrations run");
        let db = SqliteDb::new(pool);
        let now = now_rfc3339();
        UserRepo::create_user(
            &db,
            &User {
                id: "owner".to_owned(),
                email: "owner@test.invalid".to_owned(),
                password_hash: "x".to_owned(),
                display_name: None,
                is_admin: true,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("test owner creates");
        db
    }

    async fn seed_daemon(db: &SqliteDb) -> String {
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
                owner_id: Some("owner".to_owned()),
                visibility: "account".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("daemon creates");

        DaemonRepo::update_report(
            db,
            db::UpdateDaemonReport {
                id: daemon_id.clone(),
                detected_clis_json: r#"[{"kind":"shell","availability":"authenticated"}]"#
                    .to_owned(),
                labels_json: None,
                status: DaemonStatus::Online,
                last_report_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("test daemon shell capability reports");
        daemon_id
    }

    async fn seed_effective_agent(
        db: &SqliteDb,
        agent_status: AgentStatus,
        daemon_status: DaemonStatus,
        detected_clis_json: &str,
        max_concurrent_tasks: i64,
    ) -> Agent {
        let daemon_id = seed_daemon(db).await;
        DaemonRepo::update_report(
            db,
            db::UpdateDaemonReport {
                id: daemon_id.clone(),
                detected_clis_json: detected_clis_json.to_owned(),
                labels_json: None,
                status: daemon_status,
                last_report_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("daemon report updates");

        let now = now_rfc3339();
        AgentRepo::create(
            db,
            CreateAgent {
                id: new_uuid_v4(),
                name: "shell".to_owned(),
                description: None,
                executor_type: "shell".to_owned(),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                capabilities_json: "[]".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: None,
                daemon_id: Some(daemon_id),
                max_concurrent_tasks,
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status: agent_status,
                last_heartbeat_at: Some(now.clone()),
                is_default: false,
                paused: false,
                owner_id: Some("owner".to_owned()),
                visibility: "account".to_owned(),
                prompt_template: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("agent creates")
    }

    fn detached_agent(status: AgentStatus) -> Agent {
        Agent {
            id: new_uuid_v4(),
            name: "detached".to_owned(),
            description: None,
            profile_id: new_uuid_v4(),
            backend_kind: "cli".to_owned(),
            executor_type: "shell".to_owned(),
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            capabilities_json: "[]".to_owned(),
            tool_policy_json: "{}".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some("missing-daemon".to_owned()),
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some("owner".to_owned()),
            visibility: "account".to_owned(),
            prompt_template: None,
            version: 1,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        }
    }

    async fn seed_active_task(db: &SqliteDb, agent_id: &str) {
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
                owner_id: Some("owner".to_owned()),
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
        let task = TaskRepo::create(
            db,
            CreateTask {
                id: new_uuid_v4(),
                project_id,
                repo_id: Some(repo_id),
                parent_task_id: None,
                subtask_order: None,
                title: "Active task".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                lifecycle_state: db::TaskLifecycleState::Active,
                is_automation: false,
                priority: 0,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("task creates");
        let task_role_id = new_uuid_v4();
        TaskRoleRepo::create(
            db,
            CreateTaskRole {
                id: task_role_id.clone(),
                task_id: task.id.clone(),
                role: "implementer".to_owned(),
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
                task_role_id,
                actor_kind: db::ActorKind::Agent,
                actor_id: agent_id.to_owned(),
                status: db::RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("role membership creates");
        ExecutionRepo::create(
            db,
            CreateExecution {
                id: new_uuid_v4(),
                task_id: task.id,
                agent_id: Some(agent_id.to_owned()),
                actor_ref: None,
                purpose: None,
                harness_session_id: None,
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
                executor_config_snapshot_json: None,
                workspace_id: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("execution creates");
    }

    #[tokio::test]
    async fn register_update_heartbeat_list_and_archive_agent() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let service = AgentService::new(Arc::clone(&db), Arc::clone(&event_bus));
        let mut rx = event_bus.subscribe();
        let daemon_id = seed_daemon(&db).await;

        let agent = service
            .register(
                "codex",
                None,
                "shell".to_owned(),
                None,
                None,
                None,
                None,
                r#"["rust","sqlite"]"#.to_owned(),
                "{}".to_owned(),
                None,
                Some(daemon_id),
                Some(2),
                None,
                None,
                false,
                Some("owner".to_owned()),
                Some("account".to_owned()),
            )
            .await
            .expect("agent registers");
        assert_eq!(agent.status, AgentStatus::Idle);
        assert_eq!(agent.max_concurrent_tasks, 2);
        assert_eq!(rx.recv().await.unwrap().event_type, "agent.created");

        let updated = service
            .update_status(agent.id.clone(), AgentStatus::Busy, agent.version)
            .await
            .expect("agent status updates");
        assert_eq!(updated.status, AgentStatus::Busy);
        assert_eq!(rx.recv().await.unwrap().event_type, "agent.status_changed");

        service
            .update_heartbeat(updated.id.clone())
            .await
            .expect("heartbeat updates");
        let reloaded = AgentRepo::get_by_id(&*db, &updated.id)
            .await
            .expect("agent loads")
            .expect("agent exists");
        assert!(reloaded.last_heartbeat_at.is_some());

        let available = service
            .list_available(Some(vec!["rust".to_owned()]))
            .await
            .expect("available agents list");
        assert!(available.is_empty());

        let idle = service
            .update_status(reloaded.id.clone(), AgentStatus::Idle, reloaded.version)
            .await
            .expect("agent returns idle");
        let available = service
            .list_available(Some(vec!["rust".to_owned()]))
            .await
            .expect("available agents list");
        assert_eq!(available, vec![idle.clone()]);

        service.archive(idle.id).await.expect("agent archives");
        assert_eq!(rx.recv().await.unwrap().event_type, "agent.status_changed");
        assert_eq!(rx.recv().await.unwrap().event_type, "agent.archived");
    }

    #[tokio::test]
    async fn register_validates_positive_fields() {
        let db = Arc::new(sqlite_db().await);
        let event_bus = Arc::new(EventBus::new(16));
        let service = AgentService::new(db, event_bus);

        let result = service
            .register(
                "codex",
                None,
                "shell".to_owned(),
                None,
                None,
                None,
                None,
                "[]".to_owned(),
                "{}".to_owned(),
                None,
                None,
                Some(0),
                None,
                None,
                false,
                None,
                None,
            )
            .await;
        assert!(matches!(result, Err(ServiceError::InvalidOperation { .. })));
    }

    #[tokio::test]
    async fn effective_status_is_error_when_agent_status_is_error() {
        let db = sqlite_db().await;
        let agent = detached_agent(AgentStatus::Error);

        let status = compute_effective_status(&db, &agent)
            .await
            .expect("status computes");

        assert_eq!(status, EffectiveStatus::Error);
        assert_eq!(status.as_str(), "error");
        assert_eq!(status.to_string(), "error");
    }

    #[tokio::test]
    async fn effective_status_is_daemon_offline_when_daemon_missing() {
        let db = sqlite_db().await;
        let agent = detached_agent(AgentStatus::Idle);

        let status = compute_effective_status(&db, &agent)
            .await
            .expect("status computes");

        assert_eq!(status, EffectiveStatus::DaemonOffline);
    }

    #[tokio::test]
    async fn effective_status_is_daemon_offline_when_daemon_offline() {
        let db = sqlite_db().await;
        let agent = seed_effective_agent(
            &db,
            AgentStatus::Idle,
            DaemonStatus::Offline,
            r#"[{"kind":"shell","availability":"authenticated"}]"#,
            1,
        )
        .await;

        let status = compute_effective_status(&db, &agent)
            .await
            .expect("status computes");

        assert_eq!(status, EffectiveStatus::DaemonOffline);
    }

    #[tokio::test]
    async fn effective_status_is_deactivated_without_authenticated_cli() {
        let db = sqlite_db().await;
        let agent = seed_effective_agent(
            &db,
            AgentStatus::Idle,
            DaemonStatus::Online,
            r#"[{"kind":"shell","availability":"installed"}]"#,
            1,
        )
        .await;

        let status = compute_effective_status(&db, &agent)
            .await
            .expect("status computes");

        assert_eq!(status, EffectiveStatus::Deactivated);
    }

    #[tokio::test]
    async fn effective_status_is_busy_at_capacity() {
        let db = sqlite_db().await;
        let agent = seed_effective_agent(
            &db,
            AgentStatus::Idle,
            DaemonStatus::Online,
            r#"[{"kind":"shell","availability":"authenticated"}]"#,
            1,
        )
        .await;
        seed_active_task(&db, &agent.id).await;

        let status = compute_effective_status(&db, &agent)
            .await
            .expect("status computes");

        assert_eq!(status, EffectiveStatus::Busy);
    }

    #[tokio::test]
    async fn effective_status_is_paused() {
        let db = sqlite_db().await;
        let mut agent = seed_effective_agent(
            &db,
            AgentStatus::Idle,
            DaemonStatus::Online,
            r#"[{"kind":"shell","availability":"authenticated"}]"#,
            1,
        )
        .await;
        agent.paused = true;

        let status = compute_effective_status(&db, &agent)
            .await
            .expect("status computes");

        assert_eq!(status, EffectiveStatus::Paused);
    }

    #[tokio::test]
    async fn effective_status_is_active_when_authenticated_and_not_busy() {
        let db = sqlite_db().await;
        let agent = seed_effective_agent(
            &db,
            AgentStatus::Idle,
            DaemonStatus::Online,
            r#"[{"kind":"shell","availability":"authenticated"}]"#,
            1,
        )
        .await;

        let status = compute_effective_status(&db, &agent)
            .await
            .expect("status computes");

        assert_eq!(status, EffectiveStatus::Active);
    }

    #[tokio::test]
    async fn daemon_authority_is_account_scoped_and_unowned_history_fails_closed() {
        let db = sqlite_db().await;
        UserRepo::create_user(
            &db,
            &User {
                id: "account-b".to_owned(),
                email: "b@test.invalid".to_owned(),
                password_hash: "x".to_owned(),
                display_name: None,
                is_admin: false,
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("second account creates");
        let agent_a = seed_effective_agent(
            &db,
            AgentStatus::Idle,
            DaemonStatus::Online,
            r#"[{"kind":"shell","availability":"authenticated"}]"#,
            1,
        )
        .await;
        let daemon_id = agent_a.daemon_id.clone().expect("pinned daemon");
        assert!(
            resolve_daemon_for_agent(&db, &agent_a).await.is_ok(),
            "owner uses own daemon"
        );

        sqlx::query("UPDATE agent_identity SET owner_id = 'account-b' WHERE id = ?")
            .bind(&agent_a.id)
            .execute(db.pool())
            .await
            .expect("transfer test Agent");
        let agent_b = AgentRepo::get_by_id(&db, &agent_a.id)
            .await
            .expect("Agent read")
            .unwrap();
        assert!(
            resolve_daemon_for_agent(&db, &agent_b).await.is_err(),
            "pinned foreign daemon is denied"
        );
        assert_eq!(
            compute_effective_status(&db, &agent_b).await.unwrap(),
            EffectiveStatus::DaemonUnavailable
        );

        let now = now_rfc3339();
        let agent_b_auto = AgentRepo::create(
            &db,
            CreateAgent {
                id: new_uuid_v4(),
                name: "account-b-auto".to_owned(),
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
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status: AgentStatus::Idle,
                last_heartbeat_at: Some(now.clone()),
                is_default: false,
                paused: false,
                owner_id: Some("account-b".to_owned()),
                visibility: "account".to_owned(),
                prompt_template: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("unpinned Agent for account B creates");
        assert!(
            resolve_daemon_for_agent(&db, &agent_b_auto).await.is_err(),
            "automatic route cannot select account A"
        );
        assert!(
            DaemonRepo::list_available_for_executor(&db, "shell", "account-b")
                .await
                .unwrap()
                .is_empty()
        );

        sqlx::query("UPDATE daemon SET owner_id = NULL, visibility = 'global' WHERE id = ?")
            .bind(&daemon_id)
            .execute(db.pool())
            .await
            .expect("make historical daemon unowned");
        assert!(
            resolve_daemon_for_agent(&db, &agent_a).await.is_err(),
            "global metadata does not authorize an orphan"
        );
    }

    #[tokio::test]
    async fn embedded_shell_requires_durable_admin_account() {
        let db = sqlite_db().await;
        UserRepo::create_user(
            &db,
            &User {
                id: "other-admin".to_owned(),
                email: "other-admin@test.invalid".to_owned(),
                password_hash: "x".to_owned(),
                display_name: None,
                is_admin: true,
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("second admin creates");
        let agent = seed_effective_agent(
            &db,
            AgentStatus::Idle,
            DaemonStatus::Online,
            r#"[{"kind":"shell","availability":"authenticated"}]"#,
            1,
        )
        .await;
        let daemon_id = agent.daemon_id.as_deref().unwrap();
        sqlx::query("UPDATE daemon SET machine_id = ? WHERE id = ?")
            .bind(crate::embedded_daemon::embedded_machine_id())
            .bind(daemon_id)
            .execute(db.pool())
            .await
            .expect("mark server embedded host");
        assert!(
            resolve_daemon_for_agent(&db, &agent).await.is_ok(),
            "local admin owner can use embedded host"
        );
        let daemon = DaemonRepo::get_by_id(&db, daemon_id)
            .await
            .expect("embedded daemon loads")
            .expect("embedded daemon exists");
        assert!(
            !db::daemon_execution_authorized(&daemon, Some("other-admin"), true, "shell"),
            "another admin cannot use the locally owned embedded shell implicitly"
        );
        sqlx::query("UPDATE user SET is_admin = 0 WHERE id = 'owner'")
            .execute(db.pool())
            .await
            .expect("revoke explicit host trust");
        assert!(
            resolve_daemon_for_agent(&db, &agent).await.is_err(),
            "non-admin cannot use embedded shell"
        );
    }
}
