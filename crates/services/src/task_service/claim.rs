use super::workspace::prepare_workspace_owned;
use super::*;
use crate::workflow::{actions::DispatchRoleAgent, HookAction, HookContext};
use api_types::{Actor, StateKind, SystemComponent, WorkflowDefinition};
use sqlx::Row;

impl TaskService {
    pub async fn claim_task(
        &self,
        task_id: impl Into<String>,
        assignee: Assignee,
        overrides: Option<ExecutionOverrides>,
    ) -> Result<ClaimedTask> {
        let task_id = task_id.into();
        validate_required("task_id", &task_id)?;

        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        if task.parent_task_id.is_some() {
            return Err(ServiceError::subtask_managed_by_root(
                task_id.clone(),
                task.parent_task_id.clone().unwrap_or_default(),
            ));
        }
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        // Do this before workspace preparation so a blocked Charter-backed
        // implementation Task can never receive a workspace/lease as a side
        // effect of an attempted claim.
        self.ensure_task_runnable(&task).await?;
        if matches!(&assignee, Assignee::Agent(_)) && project.paused_at.is_some() {
            return Err(ServiceError::ProjectPaused {
                project_id: project.id,
            });
        }
        let workflow = WorkflowEngine::resolve_workflow_for_task(
            &task,
            &project.workflow_definition,
            &Actor::system(SystemComponent::General),
        );
        WorkflowEngine::validate_claimable(&workflow, &task.status)?;
        let target_status = resolve_claim_target(&workflow, &task.status)?;
        let target_role = workflow
            .states
            .iter()
            .find(|state| state.name == target_status)
            .and_then(crate::workflow::effective_role)
            .map(str::to_owned);
        let capacity_statuses = workflow_capacity_statuses(&workflow);
        let (assignee_type, agent, assignee_id, max_concurrent_tasks, event_assignee_id) =
            match assignee {
                Assignee::Agent(agent_id) => {
                    validate_required("agent_id", &agent_id)?;
                    let agent = AgentRepo::get_by_id(&*self.db, &agent_id)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
                    if agent.paused {
                        return Err(ServiceError::AgentPaused {
                            agent_id: agent.id.clone(),
                        });
                    }
                    let max_concurrent_tasks = agent.max_concurrent_tasks;
                    (
                        "agent".to_owned(),
                        Some(agent),
                        Some(agent_id.clone()),
                        max_concurrent_tasks,
                        Some(agent_id),
                    )
                }
                Assignee::User(user_handle) => {
                    validate_required("user_handle", &user_handle)?;
                    (
                        "user".to_owned(),
                        None,
                        Some(user_handle.clone()),
                        i64::MAX,
                        None,
                    )
                }
            };
        if let Some(claiming_agent) = agent.as_ref() {
            self.ensure_repository_worker_identity(&task.project_id, &claiming_agent.id)
                .await?;
        }
        if let Some(role_name) = target_role.as_deref() {
            let actor_kind = if agent.is_some() {
                db::ActorKind::Agent
            } else {
                db::ActorKind::Human
            };
            if let Some(actor_id) = assignee_id.as_deref() {
                self.ensure_claim_role_available(&task.id, role_name, actor_kind, actor_id)
                    .await?;
            }
        }
        let previous_status = task.status.clone();
        let (workspace, workspace_created_by_attempt) = prepare_workspace_owned(
            &self.db,
            &self.workspace_root,
            &task,
            &task_id,
            self.repo_cache_locks.clone(),
        )
        .await?;
        let now = now_rfc3339();
        let execution_id = new_uuid_v4();
        let executor_config_snapshot_json = match agent.as_ref() {
            Some(agent) => {
                match build_executor_config_snapshot(&self.db, &task, agent, overrides).await {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        create_failed_execution_record(
                            &self.db,
                            &task_id,
                            agent,
                            &workspace,
                            &execution_id,
                            error.to_string(),
                        )
                        .await?;
                        return Err(error);
                    }
                }
            }
            None => None,
        };
        let agent_id = agent.as_ref().map(|agent| agent.id.clone());
        let mut transaction = self.db.pool().begin().await.map_err(DbError::from)?;
        let claimed = TaskRepo::claim(
            &*self.db,
            &mut transaction,
            ClaimTask {
                task_id: task_id.clone(),
                assignee_type,
                assignee_id,
                expected_version: task.version,
                source_status: task.status.clone(),
                target_status: target_status.clone(),
                capacity_statuses,
                execution: CreateExecution {
                    id: execution_id.clone(),
                    task_id: task_id.clone(),
                    agent_id: agent_id.clone(),
                    role: target_role.clone().unwrap_or_else(|| "executor".to_owned()),
                    status: ExecutionStatus::Running,
                    stop_reason: None,
                    stopped_by: None,
                    resume_policy: None,
                    stopped_at: None,
                    parent_execution_id: None,
                    agent_session_id: None,
                    agent_message_id: None,
                    last_activity_at: None,
                    summary: None,
                    logs_path: None,
                    before_sha: None,
                    after_sha: None,
                    error: None,
                    executor_config_snapshot_json,
                    workspace_id: Some(workspace.id.clone()),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
                max_concurrent_tasks,
                claimed_at: now,
            },
        )
        .await;
        let mut claimed = match claimed {
            Ok(claimed) => claimed,
            Err(error) => {
                drop(transaction);
                if workspace_created_by_attempt {
                    self.cleanup_fresh_execution_workspace(&task, &workspace)
                        .await;
                }
                return Err(error.into());
            }
        };
        let membership_result: std::result::Result<(), ServiceError> = async {
            if let (Some(role_name), Some(claiming_agent_id)) =
                (target_role.as_deref(), agent_id.as_deref())
            {
            let role_now = now_rfc3339();
            let canonical_role = db::canonical_task_role_name(role_name).ok_or_else(|| {
                ServiceError::invalid_operation(format!(
                    "workflow role '{role_name}' cannot receive a membership"
                ))
            })?;
            let task_role_id = sqlx::query_scalar::<_, String>(
                "SELECT id FROM task_role WHERE task_id = ? AND role = ?",
            )
            .bind(&claimed.task.id)
            .bind(&canonical_role)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(DbError::from)?;
            let task_role_id = if let Some(task_role_id) = task_role_id {
                task_role_id
            } else {
                let task_role_id = new_uuid_v4();
                sqlx::query(
                    "INSERT INTO task_role
                        (id, task_id, role, coordination_mode, policy_json, version, created_at, updated_at)
                     VALUES (?, ?, ?, NULL, '{}', 1, ?, ?)",
                )
                .bind(&task_role_id)
                .bind(&claimed.task.id)
                .bind(&canonical_role)
                .bind(&role_now)
                .bind(&role_now)
                .execute(&mut *transaction)
                .await
                .map_err(DbError::from)?;
                task_role_id
            };
            let current_memberships = sqlx::query(
                "SELECT actor_kind, actor_id, status
                 FROM role_membership
                 WHERE task_role_id = ? AND status IN ('active', 'suspended')",
            )
            .bind(&task_role_id)
            .fetch_all(&mut *transaction)
            .await
            .map_err(DbError::from)?;
            if !current_memberships.is_empty() {
                let current_membership = current_memberships.iter().find(|membership| {
                    membership.get::<String, _>("actor_kind") == "agent"
                        && membership.get::<String, _>("actor_id") == claiming_agent_id
                });
                let Some(current_membership) = current_membership else {
                    return Err(ServiceError::conflict(format!(
                        "role '{role_name}' is eligible for a different Actor"
                    )));
                };
                let status: String = current_membership.get("status");
                if status != "active" {
                    return Err(ServiceError::conflict(format!(
                        "role '{role_name}' membership is suspended"
                    )));
                }
            } else {
                sqlx::query(
                    "INSERT INTO role_membership
                        (id, task_role_id, actor_kind, actor_id, status, version, created_at, updated_at, ended_at)
                     VALUES (?, ?, 'agent', ?, 'active', 1, ?, ?, NULL)",
                )
                .bind(new_uuid_v4())
                .bind(&task_role_id)
                .bind(claiming_agent_id)
                .bind(&role_now)
                .bind(&role_now)
                .execute(&mut *transaction)
                .await
                .map_err(DbError::from)?;
            }

            // Keep the old singleton row as a deterministic, non-authoritative
            // projection for legacy workspace SQL and public clients.  The
            // membership above is the only eligibility truth; the claimant is
            // not automatically the compatibility representative when another
            // eligible Agent was already present.
            let projection = sqlx::query(
                "SELECT actor_kind, actor_id
                 FROM role_membership
                 WHERE task_role_id = ? AND status = 'active'
                 ORDER BY CASE actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                          created_at, id
                 LIMIT 1",
            )
            .bind(&task_role_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(DbError::from)?;
            let projection_kind: String = projection.get("actor_kind");
            let projection_id: String = projection.get("actor_id");
            let projection_assignee_type = if projection_kind == "agent" {
                "agent"
            } else {
                "user"
            };
            sqlx::query(
                "INSERT INTO task_role_assignment
                    (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(task_id, role_name) DO UPDATE SET
                    assignee_type = excluded.assignee_type,
                    assignee_id = excluded.assignee_id,
                    updated_at = excluded.updated_at",
            )
            .bind(new_uuid_v4())
            .bind(&claimed.task.id)
            .bind(role_name)
            .bind(projection_assignee_type)
            .bind(&projection_id)
            .bind(&role_now)
            .bind(&role_now)
            .execute(&mut *transaction)
            .await
            .map_err(DbError::from)?;
            if canonical_role == "implementer" {
                sqlx::query(
                    "UPDATE task
                     SET assignee_type = ?, assignee_id = ?, updated_at = ?
                     WHERE id = ?",
                )
                .bind(projection_assignee_type)
                .bind(&projection_id)
                .bind(&role_now)
                .bind(&claimed.task.id)
                .execute(&mut *transaction)
                .await
                .map_err(DbError::from)?;
                claimed.task.assignee_type = Some(projection_assignee_type.to_owned());
                claimed.task.assignee_id = Some(projection_id);
            }
            }
            if agent_id.is_none() {
                if let (Some(role_name), Some(user_id)) =
                    (target_role.as_deref(), claimed.task.assignee_id.as_deref())
                {
                // Existing manual-handoff fixtures may still use the legacy
                // `human` marker. It is not an Actor identity and therefore
                // must not be materialized as a synthetic User membership.
                let user_exists: i64 = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM user WHERE id = ?)",
                )
                .bind(user_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(DbError::from)?;
                if user_exists != 0 {
                    let role_now = now_rfc3339();
                    let canonical_role = db::canonical_task_role_name(role_name).ok_or_else(|| {
                        ServiceError::invalid_operation(format!(
                            "workflow role '{role_name}' cannot receive a membership"
                        ))
                    })?;
                    let task_role_id = sqlx::query_scalar::<_, String>(
                        "SELECT id FROM task_role WHERE task_id = ? AND role = ?",
                    )
                    .bind(&claimed.task.id)
                    .bind(&canonical_role)
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(DbError::from)?;
                    let task_role_id = if let Some(task_role_id) = task_role_id {
                        task_role_id
                    } else {
                        let task_role_id = new_uuid_v4();
                        sqlx::query(
                            "INSERT INTO task_role
                                (id, task_id, role, coordination_mode, policy_json, version, created_at, updated_at)
                             VALUES (?, ?, ?, NULL, '{}', 1, ?, ?)",
                        )
                        .bind(&task_role_id)
                        .bind(&claimed.task.id)
                        .bind(&canonical_role)
                        .bind(&role_now)
                        .bind(&role_now)
                        .execute(&mut *transaction)
                        .await
                        .map_err(DbError::from)?;
                        task_role_id
                    };
                    let current_count: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM role_membership
                         WHERE task_role_id = ? AND status IN ('active', 'suspended')",
                    )
                    .bind(&task_role_id)
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(DbError::from)?;
                    let current_human = sqlx::query(
                        "SELECT status FROM role_membership
                         WHERE task_role_id = ? AND actor_kind = 'human' AND actor_id = ?
                           AND status IN ('active', 'suspended')",
                    )
                    .bind(&task_role_id)
                    .bind(user_id)
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(DbError::from)?;
                    if current_human
                        .as_ref()
                        .is_some_and(|row| row.get::<String, _>("status") == "suspended")
                    {
                        return Err(ServiceError::conflict(format!(
                            "role '{role_name}' membership is suspended"
                        )));
                    }
                    if current_human.is_none() {
                        if current_count > 0 {
                            let mode: Option<String> = sqlx::query_scalar(
                                "SELECT coordination_mode FROM task_role WHERE id = ?",
                            )
                            .bind(&task_role_id)
                            .fetch_one(&mut *transaction)
                            .await
                            .map_err(DbError::from)?;
                            if mode.is_none() {
                                return Err(ServiceError::invalid_operation(
                                    "set coordination_mode before adding a second current member",
                                ));
                            }
                        }
                        sqlx::query(
                            "INSERT INTO role_membership
                                (id, task_role_id, actor_kind, actor_id, status, version, created_at, updated_at, ended_at)
                             VALUES (?, ?, 'human', ?, 'active', 1, ?, ?, NULL)",
                        )
                        .bind(new_uuid_v4())
                        .bind(&task_role_id)
                        .bind(user_id)
                        .bind(&role_now)
                        .bind(&role_now)
                        .execute(&mut *transaction)
                        .await
                        .map_err(DbError::from)?;
                    }
                    let projection = sqlx::query(
                        "SELECT actor_kind, actor_id
                         FROM role_membership
                         WHERE task_role_id = ? AND status = 'active'
                         ORDER BY CASE actor_kind WHEN 'agent' THEN 0 ELSE 1 END, created_at, id
                         LIMIT 1",
                    )
                    .bind(&task_role_id)
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(DbError::from)?;
                    if let Some(projection) = projection {
                        let actor_kind: String = projection.get("actor_kind");
                        let actor_id: String = projection.get("actor_id");
                        let projected_assignee_type =
                            if actor_kind == "agent" { "agent" } else { "user" };
                        sqlx::query(
                            "INSERT INTO task_role_assignment
                                (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at)
                             VALUES (?, ?, ?, ?, ?, ?, ?)
                             ON CONFLICT(task_id, role_name) DO UPDATE SET
                                assignee_type = excluded.assignee_type,
                                assignee_id = excluded.assignee_id,
                                updated_at = excluded.updated_at",
                        )
                        .bind(new_uuid_v4())
                        .bind(&claimed.task.id)
                        .bind(role_name)
                        .bind(projected_assignee_type)
                        .bind(&actor_id)
                        .bind(&role_now)
                        .bind(&role_now)
                        .execute(&mut *transaction)
                        .await
                        .map_err(DbError::from)?;
                        if canonical_role == "implementer" {
                            sqlx::query(
                                "UPDATE task
                                 SET assignee_type = ?, assignee_id = ?, updated_at = ?
                                 WHERE id = ?",
                            )
                            .bind(projected_assignee_type)
                            .bind(&actor_id)
                            .bind(&role_now)
                            .bind(&claimed.task.id)
                            .execute(&mut *transaction)
                            .await
                            .map_err(DbError::from)?;
                            claimed.task.assignee_type =
                                Some(projected_assignee_type.to_owned());
                            claimed.task.assignee_id = Some(actor_id);
                        }
                    }
                }
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = membership_result {
            drop(transaction);
            if workspace_created_by_attempt {
                self.cleanup_fresh_execution_workspace(&task, &workspace)
                    .await;
            }
            return Err(error);
        }
        // Human claims remain user-managed work and do not mint repository
        // authority. Only a scheduler-dispatched Agent Worker/reviewer gets
        // an execution-scoped WorkspaceLease.
        let lease = if agent_id.is_some() {
            match self
                .issue_workspace_lease_in_tx(
                    &mut transaction,
                    &claimed.task,
                    &workspace,
                    target_role.as_deref().unwrap_or("executor"),
                    agent_id.as_deref(),
                    &execution_id,
                )
                .await
            {
                Ok(lease) => Some(lease),
                Err(error) => {
                    drop(transaction);
                    if workspace_created_by_attempt {
                        self.cleanup_fresh_execution_workspace(&task, &workspace)
                            .await;
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        if let Err(error) = transaction.commit().await.map_err(DbError::from) {
            // The commit may have succeeded at SQLite despite a transport
            // error; revoke the lease idempotently so a crashed claimant can
            // never retain repository authority.
            if let Some(lease) = lease.as_ref() {
                self.revoke_workspace_lease(lease).await;
            }
            if workspace_created_by_attempt {
                self.cleanup_fresh_execution_workspace(&task, &workspace)
                    .await;
            }
            return Err(error.into());
        }

        if let Some(agent_id) = event_assignee_id {
            self.publish(ForgeEvent {
                event_type: "task.assigned".to_owned(),
                entity_id: claimed.task.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::TaskAssigned {
                    project_id: claimed.task.project_id.clone(),
                    agent_id,
                    execution_id: execution_id.clone(),
                },
            });
        } else {
            self.publish(ForgeEvent {
                event_type: "task.updated".to_owned(),
                entity_id: claimed.task.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::TaskUpdated {
                    project_id: claimed.task.project_id.clone(),
                },
            });
        }
        // Claims move work into a workflow-resolved active state, so subscribers need the same status event as manual transitions.
        self.publish(ForgeEvent {
            event_type: "task.status_changed".to_owned(),
            entity_id: claimed.task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskStatusChanged {
                project_id: claimed.task.project_id.clone(),
                old_status: task.status.to_string(),
                new_status: claimed.task.status.to_string(),
            },
        });

        if agent_id.is_some() {
            if claimed.task.parent_task_id.is_none() {
                if let Err(error) = super::execution::subtasks::begin_next_turn(
                    &self.db,
                    &self.event_bus,
                    &self.workspace_root,
                    &claimed.task.id,
                )
                .await
                {
                    tracing::warn!(
                        task_id = %claimed.task.id,
                        %error,
                        "failed to begin subtask sequence, dispatching normally"
                    );
                }
            }
            self.dispatch_claim_state_role_agent(
                &claimed.task,
                &previous_status,
                &execution_id,
                claimed.execution.workspace_id.clone(),
                agent_id.as_deref(),
            )
            .await;
        }

        Ok(claimed)
    }

    async fn dispatch_claim_state_role_agent(
        &self,
        task: &Task,
        previous_status: &str,
        execution_id: &str,
        workspace_id: Option<String>,
        execution_agent_id: Option<&str>,
    ) {
        let Ok(Some(project)) = ProjectRepo::get_by_id(&*self.db, &task.project_id).await else {
            return;
        };
        let workflow = WorkflowEngine::resolve_workflow_for_task(
            task,
            &project.workflow_definition,
            &Actor::system(SystemComponent::General),
        );
        let Some(state) = workflow
            .states
            .iter()
            .find(|state| state.name == task.status)
        else {
            return;
        };
        if !state
            .hooks
            .on_enter
            .iter()
            .any(|hook| hook.action == "dispatch_role_agent")
        {
            return;
        }
        let gate_config = state.gate_config.clone();
        let state_config = state.config.clone();
        let trigger_agent_id = execution_agent_id.or_else(|| {
            (task.assignee_type.as_deref() == Some("agent"))
                .then_some(task.assignee_id.as_deref())
                .flatten()
        });
        let triggered_by = match trigger_agent_id {
            Some(agent_id) => Actor::Agent {
                agent_id: agent_id.to_owned(),
                execution_id: Some(execution_id.to_owned()),
            },
            None if task.assignee_type.as_deref() == Some("user") => {
                Actor::user(UserActionSource::Transition)
            }
            None => Actor::system(SystemComponent::Workflow),
        };

        let ctx = HookContext {
            task_id: task.id.clone(),
            project_id: task.project_id.clone(),
            from_state: previous_status.to_owned(),
            to_state: task.status.clone(),
            db: Arc::clone(&self.db),
            event_bus: Arc::clone(&self.event_bus),
            gate_config,
            workflow: Arc::new(workflow),
            triggered_by,
            review_runner: self.review_runner.clone(),
            merge_service: self.merge_service.clone(),
            cleanup_scheduler: self.cleanup_scheduler.clone(),
            task_executor: self.task_executor.clone(),
            daemon_connections: self.daemon_connections.clone(),
            workspace_exec_locks: self.workspace_exec_locks.clone(),
            terminal_activity: self.terminal_activity.clone(),
            workspace_root: self.workspace_root.clone(),
            repo_cache_locks: self.repo_cache_locks.clone(),
            workspace_id,
            agent_id: trigger_agent_id.map(str::to_owned),
            execution_id: Some(execution_id.to_owned()),
            state_config,
        };
        let _ = DispatchRoleAgent.execute(&ctx).await;
    }

    pub(super) async fn role_for_state(
        &self,
        task: &Task,
        state_name: &str,
    ) -> Result<Option<String>> {
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let workflow = WorkflowEngine::resolve_workflow_for_task(
            task,
            &project.workflow_definition,
            &Actor::system(SystemComponent::General),
        );
        Ok(workflow
            .states
            .iter()
            .find(|state| state.name == state_name)
            .and_then(crate::workflow::effective_role)
            .map(str::to_owned))
    }

    async fn ensure_claim_role_available(
        &self,
        task_id: &str,
        role_name: &str,
        actor_kind: db::ActorKind,
        actor_id: &str,
    ) -> Result<()> {
        match crate::task_service::current_role_memberships_authoritative(
            &self.db,
            task_id,
            role_name,
        )
        .await?
        {
            Some(memberships) => {
                let matching = memberships.iter().find(|membership| {
                    membership.actor_kind == actor_kind && membership.actor_id == actor_id
                });
                if let Some(membership) = matching {
                    if membership.status != db::RoleMembershipStatus::Active {
                        return Err(ServiceError::conflict(format!(
                            "role '{role_name}' membership is suspended"
                        )));
                    }
                    return Ok(());
                }
                if !memberships.is_empty() {
                    return Err(ServiceError::conflict(format!(
                        "role '{role_name}' is eligible for a different Actor"
                    )));
                }
                if actor_kind == db::ActorKind::Human
                    && UserRepo::get_user_by_id(&*self.db, actor_id)
                        .await?
                        .is_none()
                {
                    return Err(ServiceError::not_found("user", actor_id.to_owned()));
                }
            }
            None => {
                if actor_kind != db::ActorKind::Agent {
                    return Ok(());
                }
                let existing = TaskRoleAssignmentRepo::get_by_task_and_role(
                    &*self.db,
                    task_id,
                    role_name,
                )
                .await?;
                if existing.as_ref().is_some_and(|assignment| {
                    assignment.assignee_type != Some(AssigneeKind::Agent)
                        || assignment.assignee_id.as_deref() != Some(actor_id)
                }) {
                    return Err(ServiceError::conflict(format!(
                        "role '{role_name}' is assigned to a different agent"
                    )));
                }
            }
        }
        Ok(())
    }
}

fn resolve_claim_target(workflow: &WorkflowDefinition, current_status: &str) -> Result<String> {
    let source_kind = workflow.state_kind(current_status);
    let targets = workflow
        .outgoing_trigger_targets(current_status)
        .filter(|(trigger, _)| {
            !trigger.system_only()
                || matches!(source_kind, Some(StateKind::Initial | StateKind::Custom))
        })
        .filter_map(|(_, target_name)| {
            workflow
                .states
                .iter()
                .find(|target| target.name == target_name)
                .map(|target| (target.name.clone(), target.kind))
        })
        .collect::<Vec<_>>();
    let target = targets
        .iter()
        .find(|(_, kind)| *kind == StateKind::Active)
        .or_else(|| targets.iter().find(|(_, kind)| *kind == StateKind::Gate))
        .map(|(name, _)| name.clone());

    target.ok_or_else(|| {
        ServiceError::invalid_operation(format!(
            "task in state '{current_status}' has no claimable active transition"
        ))
    })
}

fn workflow_capacity_statuses(workflow: &WorkflowDefinition) -> Vec<String> {
    workflow
        .states
        .iter()
        .filter(|state| matches!(state.kind, StateKind::Active | StateKind::Gate))
        .map(|state| state.name.clone())
        .collect()
}
