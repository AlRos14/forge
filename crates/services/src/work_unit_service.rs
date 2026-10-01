use std::{path::Path, path::PathBuf, sync::Arc};

use db::{
    new_uuid_v4, now_rfc3339, ActorRef, AddWorkUnitDependency, AgentRepo, AllocateWorkUnit,
    CreateDomainEvent, CreateExecution, CreateWorkUnit, CreateWorkUnitExecution,
    CreateWorkUnitIntegration, Execution, ExecutionRepo, ExecutionStatus,
    RecordWorkUnitIntegration, RepoRepo, SqliteDb, Task, TransitionWorkUnit, UpdateWorkUnit,
    WorkUnit, WorkUnitDependency, WorkUnitExecutionRepo, WorkUnitIntegration,
    WorkUnitIntegrationOutcome, WorkUnitProvenance, WorkUnitRepo, WorkUnitStatus,
    WorkUnitWorkspaceRepo, Workspace, WorkspaceRepo, WorkspaceScopeKind, WorkspaceStatus,
};
use events::EventBus;
use workspace::{RepoCacheLockManager, WorkspaceManager};

use crate::{
    collaboration_service::{CollaborationActorSource, CollaborationService},
    domain_event_service::DomainEventService,
    task_integration_operation::TaskIntegrationOperationManager,
    task_service::workspace::{prepare_integration_workspace_for_work_unit, resolve_repo_source},
    workspace_execution_lock::WorkspaceExecutionLockManager,
    Result, ServiceError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitReadiness {
    pub runnable: bool,
    pub ready_for_allocation: bool,
    pub active_execution_ids: Vec<String>,
    pub unsatisfied_dependency_ids: Vec<String>,
    pub awaiting_integration: bool,
}

#[derive(Debug, Clone)]
pub struct CreateWorkUnitInput {
    pub task_id: String,
    pub title: String,
    pub scope: String,
    pub role: String,
    pub parent_work_unit_id: Option<String>,
    pub assigned_actor: Option<ActorRef>,
    pub requires_integration: bool,
    pub provenance: Option<WorkUnitProvenance>,
}

#[derive(Clone)]
pub struct WorkUnitService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    collaboration: CollaborationService,
    domain_events: DomainEventService,
    workspace_root: PathBuf,
    repo_cache_locks: Arc<RepoCacheLockManager>,
    integration_locks: Arc<WorkspaceExecutionLockManager>,
    integration_operations: TaskIntegrationOperationManager,
}

impl WorkUnitService {
    pub fn new(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        workspace_root: PathBuf,
        repo_cache_locks: Arc<RepoCacheLockManager>,
        integration_locks: Arc<WorkspaceExecutionLockManager>,
    ) -> Self {
        let integration_operations =
            TaskIntegrationOperationManager::new(Arc::clone(&db), workspace_root.clone());
        Self {
            collaboration: CollaborationService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            domain_events: DomainEventService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            event_bus: Arc::clone(&event_bus),
            db,
            workspace_root,
            repo_cache_locks,
            integration_locks,
            integration_operations,
        }
    }

    pub async fn create(
        &self,
        source: CollaborationActorSource,
        input: CreateWorkUnitInput,
    ) -> Result<WorkUnit> {
        self.create_with_id(source, input, new_uuid_v4()).await
    }

    pub(crate) async fn create_with_id(
        &self,
        source: CollaborationActorSource,
        input: CreateWorkUnitInput,
        id: String,
    ) -> Result<WorkUnit> {
        let CreateWorkUnitInput {
            task_id,
            title,
            scope,
            role,
            parent_work_unit_id,
            assigned_actor,
            requires_integration,
            provenance,
        } = input;
        let (task, actor) = self
            .collaboration
            .authorize_source(&task_id, &source)
            .await?;
        if let Some(existing) = WorkUnitRepo::get_by_id(&*self.db, &id).await? {
            if existing.task_id == task_id
                && existing.title == title
                && existing.scope == scope
                && existing.role == role
                && existing.parent_work_unit_id == parent_work_unit_id
                && existing.assigned_actor == assigned_actor
                && existing.requires_integration == requires_integration
                && existing.provenance == provenance
                && existing.created_by == actor
            {
                return Ok(existing);
            }
            return Err(ServiceError::conflict(
                "WorkUnit idempotency key conflicts with an existing record",
            ));
        }
        if requires_integration && task.repo_id.is_none() {
            return Err(invalid(
                "repository WorkUnit requires a repository-bound Task",
            ));
        }
        let _integration_guard = self
            .integration_locks
            .acquire(&format!("task-integration:{}", task.id))
            .await;
        let operation = self
            .integration_operations
            .acquire(
                &task.id,
                db::TaskIntegrationOperationKind::WorkUnitCreate,
                &new_uuid_v4(),
            )
            .await?;
        let result = async {
        let legacy_authority_active: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM execution e
                 WHERE e.task_id = ? AND e.status = 'running' AND e.work_unit_id IS NULL
             ) OR EXISTS(
                 SELECT 1 FROM workspace_lease l
                 WHERE l.task_id = ? AND l.status = 'active' AND l.work_unit_id IS NULL
             ) OR EXISTS(
                 SELECT 1 FROM task_terminal_session s
                 JOIN workspace_scope ws ON ws.workspace_id = s.workspace_id
                 WHERE s.task_id = ? AND s.status IN ('starting', 'running')
                   AND ws.scope_kind = 'integration'
             )",
        )
        .bind(&task.id)
        .bind(&task.id)
        .bind(&task.id)
        .fetch_one(self.db.pool())
        .await?;
        if legacy_authority_active != 0 {
            return Err(ServiceError::conflict(
                "Task still has active Task-scoped repository authority; finish it before creating WorkUnits",
            ));
        }
        if let Some(integration_workspace) =
            WorkspaceRepo::get_by_task_id(&*self.db, &task.id).await?
        {
            if integration_workspace.status == WorkspaceStatus::Cleaning {
                return Err(invalid(
                    "Task integration Workspace is being cleaned and cannot be adopted by WorkUnits",
                ));
            }
            if integration_workspace.cleanup_after.is_some() {
                WorkspaceRepo::set_cleanup_after(
                    &*self.db,
                    &integration_workspace.id,
                    None,
                    &now_rfc3339(),
                )
                .await?;
            }
        }
        if let Some(parent_id) = parent_work_unit_id.as_deref() {
            self.ensure_work_unit_in_task(&task, parent_id).await?;
        }
        if let Some(provenance) = provenance.as_ref() {
            match provenance {
                WorkUnitProvenance::Actor(actor) => {
                    let exists = match actor {
                        ActorRef::Human(id) => {
                            db::UserRepo::get_user_by_id(&*self.db, id).await?.is_some()
                        }
                        ActorRef::Agent(id) => {
                            AgentRepo::get_by_id(&*self.db, id).await?.is_some()
                        }
                    };
                    if !exists {
                        return Err(invalid("Actor provenance must reference an existing Actor"));
                    }
                }
                WorkUnitProvenance::WorkUnit(id) => {
                    self.ensure_work_unit_in_task(&task, id).await?;
                }
                WorkUnitProvenance::Artifact(id) => {
                    let artifact_task = db::CollaborationRepo::get_artifact_task_id(&*self.db, id)
                        .await?
                        .ok_or_else(|| not_found("artifact", id))?;
                    if artifact_task != task.id {
                        return Err(not_found("artifact", id));
                    }
                }
                WorkUnitProvenance::External(_) => {}
                WorkUnitProvenance::LegacyActor(_) => {
                    return Err(invalid(
                        "unresolved historical Actor provenance cannot be written",
                    ));
                }
            }
        }
        let now = now_rfc3339();
        let mut event = self.event(
            "work_unit.created",
            "work_unit",
            &id,
            &task,
            &actor,
            serde_json::json!({"task_id": task.id, "work_unit_id": id, "version": 1, "status": "open"}),
            &now,
        );
        if let CollaborationActorSource::Execution(execution_id) = &source {
            let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
                .await?
                .ok_or_else(|| not_found("execution", execution_id.clone()))?;
            if execution.task_id != task.id || execution.actor_ref() != Some(actor.clone()) {
                return Err(ServiceError::AuthorizationDenied {
                    message: "WorkUnit action source does not match its exact Task Actor".to_owned(),
                });
            }
            if execution.role == "orchestrator"
                && execution.purpose == Some(db::ExecutionPurpose::Orchestrate)
            {
                let start_event = db::DomainEventRepo::get_event_by_dedupe(
                    &*self.db,
                    &format!("execution.started:{}", execution.id),
                )
                .await?
                .filter(|event| {
                    event.entity_type == "execution"
                        && event.entity_id == execution.id
                        && event.scope_type == "task"
                        && event.scope_id == task.id
                })
                .ok_or_else(|| invalid("orchestrator WorkUnit action has no durable Execution start event"))?;
                let causation_depth = start_event.causation_depth.saturating_add(1);
                if causation_depth > 16 {
                    return Err(invalid("orchestrator WorkUnit action exceeds causation depth limit"));
                }
                event.correlation_id = start_event.correlation_id;
                event.causation_id = Some(start_event.id);
                event.causation_depth = causation_depth;
            }
        }
        let write = WorkUnitRepo::create(
            &*self.db,
            CreateWorkUnit {
                id,
                task_id: task.id,
                parent_work_unit_id,
                title,
                scope,
                role,
                assigned_actor,
                requires_integration,
                provenance,
                created_by: actor,
                created_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
        }
        .await;
        let finish_result = operation
            .finish(if result.is_ok() {
                db::TaskIntegrationOperationStatus::Succeeded
            } else {
                db::TaskIntegrationOperationStatus::Failed
            })
            .await;
        match (result, finish_result) {
            (Ok(unit), Ok(())) => Ok(unit),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), _) => Err(error),
        }
    }

    pub async fn get(
        &self,
        source: CollaborationActorSource,
        work_unit_id: &str,
    ) -> Result<WorkUnit> {
        let task_id = WorkUnitRepo::get_task_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| not_found("work_unit", work_unit_id))?;
        self.collaboration
            .authorize_source(&task_id, &source)
            .await
            .map_err(|_| not_found("work_unit", work_unit_id))?;
        WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| not_found("work_unit", work_unit_id))
    }

    pub async fn list(
        &self,
        source: CollaborationActorSource,
        task_id: &str,
    ) -> Result<Vec<WorkUnit>> {
        self.collaboration
            .authorize_source(task_id, &source)
            .await?;
        Ok(WorkUnitRepo::list_by_task(&*self.db, task_id).await?)
    }

    pub async fn update(
        &self,
        source: CollaborationActorSource,
        id: &str,
        expected_version: i64,
        title: Option<String>,
        scope: Option<String>,
        requires_integration: Option<bool>,
    ) -> Result<WorkUnit> {
        let (task, actor) = self.authorize_work_unit(&source, id).await?;
        let unit = WorkUnitRepo::get_by_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("work_unit", id))?;
        if unit.version != expected_version {
            return Err(db::DbError::VersionConflict.into());
        }
        if requires_integration.is_some_and(|next| next != unit.requires_integration)
            && (WorkUnitWorkspaceRepo::get_by_work_unit_id(&*self.db, id)
                .await?
                .is_some()
                || sqlx::query_scalar::<_, i64>(
                    "SELECT EXISTS(SELECT 1 FROM execution WHERE work_unit_id = ?)",
                )
                .bind(id)
                .fetch_one(self.db.pool())
                .await?
                    != 0)
        {
            return Err(ServiceError::conflict(
                "WorkUnit repository mode is fixed after its first Workspace or Execution",
            ));
        }
        let now = now_rfc3339();
        let next_version = expected_version + 1;
        let event = self.event(
            "work_unit.updated",
            "work_unit",
            id,
            &task,
            &actor,
            serde_json::json!({"task_id": task.id, "work_unit_id": id, "version": next_version}),
            &now,
        );
        let write = WorkUnitRepo::update(
            &*self.db,
            UpdateWorkUnit {
                id: id.to_owned(),
                expected_version,
                title,
                scope,
                parent_work_unit_id: None,
                requires_integration,
                updated_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn allocate(
        &self,
        source: CollaborationActorSource,
        id: &str,
        expected_version: i64,
        role: String,
        assigned_actor: Option<ActorRef>,
    ) -> Result<WorkUnit> {
        let (task, actor) = self.authorize_work_unit(&source, id).await?;
        let now = now_rfc3339();
        let event = self.event(
            "work_unit.allocation_changed", "work_unit", id, &task, &actor,
            serde_json::json!({"task_id": task.id, "work_unit_id": id, "version": expected_version + 1}), &now,
        );
        let write = WorkUnitRepo::allocate(
            &*self.db,
            AllocateWorkUnit {
                id: id.to_owned(),
                expected_version,
                role,
                assigned_actor,
                updated_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn transition(
        &self,
        source: CollaborationActorSource,
        id: &str,
        expected_version: i64,
        status: WorkUnitStatus,
    ) -> Result<WorkUnit> {
        let (task, actor) = self.authorize_work_unit(&source, id).await?;
        let unit = WorkUnitRepo::get_by_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("work_unit", id))?;
        let active = WorkUnitRepo::list_active_execution_ids(&*self.db, id).await?;
        if !active.is_empty() {
            return Err(invalid(
                "WorkUnit with a running Execution cannot be completed or cancelled",
            ));
        }
        if status == WorkUnitStatus::Completed {
            if WorkUnitRepo::list_dependencies(&*self.db, id)
                .await?
                .iter()
                .any(|dependency| !dependency.satisfied)
            {
                return Err(invalid(
                    "WorkUnit cannot complete before its dependencies are satisfied",
                ));
            }
            if unit.requires_integration {
                let has_result: i64 = sqlx::query_scalar(
                    "SELECT EXISTS(
                        SELECT 1 FROM execution e
                        JOIN workspace_scope s
                          ON s.workspace_id = e.workspace_id AND s.task_id = e.task_id
                        WHERE e.work_unit_id = ? AND e.task_id = ?
                          AND e.work_unit_version = ?
                          AND e.status = 'completed' AND e.after_sha IS NOT NULL
                          AND s.scope_kind = 'work_unit' AND s.work_unit_id = e.work_unit_id
                    )",
                )
                .bind(id)
                .bind(&task.id)
                .bind(unit.version)
                .fetch_one(self.db.pool())
                .await?;
                if has_result == 0 {
                    return Err(invalid(
                        "Repository WorkUnit requires a completed Execution result SHA",
                    ));
                }
            }
        }
        let now = now_rfc3339();
        let event_name = match status {
            WorkUnitStatus::Completed => "work_unit.completed",
            WorkUnitStatus::Cancelled => "work_unit.cancelled",
            WorkUnitStatus::Open => return Err(invalid("WorkUnit cannot transition back to open")),
        };
        let event = self.event(
            event_name, "work_unit", id, &task, &actor,
            serde_json::json!({"task_id": task.id, "work_unit_id": id, "version": expected_version + 1, "status": status.to_string()}), &now,
        );
        let write = WorkUnitRepo::transition(
            &*self.db,
            TransitionWorkUnit {
                id: id.to_owned(),
                expected_version,
                status,
                updated_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn add_dependency(
        &self,
        source: CollaborationActorSource,
        id: &str,
        depends_on_id: &str,
        expected_version: i64,
    ) -> Result<WorkUnitDependency> {
        let (task, actor) = self.authorize_work_unit(&source, id).await?;
        self.ensure_dependencies_not_pinned(id).await?;
        self.ensure_work_unit_in_task(&task, depends_on_id).await?;
        let now = now_rfc3339();
        let event = self.event(
            "work_unit.dependency_added", "work_unit", id, &task, &actor,
            serde_json::json!({"task_id": task.id, "work_unit_id": id, "depends_on_work_unit_id": depends_on_id, "version": expected_version + 1}), &now,
        );
        let write = WorkUnitRepo::add_dependency(
            &*self.db,
            AddWorkUnitDependency {
                work_unit_id: id.to_owned(),
                depends_on_work_unit_id: depends_on_id.to_owned(),
                expected_version,
                created_by: actor,
                created_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn remove_dependency(
        &self,
        source: CollaborationActorSource,
        id: &str,
        depends_on_id: &str,
        expected_version: i64,
    ) -> Result<WorkUnit> {
        let (task, actor) = self.authorize_work_unit(&source, id).await?;
        self.ensure_dependencies_not_pinned(id).await?;
        self.ensure_work_unit_in_task(&task, depends_on_id).await?;
        let now = now_rfc3339();
        let event = self.event(
            "work_unit.dependency_removed", "work_unit", id, &task, &actor,
            serde_json::json!({"task_id": task.id, "work_unit_id": id, "depends_on_work_unit_id": depends_on_id, "version": expected_version + 1}), &now,
        );
        let write = WorkUnitRepo::remove_dependency(
            &*self.db,
            db::RemoveWorkUnitDependency {
                work_unit_id: id.to_owned(),
                depends_on_work_unit_id: depends_on_id.to_owned(),
                expected_version,
                updated_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn dependencies(
        &self,
        source: CollaborationActorSource,
        id: &str,
    ) -> Result<Vec<WorkUnitDependency>> {
        self.authorize_work_unit(&source, id).await?;
        Ok(WorkUnitRepo::list_dependencies(&*self.db, id).await?)
    }

    pub async fn readiness(
        &self,
        source: CollaborationActorSource,
        id: &str,
    ) -> Result<WorkUnitReadiness> {
        let unit = self.get(source, id).await?;
        let dependencies = WorkUnitRepo::list_dependencies(&*self.db, id).await?;
        let active_execution_ids = WorkUnitRepo::list_active_execution_ids(&*self.db, id).await?;
        let allocation_eligible = if let Some(assigned_actor) = unit.assigned_actor.as_ref() {
            let active_member = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(
                    SELECT 1 FROM task_role tr
                    JOIN role_membership rm ON rm.task_role_id = tr.id
                    WHERE tr.task_id = ? AND tr.role = ?
                      AND rm.actor_kind = ? AND rm.actor_id = ? AND rm.status = 'active'
                )",
            )
            .bind(&unit.task_id)
            .bind(&unit.role)
            .bind(assigned_actor.kind().to_string())
            .bind(assigned_actor.id())
            .fetch_one(self.db.pool())
            .await?
                != 0;
            active_member
                && (!unit.requires_integration || matches!(assigned_actor, ActorRef::Agent(_)))
        } else {
            true
        };
        let unsatisfied_dependency_ids = dependencies
            .into_iter()
            .filter(|dependency| !dependency.satisfied)
            .map(|dependency| dependency.depends_on_work_unit_id)
            .collect::<Vec<_>>();
        let scope_ready = unit.status == WorkUnitStatus::Open
            && active_execution_ids.is_empty()
            && unsatisfied_dependency_ids.is_empty();
        let runnable = scope_ready && allocation_eligible;
        let ready_for_allocation =
            scope_ready && (unit.assigned_actor.is_none() || !allocation_eligible);
        let awaiting_integration = unit.status == WorkUnitStatus::Completed
            && unit.requires_integration
            && !WorkUnitRepo::list_integrations(&*self.db, id)
                .await?
                .iter()
                .any(|integration| integration.outcome == WorkUnitIntegrationOutcome::Success);
        Ok(WorkUnitReadiness {
            runnable,
            ready_for_allocation,
            active_execution_ids,
            unsatisfied_dependency_ids,
            awaiting_integration,
        })
    }

    /// Bind an explicitly prepared Execution to this exact WorkUnit revision.
    /// SQLite rechecks Actor membership, assignment, dependencies, status,
    /// workspace scope, and same-WorkUnit concurrency in the admission trigger.
    pub async fn bind_execution(
        &self,
        source: CollaborationActorSource,
        id: &str,
        expected_version: i64,
        execution: CreateExecution,
    ) -> Result<Execution> {
        let (task, actor) = self.authorize_work_unit(&source, id).await?;
        if execution.task_id != task.id || execution.actor_ref.as_ref() != Some(&actor) {
            return Err(invalid(
                "WorkUnit Execution Task and Actor must match the authorized source",
            ));
        }
        if execution.purpose.is_none() {
            return Err(invalid("WorkUnit Execution requires an explicit Purpose"));
        }
        if let ActorRef::Agent(agent_id) = &actor {
            let agent = AgentRepo::get_by_id(&*self.db, agent_id)
                .await?
                .ok_or_else(|| not_found("agent", agent_id))?;
            if !crate::agent_capacity::has_running_execution_capacity(&self.db, &agent).await? {
                return Err(db::DbError::AgentAtCapacity.into());
            }
        }
        if execution.status != ExecutionStatus::Running {
            return Err(invalid(
                "WorkUnit Execution must start in the running state",
            ));
        }
        let current = WorkUnitRepo::get_by_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("work_unit", id))?;
        if current.version != expected_version || current.status != WorkUnitStatus::Open {
            return Err(db::DbError::VersionConflict.into());
        }
        let _workspace_guard = if current.requires_integration {
            let workspace_id = execution.workspace_id.as_deref().ok_or_else(|| {
                invalid("repository WorkUnit Execution requires its exact Workspace")
            })?;
            Some(self.integration_locks.acquire(workspace_id).await)
        } else {
            if execution.workspace_id.is_some() {
                return Err(invalid(
                    "non-mutating WorkUnit Execution cannot bind a repository Workspace",
                ));
            }
            None
        };
        let current = WorkUnitRepo::get_by_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("work_unit", id))?;
        if current.version != expected_version || current.status != WorkUnitStatus::Open {
            return Err(db::DbError::VersionConflict.into());
        }
        let workspace_lease = if current.requires_integration {
            let workspace_id = execution.workspace_id.as_deref().ok_or_else(|| {
                invalid("repository WorkUnit Execution requires its exact Workspace")
            })?;
            let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| not_found("workspace", workspace_id))?;
            let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| not_found("workspace", workspace_id))?;
            if scope.task_id != task.id
                || scope.kind != WorkspaceScopeKind::WorkUnit
                || scope.work_unit_id.as_deref() != Some(id)
                || workspace.status != WorkspaceStatus::Ready
            {
                return Err(not_found("workspace", workspace_id));
            }
            Some(
                crate::task_service::TaskService::new(
                    Arc::clone(&self.db),
                    Arc::clone(&self.event_bus),
                )
                .prepare_work_unit_workspace_lease(&task, &current, &workspace, &execution, &actor)
                .await?,
            )
        } else {
            None
        };
        let now = execution.created_at.clone();
        let event = self.event(
            "execution.started", "execution", &execution.id, &task, &actor,
            serde_json::json!({"task_id": task.id, "work_unit_id": id, "execution_id": execution.id, "work_unit_version": expected_version}), &now,
        );
        let write = WorkUnitExecutionRepo::create_for_work_unit(
            &*self.db,
            CreateWorkUnitExecution {
                execution,
                work_unit_id: id.to_owned(),
                work_unit_version: expected_version,
                workspace_lease,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn prepare_workspace(
        &self,
        source: CollaborationActorSource,
        work_unit_id: &str,
    ) -> Result<Workspace> {
        let (task, _) = self.authorize_work_unit(&source, work_unit_id).await?;
        let _preparation_guard = self
            .integration_locks
            .acquire(&format!("work-unit-workspace:{work_unit_id}"))
            .await;
        let _integration_guard = self
            .integration_locks
            .acquire(&format!("task-integration:{}", task.id))
            .await;
        let operation = self
            .integration_operations
            .acquire(
                &task.id,
                db::TaskIntegrationOperationKind::WorkUnitWorkspacePrepare,
                work_unit_id,
            )
            .await?;
        let result = async {
        let unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| not_found("work_unit", work_unit_id))?;
        if !unit.requires_integration {
            return Err(invalid(
                "WorkUnit does not require a repository Workspace or integration",
            ));
        }
        if unit.status != WorkUnitStatus::Open
            || !self.readiness(source, work_unit_id).await?.runnable
        {
            return Err(invalid("WorkUnit is not runnable"));
        }
        let integration = prepare_integration_workspace_for_work_unit(
            &self.db,
            &self.workspace_root,
            &task,
            Some(Arc::clone(&self.repo_cache_locks)),
        )
        .await?;
        let _integration_workspace_guard = self.integration_locks.acquire(&integration.id).await;
        let repo = RepoRepo::get_by_id(&*self.db, &integration.repo_id)
            .await?
            .ok_or_else(|| not_found("repo", integration.repo_id.clone()))?;
        let repo_path = resolve_repo_source(&repo, &self.workspace_root).await?;
        let manager = WorkspaceManager::new(self.workspace_root.clone())
            .with_repo_cache_locks(Arc::clone(&self.repo_cache_locks));
        if let Some(existing) =
            WorkUnitWorkspaceRepo::get_by_work_unit_id(&*self.db, work_unit_id).await?
        {
            let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &existing.id)
                .await?
                .ok_or_else(|| not_found("workspace", existing.id.clone()))?;
            if scope.kind != WorkspaceScopeKind::WorkUnit
                || scope.work_unit_id.as_deref() != Some(work_unit_id)
            {
                return Err(invalid("Workspace scope does not match WorkUnit"));
            }
            if existing.status == WorkspaceStatus::Cleaning {
                return Err(invalid(
                    "WorkUnit workspace cleanup is still in progress",
                ));
            }
            let branch_exists = git::branch_exists(Path::new(&repo_path), &existing.branch).await?;
            if existing.status == WorkspaceStatus::Cleaned && !branch_exists {
                return Err(invalid(
                    "cleaned WorkUnit Workspace branch is missing; explicit reset or recovery is required",
                ));
            }
            if Path::new(&existing.worktree_path).exists() {
                git::get_current_sha(Path::new(&existing.worktree_path)).await?;
                if existing.status == WorkspaceStatus::Cleaned {
                    let worktree_path = Path::new(&existing.worktree_path);
                    if git::get_current_branch(worktree_path).await? != existing.branch
                        || !git::is_worktree_clean(worktree_path).await?
                    {
                        return Err(invalid(
                            "cleaned WorkUnit Workspace does not match its preserved branch; explicit recovery is required",
                        ));
                    }
                }
            } else if branch_exists {
                manager
                    .recover_work_unit_worktree(
                        &repo_path,
                        &task.id,
                        work_unit_id,
                        &existing.id,
                        &repo.id,
                        &existing.branch,
                    )
                    .await
                    .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            } else {
                let base = existing
                    .before_sha
                    .as_deref()
                    .ok_or_else(|| invalid("WorkUnit Workspace has no recorded base SHA"))?;
                manager
                    .create_work_unit_worktree(
                        &repo_path,
                        &task.id,
                        work_unit_id,
                        &existing.id,
                        &repo.id,
                        base,
                    )
                    .await
                    .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            }
            let now = now_rfc3339();
            WorkspaceRepo::update_status(
                &*self.db,
                &existing.id,
                WorkspaceStatus::Ready,
                None,
                &now,
            )
            .await?;
            return WorkspaceRepo::get_by_id(&*self.db, &existing.id)
                .await?
                .ok_or_else(|| not_found("workspace", existing.id));
        }
        let workspace_id = new_uuid_v4();
        let before_sha = git::get_current_sha(Path::new(&integration.worktree_path)).await?;
        let branch = workspace::work_unit_branch_name(&task.id, work_unit_id, &workspace_id)
            .map_err(|error| invalid(error.to_string()))?;
        let path = manager
            .work_unit_worktree_path(&task.id, work_unit_id, &workspace_id, &repo.id)
            .map_err(|error| invalid(error.to_string()))?;
        let now = now_rfc3339();
        let workspace = WorkUnitWorkspaceRepo::create_for_work_unit(
            &*self.db,
            db::CreateWorkUnitWorkspace {
                workspace: db::CreateWorkspace {
                    id: workspace_id.clone(),
                    task_id: task.id.clone(),
                    repo_id: repo.id,
                    worktree_path: path.to_string_lossy().into_owned(),
                    branch,
                    status: WorkspaceStatus::Creating,
                    before_sha: Some(before_sha.clone()),
                    created_at: now.clone(),
                    updated_at: now,
                },
                work_unit_id: work_unit_id.to_owned(),
            },
        )
        .await?;
        match manager
            .create_work_unit_worktree(
                &repo_path,
                &task.id,
                work_unit_id,
                &workspace.id,
                &integration.repo_id,
                &before_sha,
            )
            .await
        {
            Ok(_) => {}
            Err(error) => {
                let now = now_rfc3339();
                let _ = WorkspaceRepo::update_status(
                    &*self.db,
                    &workspace.id,
                    WorkspaceStatus::Error,
                    Some(error.to_string()),
                    &now,
                )
                .await;
                return Err(invalid(error.to_string()));
            }
        };
        let now = now_rfc3339();
        WorkspaceRepo::update_status(&*self.db, &workspace.id, WorkspaceStatus::Ready, None, &now)
            .await?;
        WorkspaceRepo::get_by_id(&*self.db, &workspace.id)
            .await?
            .ok_or_else(|| not_found("workspace", workspace.id))
        }
        .await;
        let finish_result = operation
            .finish(if result.is_ok() {
                db::TaskIntegrationOperationStatus::Succeeded
            } else {
                db::TaskIntegrationOperationStatus::Failed
            })
            .await;
        match (result, finish_result) {
            (Ok(workspace), Ok(())) => Ok(workspace),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), _) => Err(error),
        }
    }

    /// Integrate one completed WorkUnit Execution's exact committed SHA into
    /// the Task integration worktree under the durable Task operation claim.
    pub async fn integrate(
        &self,
        source: CollaborationActorSource,
        work_unit_id: &str,
        execution_id: &str,
        idempotency_key: &str,
    ) -> Result<WorkUnitIntegration> {
        let (task, actor) = self.authorize_work_unit(&source, work_unit_id).await?;
        let unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| not_found("work_unit", work_unit_id))?;
        if unit.status != WorkUnitStatus::Completed || !unit.requires_integration {
            return Err(invalid("WorkUnit is not awaiting repository integration"));
        }
        let execution_task_id = ExecutionRepo::get_task_id(&*self.db, execution_id)
            .await?
            .ok_or_else(|| not_found("execution", execution_id))?;
        if execution_task_id != task.id {
            return Err(not_found("execution", execution_id));
        }
        let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
            .await?
            .ok_or_else(|| not_found("execution", execution_id))?;
        if execution.work_unit_id.as_deref() != Some(work_unit_id)
            || execution.status != ExecutionStatus::Completed
            || execution.work_unit_version != Some(unit.version - 1)
        {
            return Err(invalid(
                "Integration requires the completed Execution admitted for this WorkUnit revision",
            ));
        }
        let source_workspace_id = execution
            .workspace_id
            .clone()
            .ok_or_else(|| invalid("WorkUnit Execution has no exact Workspace"))?;
        let source_workspace = WorkspaceRepo::get_by_id(&*self.db, &source_workspace_id)
            .await?
            .ok_or_else(|| not_found("workspace", source_workspace_id.clone()))?;
        let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &source_workspace_id)
            .await?
            .ok_or_else(|| not_found("workspace", source_workspace_id.clone()))?;
        if scope.kind != WorkspaceScopeKind::WorkUnit
            || scope.work_unit_id.as_deref() != Some(work_unit_id)
        {
            return Err(not_found("workspace", source_workspace_id));
        }
        let source_sha = execution
            .after_sha
            .clone()
            .ok_or_else(|| invalid("WorkUnit Execution has no result SHA"))?;
        let previous =
            WorkUnitRepo::get_integration_by_idempotency(&*self.db, &task.id, idempotency_key)
                .await?;
        if let Some(previous) = previous.as_ref() {
            validate_integration_replay(
                previous,
                work_unit_id,
                execution_id,
                &source_sha,
                &source_workspace_id,
            )?;
            if previous.outcome != WorkUnitIntegrationOutcome::Running {
                return Ok(previous.clone());
            }
        }

        let operation_owner = previous
            .as_ref()
            .map(|integration| integration.id.clone())
            .unwrap_or_else(new_uuid_v4);
        let integration_lock_id = format!("task-integration:{}", task.id);
        let _local_lock = self.integration_locks.acquire(&integration_lock_id).await;
        let operation = self
            .integration_operations
            .acquire(
                &task.id,
                db::TaskIntegrationOperationKind::WorkUnitIntegration,
                &operation_owner,
            )
            .await?;

        let result = async {
            let integration = prepare_integration_workspace_for_work_unit(
                &self.db,
                &self.workspace_root,
                &task,
                Some(Arc::clone(&self.repo_cache_locks)),
            )
            .await?;
            let _integration_workspace_guard =
                self.integration_locks.acquire(&integration.id).await;
            let existing =
                WorkUnitRepo::get_integration_by_idempotency(&*self.db, &task.id, idempotency_key)
                    .await?;
            let record = if let Some(existing) = existing {
                validate_integration_replay(
                    &existing,
                    work_unit_id,
                    execution_id,
                    &source_sha,
                    &source_workspace_id,
                )?;
                if existing.target_workspace_id != integration.id {
                    return Err(db::DbError::IdempotencyConflict.into());
                }
                if existing.outcome != WorkUnitIntegrationOutcome::Running {
                    return Ok(existing);
                }
                existing
            } else {
                let target_path = Path::new(&integration.worktree_path);
                if !git::is_worktree_clean(target_path).await? {
                    return Err(invalid(
                        "Task integration workspace has uncommitted changes",
                    ));
                }
                let target_before_sha = git::get_current_sha(target_path).await?;
                let now = now_rfc3339();
                let event = self.event(
                    "work_unit.integration_started",
                    "work_unit_integration",
                    &operation_owner,
                    &task,
                    &actor,
                    serde_json::json!({
                        "task_id": task.id,
                        "work_unit_id": work_unit_id,
                        "integration_id": operation_owner
                    }),
                    &now,
                );
                let write = WorkUnitRepo::begin_integration(
                    &*self.db,
                    CreateWorkUnitIntegration {
                        id: operation_owner.clone(),
                        task_id: task.id.clone(),
                        work_unit_id: work_unit_id.to_owned(),
                        execution_id: execution_id.to_owned(),
                        source_workspace_id: source_workspace_id.clone(),
                        source_branch: source_workspace.branch.clone(),
                        source_sha: source_sha.clone(),
                        target_workspace_id: integration.id.clone(),
                        target_branch: integration.branch.clone(),
                        target_before_sha,
                        operation_idempotency_key: idempotency_key.to_owned(),
                        started_at: now.clone(),
                        created_at: now,
                    },
                    event,
                )
                .await?;
                self.domain_events.publish_committed(&write.event);
                write.record
            };

            let target_path = Path::new(&integration.worktree_path);
            match classify_integration_recovery(target_path, &record).await? {
                IntegrationRecovery::Materialized(after_sha) => {
                    self.record_integration_outcome(
                        &record,
                        &task,
                        &actor,
                        work_unit_id,
                        WorkUnitIntegrationOutcome::Success,
                        Some(after_sha),
                        None,
                    )
                    .await
                }
                IntegrationRecovery::Mismatch(kind) => {
                    self.record_integration_outcome(
                        &record,
                        &task,
                        &actor,
                        work_unit_id,
                        WorkUnitIntegrationOutcome::Failed,
                        None,
                        Some(serde_json::json!({ "kind": kind }).to_string()),
                    )
                    .await
                }
                IntegrationRecovery::Retry => {
                    if !git::is_worktree_clean(target_path).await? {
                        return self
                            .record_integration_outcome(
                                &record,
                                &task,
                                &actor,
                                work_unit_id,
                                WorkUnitIntegrationOutcome::Failed,
                                None,
                                Some(r#"{"kind":"target_worktree_dirty"}"#.to_owned()),
                            )
                            .await;
                    }
                    match git::merge(target_path, &record.source_sha).await {
                        Ok(()) => {
                            let after_sha = git::get_current_sha(target_path).await?;
                            if git::is_worktree_clean(target_path).await? {
                                self.record_integration_outcome(
                                    &record,
                                    &task,
                                    &actor,
                                    work_unit_id,
                                    WorkUnitIntegrationOutcome::Success,
                                    Some(after_sha),
                                    None,
                                )
                                .await
                            } else {
                                self.record_integration_outcome(
                                    &record,
                                    &task,
                                    &actor,
                                    work_unit_id,
                                    WorkUnitIntegrationOutcome::Failed,
                                    None,
                                    Some(
                                        r#"{"kind":"target_worktree_dirty_after_merge"}"#
                                            .to_owned(),
                                    ),
                                )
                                .await
                            }
                        }
                        Err(git::GitError::MergeConflict { .. }) => {
                            recover_interrupted_merge(target_path, &record).await?;
                            self.record_integration_outcome(
                                &record,
                                &task,
                                &actor,
                                work_unit_id,
                                WorkUnitIntegrationOutcome::Conflict,
                                None,
                                Some(r#"{"kind":"merge_conflict"}"#.to_owned()),
                            )
                            .await
                        }
                        Err(_error) => {
                            if git::get_merge_head(target_path).await?.is_some() {
                                recover_interrupted_merge(target_path, &record).await?;
                            }
                            let head = git::get_current_sha(target_path).await?;
                            if head != record.target_before_sha
                                || !git::is_worktree_clean(target_path).await?
                            {
                                self.record_integration_outcome(
                                    &record,
                                    &task,
                                    &actor,
                                    work_unit_id,
                                    WorkUnitIntegrationOutcome::Failed,
                                    None,
                                    Some(r#"{"kind":"target_head_mismatch"}"#.to_owned()),
                                )
                                .await
                            } else {
                                self.record_integration_outcome(
                                    &record,
                                    &task,
                                    &actor,
                                    work_unit_id,
                                    WorkUnitIntegrationOutcome::Failed,
                                    None,
                                    Some(r#"{"kind":"git_failure"}"#.to_owned()),
                                )
                                .await
                            }
                        }
                    }
                }
            }
        }
        .await;

        let operation_status = match &result {
            Ok(record) if record.outcome == WorkUnitIntegrationOutcome::Success => {
                db::TaskIntegrationOperationStatus::Succeeded
            }
            Ok(record) if record.outcome == WorkUnitIntegrationOutcome::Conflict => {
                db::TaskIntegrationOperationStatus::Conflict
            }
            Ok(_) | Err(_) => db::TaskIntegrationOperationStatus::Failed,
        };
        let finish_result = operation.finish(operation_status).await;
        match (result, finish_result) {
            (Ok(record), Ok(())) => Ok(record),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), _) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_integration_outcome(
        &self,
        record: &WorkUnitIntegration,
        task: &Task,
        actor: &ActorRef,
        work_unit_id: &str,
        outcome: WorkUnitIntegrationOutcome,
        target_after_sha: Option<String>,
        conflict_metadata_json: Option<String>,
    ) -> Result<WorkUnitIntegration> {
        let now = now_rfc3339();
        let event_type = match outcome {
            WorkUnitIntegrationOutcome::Success => "work_unit.integration_succeeded",
            WorkUnitIntegrationOutcome::Conflict => "work_unit.integration_conflicted",
            _ => "work_unit.integration_failed",
        };
        let event = self.event(
            event_type,
            "work_unit_integration",
            &record.id,
            task,
            actor,
            serde_json::json!({
                "task_id": task.id,
                "work_unit_id": work_unit_id,
                "integration_id": record.id,
                "outcome": outcome.to_string()
            }),
            &now,
        );
        let write = WorkUnitRepo::record_integration(
            &*self.db,
            RecordWorkUnitIntegration {
                id: record.id.clone(),
                expected_version: record.version,
                outcome,
                target_after_sha,
                conflict_metadata_json,
                finished_at: now.clone(),
                updated_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    async fn authorize_work_unit(
        &self,
        source: &CollaborationActorSource,
        id: &str,
    ) -> Result<(Task, ActorRef)> {
        let task_id = WorkUnitRepo::get_task_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("work_unit", id))?;
        self.collaboration
            .authorize_source(&task_id, source)
            .await
            .map_err(|_| not_found("work_unit", id))
    }

    async fn ensure_work_unit_in_task(&self, task: &Task, id: &str) -> Result<WorkUnit> {
        let owner_task = WorkUnitRepo::get_task_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("work_unit", id))?;
        if owner_task != task.id {
            return Err(not_found("work_unit", id));
        }
        WorkUnitRepo::get_by_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("work_unit", id))
    }

    async fn ensure_dependencies_not_pinned(&self, id: &str) -> Result<()> {
        if WorkUnitWorkspaceRepo::get_by_work_unit_id(&*self.db, id)
            .await?
            .is_some()
        {
            return Err(ServiceError::conflict(
                "WorkUnit dependencies are fixed after its repository Workspace is prepared",
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn event(
        &self,
        event_type: &str,
        entity_type: &str,
        entity_id: &str,
        task: &Task,
        actor: &ActorRef,
        payload: serde_json::Value,
        now: &str,
    ) -> CreateDomainEvent {
        CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: event_type.to_owned(),
            entity_type: entity_type.to_owned(),
            entity_id: entity_id.to_owned(),
            actor_type: actor.kind().to_string(),
            actor_id: Some(actor.id().to_owned()),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: None,
            payload_json: payload.to_string(),
            created_at: now.to_owned(),
        }
    }
}

enum IntegrationRecovery {
    Retry,
    Materialized(String),
    Mismatch(&'static str),
}

fn validate_integration_replay(
    existing: &WorkUnitIntegration,
    work_unit_id: &str,
    execution_id: &str,
    source_sha: &str,
    source_workspace_id: &str,
) -> Result<()> {
    if existing.work_unit_id != work_unit_id
        || existing.execution_id != execution_id
        || existing.source_sha != source_sha
        || existing.source_workspace_id != source_workspace_id
    {
        return Err(db::DbError::IdempotencyConflict.into());
    }
    Ok(())
}

async fn classify_integration_recovery(
    target_path: &Path,
    record: &WorkUnitIntegration,
) -> Result<IntegrationRecovery> {
    if let Some(merge_head) = git::get_merge_head(target_path).await? {
        if merge_head != record.source_sha {
            return Ok(IntegrationRecovery::Mismatch(
                "interrupted_merge_source_mismatch",
            ));
        }
        if git::get_current_sha(target_path).await? != record.target_before_sha {
            return Ok(IntegrationRecovery::Mismatch("target_head_mismatch"));
        }
        recover_interrupted_merge(target_path, record).await?;
        return Ok(IntegrationRecovery::Retry);
    }

    let head = git::get_current_sha(target_path).await?;
    if head == record.target_before_sha {
        return Ok(IntegrationRecovery::Retry);
    }
    if !git::is_worktree_clean(target_path).await? {
        return Ok(IntegrationRecovery::Mismatch("target_head_mismatch"));
    }
    if head == record.source_sha
        && git::is_ancestor(target_path, &record.target_before_sha, &record.source_sha).await?
    {
        return Ok(IntegrationRecovery::Materialized(head));
    }
    let parents = git::commit_parents(target_path, &head).await?;
    if parents.len() == 2
        && parents[0] == record.target_before_sha
        && parents[1] == record.source_sha
    {
        return Ok(IntegrationRecovery::Materialized(head));
    }
    Ok(IntegrationRecovery::Mismatch("target_head_mismatch"))
}

async fn recover_interrupted_merge(target_path: &Path, record: &WorkUnitIntegration) -> Result<()> {
    if git::get_merge_head(target_path).await?.as_deref() != Some(&record.source_sha) {
        return Err(invalid(
            "interrupted integration merge does not match the pinned source",
        ));
    }
    if git::get_current_sha(target_path).await? != record.target_before_sha {
        return Err(invalid(
            "interrupted integration target HEAD changed; recovery is required",
        ));
    }
    git::abort_merge(target_path).await.map_err(|error| {
        invalid(format!(
            "interrupted integration merge could not be aborted safely: {error}"
        ))
    })?;
    if git::get_current_sha(target_path).await? != record.target_before_sha
        || !git::is_worktree_clean(target_path).await?
    {
        return Err(invalid(
            "interrupted integration merge did not restore its pinned clean target",
        ));
    }
    Ok(())
}

fn not_found(entity: &'static str, id: impl Into<String>) -> ServiceError {
    ServiceError::NotFound {
        entity,
        id: id.into(),
    }
}

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::InvalidOperation {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{
        create_sqlite_pool, run_migrations, ActorKind, CoordinationMode, CreateAgentIdentity,
        CreateAgentProfile, CreateDomainEvent, CreateProject, CreateRepo, CreateRoleMembership,
        CreateTask, CreateTaskRole, CreateWorkUnit, CreateWorkspace, ExecutionRepo,
        ExecutionStatus, ProjectRepo, RepoRepo, RoleMembershipRepo, RoleMembershipStatus, TaskRepo,
        TaskRoleRepo, UpdateExecution, UpdateRoleMembership, WorkMode, WorkUnitRepo,
        WorkUnitStatus, WorkUnitWorkspaceRepo, WorkspaceRepo, WorkspaceStatus,
    };
    use std::{path::Path, process::Stdio, time::Duration};
    use tempfile::TempDir;
    use tokio::process::Command;

    fn event(
        event_type: &str,
        entity_type: &str,
        entity_id: &str,
        task_id: &str,
        actor_id: &str,
    ) -> CreateDomainEvent {
        let now = now_rfc3339();
        CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: event_type.to_owned(),
            entity_type: entity_type.to_owned(),
            entity_id: entity_id.to_owned(),
            actor_type: "human".to_owned(),
            actor_id: Some(actor_id.to_owned()),
            scope_type: "task".to_owned(),
            scope_id: task_id.to_owned(),
            correlation_id: new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: None,
            payload_json: serde_json::json!({
                "task_id": task_id,
                "entity_id": entity_id
            })
            .to_string(),
            created_at: now,
        }
    }

    fn running_repository_execution(
        task_id: &str,
        workspace_id: &str,
        agent_id: &str,
        before_sha: &str,
    ) -> db::CreateExecution {
        let now = now_rfc3339();
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            agent_id: Some(agent_id.to_owned()),
            actor_ref: Some(ActorRef::Agent(agent_id.to_owned())),
            role: "implementer".to_owned(),
            purpose: Some(db::ExecutionPurpose::Implement),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: Some(before_sha.to_owned()),
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: Some(workspace_id.to_owned()),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    async fn run_git(path: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {} failed\nstdout: {}\nstderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn integration_pins_success_and_aborts_conflict_without_losing_workunit_workspace() {
        let temp = TempDir::new().expect("temporary directory");
        let database_path = temp.path().join("pr5-integration.db");
        let database_url = format!("sqlite://{}", database_path.display());
        let pool = create_sqlite_pool(&database_url).await.expect("pool");
        run_migrations(&pool).await.expect("migrations");
        let db = Arc::new(SqliteDb::new(pool));
        let event_bus = Arc::new(EventBus::new(16));
        let repository_path = temp.path().join("repository");
        std::fs::create_dir_all(&repository_path).expect("repository directory");
        git::init(&repository_path).await.expect("git init");
        run_git(&repository_path, &["checkout", "-B", "main"]).await;
        std::fs::write(repository_path.join("base.txt"), "base\n").expect("base file");
        let base_sha = git::commit_all(&repository_path, "initial")
            .await
            .expect("base commit");

        let now = now_rfc3339();
        let project_id = new_uuid_v4();
        let task_id = new_uuid_v4();
        let repo_id = new_uuid_v4();
        let human_id = new_uuid_v4();
        let agent_id = new_uuid_v4();
        let task_role_id = new_uuid_v4();
        ProjectRepo::create(
            &*db,
            CreateProject {
                id: project_id.clone(),
                name: "PR5 integration".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(human_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Project");
        db::UserRepo::create_user(
            &*db,
            &db::User {
                id: human_id.clone(),
                email: "pr5-integration@example.invalid".to_owned(),
                password_hash: "unused".to_owned(),
                display_name: None,
                is_admin: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Human");
        db::AgentRepo::create_identity_with_profile(
            &*db,
            CreateAgentIdentity {
                id: agent_id.clone(),
                name: "WorkUnit worker".to_owned(),
                description: None,
                max_concurrent_tasks: 4,
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status: db::AgentStatus::Idle,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: Some(human_id.clone()),
                visibility: "account".to_owned(),
                account_permission_ceiling: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            CreateAgentProfile {
                id: new_uuid_v4(),
                identity_id: agent_id.clone(),
                backend_kind: "cli".to_owned(),
                executor_type: "codex".to_owned(),
                provider: None,
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: "[]".to_owned(),
                tool_policy_json: "{}".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: None,
                daemon_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Agent");
        RepoRepo::create(
            &*db,
            CreateRepo {
                id: repo_id.clone(),
                project_id: project_id.clone(),
                name: "repository".to_owned(),
                remote_url: repository_path.to_string_lossy().into_owned(),
                local_path: Some(repository_path.to_string_lossy().into_owned()),
                work_mode: WorkMode::DirectMerge,
                default_branch: "main".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Repo");
        TaskRepo::create(
            &*db,
            CreateTask {
                id: task_id.clone(),
                project_id: project_id.clone(),
                repo_id: Some(repo_id.clone()),
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "Task".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                plan: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Task");
        TaskRoleRepo::create(
            &*db,
            CreateTaskRole {
                id: task_role_id.clone(),
                task_id: task_id.clone(),
                role: "implementer".to_owned(),
                coordination_mode: Some(CoordinationMode::Independent),
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("TaskRole");
        RoleMembershipRepo::add(
            &*db,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id: task_role_id.clone(),
                actor_kind: ActorKind::Agent,
                actor_id: agent_id.clone(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("membership");
        RoleMembershipRepo::add(
            &*db,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id: task_role_id.clone(),
                actor_kind: ActorKind::Human,
                actor_id: human_id.clone(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Human TaskRole membership");

        let workspace_root = temp.path().join("workspaces");
        let manager = WorkspaceManager::new(workspace_root.clone());
        let integration_path = manager
            .create_worktree_named(
                &repository_path.to_string_lossy(),
                &task_id,
                "repository",
                "main",
            )
            .await
            .expect("Task integration worktree");
        let integration_workspace = WorkspaceRepo::create(
            &*db,
            CreateWorkspace {
                id: new_uuid_v4(),
                task_id: task_id.clone(),
                repo_id: repo_id.clone(),
                worktree_path: integration_path.to_string_lossy().into_owned(),
                branch: workspace::task_branch_name(&task_id),
                status: WorkspaceStatus::Ready,
                before_sha: Some(base_sha.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Task integration Workspace");

        let repo_cache_locks = Arc::new(RepoCacheLockManager::default());
        let integration_locks = Arc::new(WorkspaceExecutionLockManager::default());
        let service = WorkUnitService::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            workspace_root.clone(),
            Arc::clone(&repo_cache_locks),
            Arc::clone(&integration_locks),
        );
        let human_repo_unit = service
            .create(
                CollaborationActorSource::Human(human_id.clone()),
                CreateWorkUnitInput {
                    task_id: task_id.clone(),
                    title: "Human repository allocation".to_owned(),
                    scope: "requires a repository execution".to_owned(),
                    role: "implementer".to_owned(),
                    parent_work_unit_id: None,
                    assigned_actor: Some(ActorRef::Human(human_id.clone())),
                    requires_integration: true,
                    provenance: None,
                },
            )
            .await
            .expect("Human may hold an eligible coordination allocation");
        let human_repo_readiness = service
            .readiness(
                CollaborationActorSource::Human(human_id.clone()),
                &human_repo_unit.id,
            )
            .await
            .expect("repository allocation readiness");
        assert!(!human_repo_readiness.runnable);
        assert!(human_repo_readiness.ready_for_allocation);
        let mut committed_notifications = event_bus.subscribe();

        async fn add_result(
            db: &SqliteDb,
            manager: &WorkspaceManager,
            repository_path: &Path,
            task_id: &str,
            repo_id: &str,
            human_id: &str,
            agent_id: &str,
            unit_title: &str,
            file_name: &str,
            contents: &str,
            base_sha: &str,
        ) -> (db::WorkUnit, db::Workspace, String, String) {
            let now = now_rfc3339();
            let unit_id = new_uuid_v4();
            let unit = WorkUnitRepo::create(
                db,
                CreateWorkUnit {
                    id: unit_id.clone(),
                    task_id: task_id.to_owned(),
                    parent_work_unit_id: None,
                    title: unit_title.to_owned(),
                    scope: "test scope".to_owned(),
                    role: "implementer".to_owned(),
                    assigned_actor: Some(ActorRef::Agent(agent_id.to_owned())),
                    requires_integration: true,
                    provenance: None,
                    created_by: ActorRef::Human(human_id.to_owned()),
                    created_at: now.clone(),
                },
                event(
                    "work_unit.created",
                    "work_unit",
                    &unit_id,
                    task_id,
                    human_id,
                ),
            )
            .await
            .expect("WorkUnit")
            .record;
            let workspace_id = new_uuid_v4();
            let branch = workspace::work_unit_branch_name(task_id, &unit_id, &workspace_id)
                .expect("branch name");
            let path = manager
                .create_work_unit_worktree(
                    &repository_path.to_string_lossy(),
                    task_id,
                    &unit_id,
                    &workspace_id,
                    repo_id,
                    base_sha,
                )
                .await
                .expect("WorkUnit worktree");
            let workspace = WorkUnitWorkspaceRepo::create_for_work_unit(
                db,
                db::CreateWorkUnitWorkspace {
                    workspace: CreateWorkspace {
                        id: workspace_id.clone(),
                        task_id: task_id.to_owned(),
                        repo_id: repo_id.to_owned(),
                        worktree_path: path.to_string_lossy().into_owned(),
                        branch,
                        status: WorkspaceStatus::Ready,
                        before_sha: Some(base_sha.to_owned()),
                        created_at: now.clone(),
                        updated_at: now.clone(),
                    },
                    work_unit_id: unit_id.clone(),
                },
            )
            .await
            .expect("WorkUnit Workspace");
            std::fs::write(path.join(file_name), contents).expect("WorkUnit file");
            let source_sha = git::commit_all(&path, "WorkUnit result")
                .await
                .expect("WorkUnit result commit");
            let execution_id = new_uuid_v4();
            sqlx::query(
                "INSERT INTO execution (
                     id, task_id, agent_id, role, status, workspace_id, before_sha,
                     created_at, updated_at, actor_kind, actor_id, purpose,
                     work_unit_id, work_unit_version
                 ) VALUES (?, ?, ?, 'implementer', 'running', ?, ?, ?, ?, 'agent', ?, 'implement', ?, 1)",
            )
            .bind(&execution_id)
            .bind(task_id)
            .bind(agent_id)
            .bind(&workspace_id)
            .bind(base_sha)
            .bind(&now)
            .bind(&now)
            .bind(agent_id)
            .bind(&unit_id)
            .execute(db.pool())
            .await
            .expect("WorkUnit Execution admission");
            ExecutionRepo::update(
                db,
                UpdateExecution {
                    id: execution_id.clone(),
                    status: Some(ExecutionStatus::Completed),
                    stop_reason: None,
                    stopped_by: None,
                    resume_policy: None,
                    stopped_at: None,
                    agent_session_id: None,
                    agent_message_id: None,
                    last_activity_at: None,
                    summary: None,
                    logs_path: None,
                    before_sha: None,
                    after_sha: Some(Some(source_sha.clone())),
                    error: None,
                    executor_config_snapshot_json: None,
                    updated_at: now.clone(),
                },
            )
            .await
            .expect("Execution completion");
            WorkUnitRepo::transition(
                db,
                db::TransitionWorkUnit {
                    id: unit_id.clone(),
                    expected_version: 1,
                    status: WorkUnitStatus::Completed,
                    updated_at: now,
                },
                event(
                    "work_unit.completed",
                    "work_unit",
                    &unit_id,
                    task_id,
                    human_id,
                ),
            )
            .await
            .expect("WorkUnit completion");
            (unit, workspace, execution_id, source_sha)
        }

        async fn begin_running_integration(
            db: &SqliteDb,
            task_id: &str,
            work_unit: &db::WorkUnit,
            workspace: &db::Workspace,
            execution_id: &str,
            source_sha: &str,
            target: &db::Workspace,
            target_before_sha: &str,
            idempotency_key: &str,
            human_id: &str,
        ) -> db::WorkUnitIntegration {
            let integration_id = new_uuid_v4();
            let now = now_rfc3339();
            WorkUnitRepo::begin_integration(
                db,
                CreateWorkUnitIntegration {
                    id: integration_id.clone(),
                    task_id: task_id.to_owned(),
                    work_unit_id: work_unit.id.clone(),
                    execution_id: execution_id.to_owned(),
                    source_workspace_id: workspace.id.clone(),
                    source_branch: workspace.branch.clone(),
                    source_sha: source_sha.to_owned(),
                    target_workspace_id: target.id.clone(),
                    target_branch: target.branch.clone(),
                    target_before_sha: target_before_sha.to_owned(),
                    operation_idempotency_key: idempotency_key.to_owned(),
                    started_at: now.clone(),
                    created_at: now,
                },
                event(
                    "work_unit.integration_started",
                    "work_unit_integration",
                    &integration_id,
                    task_id,
                    human_id,
                ),
            )
            .await
            .expect("persist running integration before Git")
            .record
        }

        let (first_unit, first_workspace, first_execution, first_sha) = add_result(
            &db,
            &manager,
            &repository_path,
            &task_id,
            &repo_id,
            &human_id,
            &agent_id,
            "clean integration",
            "result.txt",
            "first result\n",
            &base_sha,
        )
        .await;
        assert_eq!(
            git::get_current_sha(&integration_path).await.unwrap(),
            base_sha
        );
        let dependent_id = new_uuid_v4();
        WorkUnitRepo::create(
            &*db,
            CreateWorkUnit {
                id: dependent_id.clone(),
                task_id: task_id.clone(),
                parent_work_unit_id: None,
                title: "dependent WorkUnit".to_owned(),
                scope: "wait for integrated prerequisite".to_owned(),
                role: "implementer".to_owned(),
                assigned_actor: Some(ActorRef::Agent(agent_id.clone())),
                requires_integration: false,
                provenance: None,
                created_by: ActorRef::Human(human_id.clone()),
                created_at: now_rfc3339(),
            },
            event(
                "work_unit.created",
                "work_unit",
                &dependent_id,
                &task_id,
                &human_id,
            ),
        )
        .await
        .expect("dependent WorkUnit creates");
        WorkUnitRepo::add_dependency(
            &*db,
            db::AddWorkUnitDependency {
                work_unit_id: dependent_id.clone(),
                depends_on_work_unit_id: first_unit.id.clone(),
                expected_version: 1,
                created_by: ActorRef::Human(human_id.clone()),
                created_at: now_rfc3339(),
            },
            event(
                "work_unit.dependency_added",
                "work_unit",
                &dependent_id,
                &task_id,
                &human_id,
            ),
        )
        .await
        .expect("dependent WorkUnit records its prerequisite");
        let blocked_readiness = service
            .readiness(
                CollaborationActorSource::Human(human_id.clone()),
                &dependent_id,
            )
            .await
            .expect("prerequisite is not yet integrated");
        assert!(!blocked_readiness.runnable);
        assert_eq!(
            blocked_readiness.unsatisfied_dependency_ids,
            vec![first_unit.id.clone()]
        );
        let first = service
            .integrate(
                CollaborationActorSource::Human(human_id.clone()),
                &first_unit.id,
                &first_execution,
                "integrate-first-result",
            )
            .await
            .expect("clean integration");
        assert_eq!(first.outcome, WorkUnitIntegrationOutcome::Success);
        assert_eq!(first.source_sha, first_sha);
        assert_eq!(first.source_workspace_id, first_workspace.id);
        assert_eq!(first.target_workspace_id, integration_workspace.id);
        assert_eq!(first.target_before_sha, base_sha);
        let started_notification = committed_notifications
            .recv()
            .await
            .expect("integration-started notification");
        let completed_notification = committed_notifications
            .recv()
            .await
            .expect("integration-completed notification");
        let durable_notification_ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM domain_event
             WHERE entity_type = 'work_unit_integration' AND entity_id = ?
               AND event_type IN ('work_unit.integration_started', 'work_unit.integration_succeeded')
             ORDER BY sequence",
        )
        .bind(&first.id)
        .fetch_all(db.pool())
        .await
        .expect("durable integration events");
        assert_eq!(durable_notification_ids.len(), 2);
        assert_eq!(started_notification.entity_id, durable_notification_ids[0]);
        assert_eq!(
            completed_notification.entity_id,
            durable_notification_ids[1]
        );
        let ready_after_integration = service
            .readiness(
                CollaborationActorSource::Human(human_id.clone()),
                &dependent_id,
            )
            .await
            .expect("integrated prerequisite releases dependent");
        assert!(ready_after_integration.runnable);
        assert!(ready_after_integration
            .unsatisfied_dependency_ids
            .is_empty());
        let after_first = git::get_current_sha(&integration_path)
            .await
            .expect("integrated head");
        assert_eq!(
            first.target_after_sha.as_deref(),
            Some(after_first.as_str())
        );
        let replay = service
            .integrate(
                CollaborationActorSource::Human(human_id.clone()),
                &first_unit.id,
                &first_execution,
                "integrate-first-result",
            )
            .await
            .expect("idempotent integration replay");
        assert_eq!(replay.id, first.id);
        assert_eq!(
            git::get_current_sha(&integration_path).await.unwrap(),
            after_first
        );
        assert!(integration_path.join("result.txt").exists());

        let (second_unit, second_workspace, second_execution, second_sha) = add_result(
            &db,
            &manager,
            &repository_path,
            &task_id,
            &repo_id,
            &human_id,
            &agent_id,
            "conflicting integration",
            "conflict.txt",
            "WorkUnit version\n",
            &after_first,
        )
        .await;
        std::fs::write(
            integration_path.join("conflict.txt"),
            "Task target version\n",
        )
        .expect("target conflict");
        git::commit_all(&integration_path, "Task integration change")
            .await
            .expect("target commit");
        let before_conflict = git::get_current_sha(&integration_path)
            .await
            .expect("conflict target head");
        let conflict_idempotency_key = "integrate-conflicting-result";
        let interrupted_attempt_id = new_uuid_v4();
        let interrupted_at = now_rfc3339();
        WorkUnitRepo::begin_integration(
            &*db,
            db::CreateWorkUnitIntegration {
                id: interrupted_attempt_id.clone(),
                task_id: task_id.clone(),
                work_unit_id: second_unit.id.clone(),
                execution_id: second_execution.clone(),
                source_workspace_id: second_workspace.id.clone(),
                source_branch: second_workspace.branch.clone(),
                source_sha: second_sha.clone(),
                target_workspace_id: integration_workspace.id.clone(),
                target_branch: integration_workspace.branch.clone(),
                target_before_sha: before_conflict.clone(),
                operation_idempotency_key: conflict_idempotency_key.to_owned(),
                started_at: interrupted_at.clone(),
                created_at: interrupted_at,
            },
            event(
                "work_unit.integration_started",
                "work_unit_integration",
                &interrupted_attempt_id,
                &task_id,
                &human_id,
            ),
        )
        .await
        .expect("durable attempt exists before Git mutation");
        assert!(matches!(
            git::merge(&integration_path, &second_sha).await,
            Err(git::GitError::MergeConflict { .. })
        ));
        let conflict = service
            .integrate(
                CollaborationActorSource::Human(human_id.clone()),
                &second_unit.id,
                &second_execution,
                conflict_idempotency_key,
            )
            .await
            .expect("conflict is recorded");
        assert_eq!(conflict.outcome, WorkUnitIntegrationOutcome::Conflict);
        assert_eq!(conflict.source_sha, second_sha);
        assert_eq!(conflict.source_workspace_id, second_workspace.id);
        assert_eq!(conflict.target_before_sha, before_conflict);
        assert!(conflict.conflict_metadata_json.is_some());
        assert_eq!(
            git::get_current_sha(&integration_path).await.unwrap(),
            before_conflict
        );
        assert!(git::is_worktree_clean(&integration_path).await.unwrap());
        assert!(!git::detect_interrupted_merge(&integration_path)
            .await
            .unwrap());
        assert!(Path::new(&second_workspace.worktree_path).exists());
        let conflict_replay = service
            .integrate(
                CollaborationActorSource::Human(human_id.clone()),
                &second_unit.id,
                &second_execution,
                "integrate-conflicting-result",
            )
            .await
            .expect("conflict replay");
        assert_eq!(conflict_replay.id, conflict.id);
        assert_eq!(
            git::get_current_sha(&integration_path).await.unwrap(),
            before_conflict
        );

        // Hold an integration claim from one service instance. A second
        // WorkUnitService has a distinct in-process lock manager, and
        // MergeService has another one; all three must observe the durable
        // Task operation claim before any integration-workspace mutation.
        let (recovery_unit, recovery_workspace, recovery_execution, recovery_sha) = add_result(
            &db,
            &manager,
            &repository_path,
            &task_id,
            &repo_id,
            &human_id,
            &agent_id,
            "crash before integration Git",
            "recovery-before.txt",
            "recover without replaying a completed merge\n",
            &before_conflict,
        )
        .await;
        let active_operation = service
            .integration_operations
            .acquire(
                &task_id,
                db::TaskIntegrationOperationKind::WorkUnitIntegration,
                "cross-service-integration-owner",
            )
            .await
            .expect("first service claims Task integration scope");
        let durable_claim = db::TaskIntegrationOperationRepo::get_active_for_task(&*db, &task_id)
            .await
            .expect("durable claim lookup")
            .expect("claim is persisted");
        assert_eq!(
            durable_claim.kind,
            db::TaskIntegrationOperationKind::WorkUnitIntegration
        );
        // Each service uses a separate pool against the same on-disk database,
        // plus its own local WorkspaceExecutionLockManager and OS lock handle.
        let competing_db = Arc::new(SqliteDb::new(
            create_sqlite_pool(&database_url)
                .await
                .expect("independent service database pool"),
        ));
        let competing_service = WorkUnitService::new(
            Arc::clone(&competing_db),
            Arc::clone(&event_bus),
            workspace_root.clone(),
            Arc::new(RepoCacheLockManager::default()),
            Arc::new(WorkspaceExecutionLockManager::default()),
        );
        let integration_head_before_competitors = git::get_current_sha(&integration_path)
            .await
            .expect("integration target head before competitors");
        let competing_integration = competing_service
            .integrate(
                CollaborationActorSource::Human(human_id.clone()),
                &recovery_unit.id,
                &recovery_execution,
                "recover-before-git",
            )
            .await;
        assert!(matches!(
            competing_integration,
            Err(ServiceError::Conflict(_))
        ));
        let merge_service = crate::MergeService::new(
            Arc::clone(&competing_db),
            Arc::clone(&event_bus),
            workspace_root.clone(),
        );
        assert!(matches!(
            merge_service.merge(task_id.clone()).await,
            Err(ServiceError::Conflict(_))
        ));
        RepoRepo::update(
            &*db,
            db::UpdateRepo {
                id: repo_id.clone(),
                name: None,
                local_path: None,
                remote_url: None,
                work_mode: Some(WorkMode::PullRequest),
                default_branch: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("switch repo to PR mode for the publication contention path");
        assert!(matches!(
            merge_service.publish_pr(task_id.clone()).await,
            Err(ServiceError::Conflict(_))
        ));
        assert_eq!(
            git::get_current_sha(&integration_path).await.unwrap(),
            integration_head_before_competitors,
            "all competing operations fail before changing integration HEAD"
        );
        assert_eq!(
            db::TaskIntegrationOperationRepo::get_active_for_task(&*db, &task_id)
                .await
                .expect("active claim remains queryable")
                .expect("original claim remains active")
                .id,
            durable_claim.id
        );
        active_operation
            .finish(db::TaskIntegrationOperationStatus::Failed)
            .await
            .expect("release the test claim durably");
        let interrupted_owner = competing_service
            .integration_operations
            .acquire(
                &task_id,
                db::TaskIntegrationOperationKind::WorkUnitIntegration,
                "simulated-crashed-process",
            )
            .await
            .expect("second operation acquires after normal finish");
        let interrupted_operation_id: String = sqlx::query_scalar(
            "SELECT id FROM task_integration_operation WHERE task_id = ? AND status = 'running'",
        )
        .bind(&task_id)
        .fetch_one(db.pool())
        .await
        .expect("interrupted owner row");
        drop(interrupted_owner);
        let recovered_owner = service
            .integration_operations
            .acquire(
                &task_id,
                db::TaskIntegrationOperationKind::TaskMerge,
                "recovery-after-process-exit",
            )
            .await
            .expect("released process lock permits recovery");
        let abandoned_status: String =
            sqlx::query_scalar("SELECT status FROM task_integration_operation WHERE id = ?")
                .bind(&interrupted_operation_id)
                .fetch_one(db.pool())
                .await
                .expect("previous durable row remains auditable");
        assert_eq!(abandoned_status, "abandoned");
        recovered_owner
            .finish(db::TaskIntegrationOperationStatus::Failed)
            .await
            .expect("recovered claim closes");
        RepoRepo::update(
            &*db,
            db::UpdateRepo {
                id: repo_id.clone(),
                name: None,
                local_path: None,
                remote_url: None,
                work_mode: Some(WorkMode::DirectMerge),
                default_branch: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("restore direct merge mode");

        // Simulate a crash after the running integration attempt is persisted
        // but before Git is touched. Recovery may retry because HEAD is still
        // the exact recorded target-before SHA.
        let untouched_attempt = begin_running_integration(
            &db,
            &task_id,
            &recovery_unit,
            &recovery_workspace,
            &recovery_execution,
            &recovery_sha,
            &integration_workspace,
            &integration_head_before_competitors,
            "recover-before-git",
            &human_id,
        )
        .await;
        let recovered_untouched = service
            .integrate(
                CollaborationActorSource::Human(human_id.clone()),
                &recovery_unit.id,
                &recovery_execution,
                "recover-before-git",
            )
            .await
            .expect("retry from the pinned clean target");
        assert_eq!(recovered_untouched.id, untouched_attempt.id);
        assert_eq!(
            recovered_untouched.outcome,
            WorkUnitIntegrationOutcome::Success
        );

        // Simulate a crash after Git completed a fast-forward but before the
        // durable integration result was recorded. Recovery proves the exact
        // source SHA is already the target and converges to success.
        let after_untouched = recovered_untouched
            .target_after_sha
            .clone()
            .expect("successful target SHA");
        let (materialized_unit, materialized_workspace, materialized_execution, materialized_sha) =
            add_result(
                &db,
                &manager,
                &repository_path,
                &task_id,
                &repo_id,
                &human_id,
                &agent_id,
                "Git completed before durable success",
                "recovery-materialized.txt",
                "the exact result is already on target\n",
                &after_untouched,
            )
            .await;
        let materialized_attempt = begin_running_integration(
            &db,
            &task_id,
            &materialized_unit,
            &materialized_workspace,
            &materialized_execution,
            &materialized_sha,
            &integration_workspace,
            &after_untouched,
            "recover-after-git",
            &human_id,
        )
        .await;
        git::merge(&integration_path, &materialized_sha)
            .await
            .expect("previous attempt materializes the exact source commit");
        let materialized_head = git::get_current_sha(&integration_path)
            .await
            .expect("materialized target SHA");
        let recovered_materialized = service
            .integrate(
                CollaborationActorSource::Human(human_id.clone()),
                &materialized_unit.id,
                &materialized_execution,
                "recover-after-git",
            )
            .await
            .expect("materialized merge converges to success");
        assert_eq!(recovered_materialized.id, materialized_attempt.id);
        assert_eq!(
            recovered_materialized.outcome,
            WorkUnitIntegrationOutcome::Success
        );
        assert_eq!(
            recovered_materialized.target_after_sha.as_deref(),
            Some(materialized_head.as_str())
        );
        let replay_materialized = service
            .integrate(
                CollaborationActorSource::Human(human_id.clone()),
                &materialized_unit.id,
                &materialized_execution,
                "recover-after-git",
            )
            .await
            .expect("already recorded materialized result is idempotent");
        assert_eq!(replay_materialized.id, materialized_attempt.id);
        assert_eq!(
            git::get_current_sha(&integration_path).await.unwrap(),
            materialized_head
        );

        // An unrelated commit on the integration target cannot be attributed
        // to this attempt. Recovery records a mismatch and leaves that HEAD
        // untouched instead of merging on top or resetting it.
        let (mismatch_unit, mismatch_workspace, mismatch_execution, mismatch_sha) = add_result(
            &db,
            &manager,
            &repository_path,
            &task_id,
            &repo_id,
            &human_id,
            &agent_id,
            "unexpected target head",
            "recovery-mismatch-source.txt",
            "pinned source\n",
            &materialized_head,
        )
        .await;
        let mismatch_attempt = begin_running_integration(
            &db,
            &task_id,
            &mismatch_unit,
            &mismatch_workspace,
            &mismatch_execution,
            &mismatch_sha,
            &integration_workspace,
            &materialized_head,
            "recover-unexpected-head",
            &human_id,
        )
        .await;
        std::fs::write(
            integration_path.join("unattributed-target.txt"),
            "external\n",
        )
        .expect("external target change");
        let unexpected_head = git::commit_all(&integration_path, "unattributed target change")
            .await
            .expect("external target commit");
        let mismatch = service
            .integrate(
                CollaborationActorSource::Human(human_id.clone()),
                &mismatch_unit.id,
                &mismatch_execution,
                "recover-unexpected-head",
            )
            .await
            .expect("unexpected target fails closed as a durable outcome");
        assert_eq!(mismatch.id, mismatch_attempt.id);
        assert_eq!(mismatch.outcome, WorkUnitIntegrationOutcome::Failed);
        assert!(mismatch
            .conflict_metadata_json
            .as_deref()
            .is_some_and(|metadata| metadata.contains("target_head_mismatch")));
        assert_eq!(
            git::get_current_sha(&integration_path).await.unwrap(),
            unexpected_head,
            "recovery never changes an unattributed target head"
        );

        let cleanup = crate::workspace_cleanup::WorkspaceCleanupScheduler::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            workspace_root.clone(),
        )
        .with_repo_cache_locks(Arc::clone(&repo_cache_locks));
        cleanup.set_workspace_exec_locks(Arc::clone(&integration_locks));

        sqlx::query("UPDATE task SET status = 'in_progress', version = version + 1, updated_at = ? WHERE id = ?")
            .bind(now_rfc3339())
            .bind(&task_id)
            .execute(db.pool())
            .await
            .expect("Task admits repository Execution");
        let retryable_unit = service
            .create(
                CollaborationActorSource::Human(human_id.clone()),
                CreateWorkUnitInput {
                    task_id: task_id.clone(),
                    title: "recover cleaned WorkUnit workspace".to_owned(),
                    scope: "preserve branch commits after failed attempt".to_owned(),
                    role: "implementer".to_owned(),
                    parent_work_unit_id: None,
                    assigned_actor: Some(ActorRef::Agent(agent_id.clone())),
                    requires_integration: true,
                    provenance: None,
                },
            )
            .await
            .expect("open repository WorkUnit");
        let original_workspace = service
            .prepare_workspace(
                CollaborationActorSource::Human(human_id.clone()),
                &retryable_unit.id,
            )
            .await
            .expect("WorkUnit Workspace prepares");
        let first_attempt = running_repository_execution(
            &task_id,
            &original_workspace.id,
            &agent_id,
            original_workspace.before_sha.as_deref().unwrap(),
        );
        let first_attempt_id = first_attempt.id.clone();
        service
            .bind_execution(
                CollaborationActorSource::Execution(recovery_execution.clone()),
                &retryable_unit.id,
                retryable_unit.version,
                first_attempt,
            )
            .await
            .expect("first exact WorkUnit Execution and lease bind");
        let first_worktree = Path::new(&original_workspace.worktree_path);
        cleanup
            .cleanup_now(original_workspace.id.clone())
            .await
            .expect("active Execution prevents cleanup admission");
        assert!(first_worktree.exists());
        assert_eq!(
            WorkspaceRepo::get_by_id(&*db, &original_workspace.id)
                .await
                .expect("active Workspace lookup")
                .expect("Workspace remains present")
                .status,
            WorkspaceStatus::Ready
        );
        std::fs::write(
            first_worktree.join("kept-commit.txt"),
            "branch survives cleanup\n",
        )
        .expect("WorkUnit change writes");
        let preserved_commit = git::commit_all(first_worktree, "preserved failed-attempt commit")
            .await
            .expect("WorkUnit commit exists on its branch");
        ExecutionRepo::update(
            &*db,
            UpdateExecution {
                id: first_attempt_id.clone(),
                status: Some(ExecutionStatus::Failed),
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: Some(Some("simulated worker failure after commit".to_owned())),
                executor_config_snapshot_json: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("failed attempt remains historical");
        let failed_lease =
            db::WorkspaceLeaseRepo::get_active_for_work_unit(&*db, &retryable_unit.id)
                .await
                .expect("failed attempt lease lookup")
                .expect("failed attempt lease remains active until runner cleanup");
        db::WorkspaceLeaseRepo::revoke(
            &*db,
            &failed_lease.id,
            failed_lease.version,
            &now_rfc3339(),
        )
        .await
        .expect("failed attempt authority is revoked before workspace cleanup");
        WorkspaceRepo::claim_work_unit_cleanup(
            &*db,
            &original_workspace.id,
            &task_id,
            &retryable_unit.id,
            &now_rfc3339(),
        )
        .await
        .expect("durable cleanup claim survives a process restart")
        .expect("stopped Execution has no active lease authority");
        assert!(WorkspaceRepo::list_pending_cleanup(&*db, &now_rfc3339())
            .await
            .expect("Cleaning Workspaces are recovery candidates")
            .iter()
            .any(|pending| pending.id == original_workspace.id));
        cleanup
            .cleanup_now(original_workspace.id.clone())
            .await
            .expect("resume cleanup from Cleaning while the exact branch exists");
        assert!(!first_worktree.exists());
        assert!(Path::new(&second_workspace.worktree_path).exists());
        assert!(integration_path.exists());
        assert!(
            git::branch_exists(&repository_path, &second_workspace.branch)
                .await
                .expect("sibling branch remains untouched")
        );
        assert!(
            git::branch_exists(&repository_path, &original_workspace.branch)
                .await
                .expect("preserved branch lookup")
        );
        assert_eq!(
            WorkspaceRepo::get_by_id(&*db, &original_workspace.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            WorkspaceStatus::Cleaned
        );
        let recovered_workspace = service
            .prepare_workspace(
                CollaborationActorSource::Human(human_id.clone()),
                &retryable_unit.id,
            )
            .await
            .expect("Cleaned WorkUnit Workspace recovers from its branch");
        assert_eq!(recovered_workspace.id, original_workspace.id);
        assert_eq!(recovered_workspace.branch, original_workspace.branch);
        assert_eq!(
            recovered_workspace.before_sha,
            original_workspace.before_sha
        );
        assert_eq!(
            git::get_current_sha(Path::new(&recovered_workspace.worktree_path))
                .await
                .expect("recovered branch head"),
            preserved_commit
        );
        assert_eq!(recovered_workspace.status, WorkspaceStatus::Ready);
        let retry_attempt = running_repository_execution(
            &task_id,
            &recovered_workspace.id,
            &agent_id,
            &preserved_commit,
        );
        let retry_attempt_id = retry_attempt.id.clone();
        service
            .bind_execution(
                CollaborationActorSource::Execution(recovery_execution.clone()),
                &retryable_unit.id,
                retryable_unit.version,
                retry_attempt,
            )
            .await
            .expect("retry binds to the recovered Workspace");
        let retry_lease =
            db::WorkspaceLeaseRepo::get_active_for_work_unit(&*db, &retryable_unit.id)
                .await
                .expect("retried WorkUnit lease lookup")
                .expect("retry receives exact active lease");
        assert_eq!(
            retry_lease.workspace_id.as_deref(),
            Some(original_workspace.id.as_str())
        );
        assert_eq!(retry_lease.execution_id, retry_attempt_id);
        ExecutionRepo::update(
            &*db,
            UpdateExecution {
                id: retry_attempt_id.clone(),
                status: Some(ExecutionStatus::Cancelled),
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                agent_session_id: None,
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
        .expect("retry attempt can stop");
        let retry_lease =
            db::WorkspaceLeaseRepo::get_active_for_work_unit(&*db, &retryable_unit.id)
                .await
                .expect("retry lease lookup")
                .expect("retry lease remains active until runner cleanup");
        db::WorkspaceLeaseRepo::revoke(&*db, &retry_lease.id, retry_lease.version, &now_rfc3339())
            .await
            .expect("retry authority is revoked before cleanup");
        cleanup
            .cleanup_now(recovered_workspace.id.clone())
            .await
            .expect("clean recovered exact Workspace");

        WorkspaceRepo::claim_work_unit_cleanup(
            &*db,
            &second_workspace.id,
            &task_id,
            &second_unit.id,
            &now_rfc3339(),
        )
        .await
        .expect("second sibling cleanup claim commits")
        .expect("second sibling has no active authority");
        manager
            .cleanup_work_unit_worktree(
                &repository_path.to_string_lossy(),
                &task_id,
                &second_unit.id,
                &second_workspace.id,
                &repo_id,
            )
            .await
            .expect("simulate process exit after removing only the second worktree");
        cleanup
            .cleanup_now(second_workspace.id.clone())
            .await
            .expect("resume cleanup when the exact branch remains but worktree is gone");
        assert!(!Path::new(&second_workspace.worktree_path).exists());
        assert!(Path::new(&first_workspace.worktree_path).exists());
        assert!(integration_path.exists());
        assert!(
            git::branch_exists(&repository_path, &second_workspace.branch)
                .await
                .expect("WorkUnit branch lookup")
        );
        assert_eq!(
            WorkspaceRepo::get_by_id(&*db, &second_workspace.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            WorkspaceStatus::Cleaned
        );

        let (missing_branch_unit, missing_branch_workspace, _, _) = add_result(
            &db,
            &manager,
            &repository_path,
            &task_id,
            &repo_id,
            &human_id,
            &agent_id,
            "missing cleanup branch",
            "missing-branch.txt",
            "preserve or require explicit recovery\n",
            &base_sha,
        )
        .await;
        WorkspaceRepo::claim_work_unit_cleanup(
            &*db,
            &missing_branch_workspace.id,
            &task_id,
            &missing_branch_unit.id,
            &now_rfc3339(),
        )
        .await
        .expect("missing-branch cleanup claim commits")
        .expect("completed WorkUnit has no active execution or lease");
        manager
            .cleanup_work_unit_worktree(
                &repository_path.to_string_lossy(),
                &task_id,
                &missing_branch_unit.id,
                &missing_branch_workspace.id,
                &repo_id,
            )
            .await
            .expect("simulate crash after worktree removal");
        run_git(
            &repository_path,
            &["branch", "-D", &missing_branch_workspace.branch],
        )
        .await;
        cleanup
            .cleanup_now(missing_branch_workspace.id.clone())
            .await
            .expect("scheduled cleanup reports a durable recovery-required state");
        let missing_branch_record = WorkspaceRepo::get_by_id(&*db, &missing_branch_workspace.id)
            .await
            .expect("missing-branch workspace lookup")
            .expect("workspace remains available for explicit recovery");
        assert_eq!(missing_branch_record.status, WorkspaceStatus::Cleaning);
        assert!(missing_branch_record
            .error
            .as_deref()
            .is_some_and(|error| error.contains("explicit recovery or reset is required")));
        assert!(Path::new(&first_workspace.worktree_path).exists());
        assert!(integration_path.exists());

        cleanup
            .schedule(&integration_workspace.id, Duration::ZERO)
            .await
            .expect("integration cleanup schedule");
        cleanup
            .cleanup_now(integration_workspace.id.clone())
            .await
            .expect("integration workspace remains protected");
        assert!(integration_path.exists());
        assert_eq!(
            WorkspaceRepo::get_by_id(&*db, &integration_workspace.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            WorkspaceStatus::Ready
        );

        let readiness_unit = service
            .create(
                CollaborationActorSource::Human(human_id.clone()),
                CreateWorkUnitInput {
                    task_id: task_id.clone(),
                    title: "allocation readiness".to_owned(),
                    scope: "check active membership".to_owned(),
                    role: "implementer".to_owned(),
                    parent_work_unit_id: None,
                    assigned_actor: Some(ActorRef::Agent(agent_id.clone())),
                    requires_integration: false,
                    provenance: None,
                },
            )
            .await
            .expect("assigned WorkUnit");
        let ready = service
            .readiness(
                CollaborationActorSource::Human(human_id.clone()),
                &readiness_unit.id,
            )
            .await
            .expect("active allocation readiness");
        assert!(ready.runnable);
        assert!(!ready.ready_for_allocation);

        let membership = RoleMembershipRepo::list_by_role(&*db, &task_role_id, true)
            .await
            .expect("TaskRole memberships")
            .into_iter()
            .find(|membership| membership.actor_id == agent_id)
            .expect("Agent membership");
        let suspended = RoleMembershipRepo::update(
            &*db,
            UpdateRoleMembership {
                id: membership.id.clone(),
                expected_version: membership.version,
                status: RoleMembershipStatus::Suspended,
                updated_at: now_rfc3339(),
                ended_at: None,
            },
        )
        .await
        .expect("suspend allocation membership");
        let not_ready = service
            .readiness(
                CollaborationActorSource::Human(human_id.clone()),
                &readiness_unit.id,
            )
            .await
            .expect("suspended allocation readiness");
        assert!(!not_ready.runnable);
        assert!(not_ready.ready_for_allocation);

        RoleMembershipRepo::update(
            &*db,
            UpdateRoleMembership {
                id: suspended.id,
                expected_version: suspended.version,
                status: RoleMembershipStatus::Ended,
                updated_at: now_rfc3339(),
                ended_at: Some(now_rfc3339()),
            },
        )
        .await
        .expect("end allocation membership");
        let ended_not_ready = service
            .readiness(
                CollaborationActorSource::Human(human_id),
                &readiness_unit.id,
            )
            .await
            .expect("ended allocation readiness");
        assert!(!ended_not_ready.runnable);
        assert!(ended_not_ready.ready_for_allocation);
    }
}
