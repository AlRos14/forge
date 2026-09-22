use super::*;
use events::RoleAssignmentSnapshot;
use sqlx::{Row, Sqlite, Transaction};

impl TaskService {
    pub async fn on_agent_deleted(&self, agent_id: &str) -> Result<()> {
        validate_required("agent_id", agent_id)?;
        let mut transaction = self.db.pool().begin().await?;
        let events = self
            .on_agent_deleted_in_tx(&mut transaction, agent_id)
            .await?;
        transaction.commit().await?;
        self.publish_role_sweep_events(events);
        Ok(())
    }

    pub(crate) async fn on_agent_deleted_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<Vec<RoleSweepEvent>> {
        validate_required("agent_id", agent_id)?;
        let rows = sqlx::query(
            "SELECT id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at FROM task_role_assignment WHERE assignee_type = 'agent' AND assignee_id = ? ORDER BY task_id, role_name",
        )
        .bind(agent_id)
        .fetch_all(&mut **transaction)
        .await?;

        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let previous_assignment = TaskRoleAssignment {
                id: row.try_get("id")?,
                task_id: row.try_get("task_id")?,
                role_name: row.try_get("role_name")?,
                assignee_type: Some(AssigneeKind::Agent),
                assignee_id: row.try_get("assignee_id")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
            };
            let mut new_assignment = previous_assignment.clone();
            new_assignment.assignee_id = None;
            events.push(RoleSweepEvent {
                task_id: previous_assignment.task_id.clone(),
                role_name: previous_assignment.role_name.clone(),
                previous_assignment,
                new_assignment,
            });
        }

        // The compatibility row is optional for roles created directly by
        // the new membership API.  Track every affected Task, not only Tasks
        // that happen to have a legacy singleton row, so a surviving member
        // can still rebuild the task-level implementer projection.
        let affected_task_rows = sqlx::query(
            "SELECT task_id
             FROM task_role tr
             JOIN role_membership rm ON rm.task_role_id = tr.id
             WHERE rm.actor_kind = 'agent' AND rm.actor_id = ?
               AND rm.status IN ('active', 'suspended')
             UNION
             SELECT task_id FROM task_role_assignment
             WHERE assignee_type = 'agent' AND assignee_id = ?
             UNION
             SELECT id AS task_id FROM task
             WHERE assignee_type = 'agent' AND assignee_id = ?
             ORDER BY task_id",
        )
        .bind(agent_id)
        .bind(agent_id)
        .bind(agent_id)
        .fetch_all(&mut **transaction)
        .await?;
        let affected_task_ids = affected_task_rows
            .iter()
            .map(|row| row.try_get::<String, _>("task_id"))
            .collect::<std::result::Result<Vec<_>, _>>()?;

        sqlx::query(
            "UPDATE task_role_assignment SET assignee_id = NULL, updated_at = ? WHERE assignee_type = 'agent' AND assignee_id = ?",
        )
        .bind(now_rfc3339())
        .bind(agent_id)
        .execute(&mut **transaction)
        .await?;
        sqlx::query(
            "UPDATE role_membership
             SET status = 'ended', ended_at = ?, updated_at = ?, version = version + 1
             WHERE actor_kind = 'agent' AND actor_id = ? AND status IN ('active', 'suspended')",
        )
        .bind(now_rfc3339())
        .bind(now_rfc3339())
        .bind(agent_id)
        .execute(&mut **transaction)
        .await?;
        sqlx::query(
            "UPDATE task SET assignee_id = NULL, updated_at = ? WHERE assignee_type = 'agent' AND assignee_id = ?",
        )
        .bind(now_rfc3339())
        .bind(agent_id)
        .execute(&mut **transaction)
        .await?;

        // Rebuild every affected singleton projection from the surviving
        // membership set.  A removed Agent must not leave the compatibility
        // row pointing at the deleted identity or hide another active member.
        for task_id in &affected_task_ids {
            let legacy_rows = sqlx::query(
                "SELECT id, role_name
                 FROM task_role_assignment
                 WHERE task_id = ?",
            )
            .bind(task_id)
            .fetch_all(&mut **transaction)
            .await?;
            for legacy_row in legacy_rows {
                let legacy_id: String = legacy_row.try_get("id")?;
                let role_name: String = legacy_row.try_get("role_name")?;
                let Some(canonical_role) = db::canonical_task_role_name(&role_name) else {
                    continue;
                };
                let member = sqlx::query(
                    "SELECT rm.actor_kind, rm.actor_id
                     FROM task_role tr
                     JOIN role_membership rm ON rm.task_role_id = tr.id
                     WHERE tr.task_id = ? AND tr.role = ? AND rm.status = 'active'
                     ORDER BY CASE rm.actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                              rm.created_at, rm.id
                     LIMIT 1",
                )
                .bind(task_id)
                .bind(&canonical_role)
                .fetch_optional(&mut **transaction)
                .await?;
                let (assignee_type, assignee_id) = member
                    .map(|row| {
                        let actor_kind: String = row.get("actor_kind");
                        let actor_id: String = row.get("actor_id");
                        (
                            Some(if actor_kind == "agent" {
                                "agent"
                            } else {
                                "user"
                            }),
                            Some(actor_id),
                        )
                    })
                    // Preserve the existing agent-deletion compatibility
                    // shape (`agent` with a cleared id).  The new
                    // RoleMembership authority is already ended, so this
                    // placeholder cannot authorize execution or workspace
                    // access.
                    .unwrap_or((Some("agent"), None));
                if canonical_role == "implementer" {
                    sqlx::query(
                        "UPDATE task
                         SET assignee_type = ?, assignee_id = ?, updated_at = ?
                         WHERE id = ?",
                    )
                    .bind(assignee_type)
                    .bind(assignee_id.as_deref())
                    .bind(now_rfc3339())
                    .bind(task_id)
                    .execute(&mut **transaction)
                    .await?;
                }
                sqlx::query(
                    "UPDATE task_role_assignment
                     SET assignee_type = ?, assignee_id = ?, updated_at = ?
                     WHERE id = ?",
                )
                .bind(assignee_type)
                .bind(assignee_id)
                .bind(now_rfc3339())
                .bind(legacy_id)
                .execute(&mut **transaction)
                .await?;
            }

            // Rebuild the task-level compatibility projection even when the
            // role has no legacy row.  This is the common path for a
            // TaskRole created through the additive API with multiple
            // members.
            let implementer_member = sqlx::query(
                "SELECT rm.actor_kind, rm.actor_id
                 FROM task_role tr
                 JOIN role_membership rm ON rm.task_role_id = tr.id
                 WHERE tr.task_id = ? AND tr.role = 'implementer'
                   AND rm.status = 'active'
                 ORDER BY CASE rm.actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                          rm.created_at, rm.id
                 LIMIT 1",
            )
            .bind(task_id)
            .fetch_optional(&mut **transaction)
            .await?;
            let (assignee_type, assignee_id) = implementer_member
                .map(|row| {
                    let actor_kind: String = row.get("actor_kind");
                    let actor_id: String = row.get("actor_id");
                    (
                        if actor_kind == "agent" {
                            "agent"
                        } else {
                            "user"
                        },
                        Some(actor_id),
                    )
                })
                .unwrap_or(("agent", None));
            sqlx::query(
                "UPDATE task
                 SET assignee_type = ?, assignee_id = ?, updated_at = ?
                 WHERE id = ? AND assignee_type = 'agent' AND assignee_id IS NULL",
            )
            .bind(assignee_type)
            .bind(assignee_id)
            .bind(now_rfc3339())
            .bind(task_id)
            .execute(&mut **transaction)
            .await?;
        }

        Ok(events)
    }

    pub(crate) fn publish_role_sweep_events(&self, events: Vec<RoleSweepEvent>) {
        for event in events {
            self.publish_role_reassigned(
                &event.task_id,
                &event.role_name,
                Some(&event.previous_assignment),
                Some(&event.new_assignment),
                RoleReassignmentEventFlags::default(),
            );
        }
    }

    pub async fn coder_assignment(&self, task_id: &str) -> Result<Option<TaskRoleAssignment>> {
        validate_required("task_id", task_id)?;
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let Some(role_name) = self.active_work_role(&task).await? else {
            return Ok(None);
        };
        match crate::task_service::current_role_memberships_authoritative(
            &self.db, task_id, &role_name,
        )
        .await?
        {
            Some(memberships) => Ok(memberships
                .into_iter()
                .filter(|membership| membership.status == db::RoleMembershipStatus::Active)
                .min_by_key(|membership| {
                    (
                        if membership.actor_kind == db::ActorKind::Agent {
                            0
                        } else {
                            1
                        },
                        membership.created_at.clone(),
                        membership.id.clone(),
                    )
                })
                .map(|membership| TaskRoleAssignment {
                    id: membership.id,
                    task_id: task_id.to_owned(),
                    role_name,
                    assignee_type: Some(match membership.actor_kind {
                        db::ActorKind::Agent => AssigneeKind::Agent,
                        db::ActorKind::Human => AssigneeKind::User,
                    }),
                    assignee_id: Some(membership.actor_id),
                    created_at: membership.created_at,
                    updated_at: membership.updated_at,
                })),
            None => TaskRoleAssignmentRepo::get_by_task_and_role(&*self.db, task_id, &role_name)
                .await
                .map_err(Into::into),
        }
    }

    pub async fn reassign_role(
        &self,
        input: CreateTaskRoleAssignment,
        reset_workspace: bool,
        reset_worktree: bool,
    ) -> Result<TaskRoleAssignment> {
        let task = self.validate_reassignable_task(&input.task_id).await?;
        self.enforce_mode_specific_role_guards(&task, &input.role_name, Some(&input))
            .await?;
        self.preflight_legacy_singleton_role_mutation(&task, &input.role_name, Some(&input))
            .await?;
        let previous = TaskRoleAssignmentRepo::get_by_task_and_role(
            &*self.db,
            &input.task_id,
            &input.role_name,
        )
        .await?;
        let authoritative = crate::task_service::current_role_memberships_authoritative(
            &self.db,
            &input.task_id,
            &input.role_name,
        )
        .await?;
        let same_active = authoritative.as_ref().is_some_and(|memberships| {
            input
                .assignee_type
                .as_ref()
                .zip(input.assignee_id.as_ref())
                .is_some_and(|(assignee_type, assignee_id)| {
                    let actor_kind = match assignee_type {
                        AssigneeKind::Agent => db::ActorKind::Agent,
                        AssigneeKind::User => db::ActorKind::Human,
                    };
                    memberships.iter().any(|membership| {
                        membership.status == db::RoleMembershipStatus::Active
                            && membership.actor_kind == actor_kind
                            && membership.actor_id == assignee_id.as_str()
                    })
                })
        });
        if same_active
            || (authoritative.is_none() && same_assignment(previous.as_ref(), Some(&input)))
        {
            if authoritative.is_some() {
                return self.assign_role_membership(input).await;
            }
            return previous.ok_or_else(|| {
                ServiceError::not_found("task_role_assignment", input.role_name.clone())
            });
        }

        let is_coder_role =
            self.active_work_role(&task).await?.as_deref() == Some(input.role_name.as_str());

        if !is_coder_role {
            let assignment = self.assign_role_assignment(input).await?;
            self.publish_role_reassigned(
                &assignment.task_id,
                &assignment.role_name,
                previous.as_ref(),
                Some(&assignment),
                RoleReassignmentEventFlags::default(),
            );
            return Ok(assignment);
        }

        let active_execution = self
            .active_execution_for_role(&task, &input.role_name)
            .await?;
        let Some(active_execution) = active_execution else {
            let assignment = self.assign_role_assignment(input).await?;
            TaskRepo::set_review_passed_at(&*self.db, &assignment.task_id, None, &now_rfc3339())
                .await?;
            self.publish_role_reassigned(
                &assignment.task_id,
                &assignment.role_name,
                previous.as_ref(),
                Some(&assignment),
                RoleReassignmentEventFlags::default(),
            );
            return Ok(assignment);
        };

        self.cancel_active_execution(
            &active_execution,
            "cancelled by role reassignment",
            db::StopReason::RoleReassigned,
            &api_types::Actor::user(api_types::UserActionSource::RoleReassignment),
            db::ResumePolicy::None,
        )
        .await?;

        let assignment = self.assign_role_assignment(input).await?;
        TaskRepo::set_review_passed_at(&*self.db, &assignment.task_id, None, &now_rfc3339())
            .await?;

        let workflow = self.workflow_for_task(&task).await?;
        let initial_state = workflow_initial_state(&workflow)?;
        self.workflow_engine()
            .reset_to_initial(
                &task.id,
                &initial_state,
                task.version,
                &workflow,
                &api_types::Actor::user(api_types::UserActionSource::Reassignment),
                "coder reassigned",
            )
            .await?;

        let (effective_reset_workspace, effective_reset_worktree) = self
            .apply_reassignment_reset(&task, &active_execution, reset_workspace, reset_worktree)
            .await?;

        self.publish_role_reassigned(
            &assignment.task_id,
            &assignment.role_name,
            previous.as_ref(),
            Some(&assignment),
            RoleReassignmentEventFlags {
                triggered_cancellation: true,
                reset_workspace: effective_reset_workspace,
                reset_worktree: effective_reset_worktree,
                transitioned_to_todo: true,
            },
        );
        Ok(assignment)
    }

    pub async fn remove_role(
        &self,
        task_id: &str,
        role_name: &str,
        reset_workspace: bool,
        reset_worktree: bool,
    ) -> Result<()> {
        let task = self.validate_reassignable_task(task_id).await?;
        self.enforce_mode_specific_role_guards(&task, role_name, None)
            .await?;
        self.preflight_legacy_singleton_role_mutation(&task, role_name, None)
            .await?;
        let previous =
            TaskRoleAssignmentRepo::get_by_task_and_role(&*self.db, task_id, role_name).await?;
        let Some(previous) = previous else {
            // A TaskRole created through the new membership API may not have
            // acquired a legacy singleton row yet.  Do not let the old route
            // silently leave that authoritative membership alive; the bounded
            // compatibility writer will end the current set (and reject a
            // plural set) in the same way as it does when a projection exists.
            if crate::task_service::current_role_memberships_authoritative(
                &self.db, task_id, role_name,
            )
            .await?
            .is_some()
            {
                TaskRoleAssignmentRepo::remove(&*self.db, task_id, role_name).await?;
            }
            return Ok(());
        };

        let is_coder_role = self.active_work_role(&task).await?.as_deref() == Some(role_name);
        if !is_coder_role {
            TaskRoleAssignmentRepo::remove(&*self.db, task_id, role_name).await?;
            self.publish_role_reassigned(
                task_id,
                role_name,
                Some(&previous),
                None,
                RoleReassignmentEventFlags::default(),
            );
            return Ok(());
        }

        let active_execution = self.active_execution_for_role(&task, role_name).await?;
        let Some(active_execution) = active_execution else {
            TaskRoleAssignmentRepo::remove(&*self.db, task_id, role_name).await?;
            TaskRepo::set_review_passed_at(&*self.db, task_id, None, &now_rfc3339()).await?;
            self.publish_role_reassigned(
                task_id,
                role_name,
                Some(&previous),
                None,
                RoleReassignmentEventFlags::default(),
            );
            return Ok(());
        };

        self.cancel_active_execution(
            &active_execution,
            "cancelled by role reassignment",
            db::StopReason::RoleReassigned,
            &api_types::Actor::user(api_types::UserActionSource::RoleReassignment),
            db::ResumePolicy::None,
        )
        .await?;
        TaskRoleAssignmentRepo::remove(&*self.db, task_id, role_name).await?;
        TaskRepo::set_review_passed_at(&*self.db, task_id, None, &now_rfc3339()).await?;

        let workflow = self.workflow_for_task(&task).await?;
        let initial_state = workflow_initial_state(&workflow)?;
        self.workflow_engine()
            .reset_to_initial(
                &task.id,
                &initial_state,
                task.version,
                &workflow,
                &api_types::Actor::user(api_types::UserActionSource::Reassignment),
                "coder reassigned",
            )
            .await?;

        let (effective_reset_workspace, effective_reset_worktree) = self
            .apply_reassignment_reset(&task, &active_execution, reset_workspace, reset_worktree)
            .await?;

        self.publish_role_reassigned(
            task_id,
            role_name,
            Some(&previous),
            None,
            RoleReassignmentEventFlags {
                triggered_cancellation: true,
                reset_workspace: effective_reset_workspace,
                reset_worktree: effective_reset_worktree,
                transitioned_to_todo: true,
            },
        );
        Ok(())
    }

    async fn validate_reassignable_task(&self, task_id: &str) -> Result<Task> {
        validate_required("task_id", task_id)?;
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let workflow = WorkflowEngine::resolve_workflow(&project.workflow_definition);
        if workflow
            .states
            .iter()
            .find(|state| state.name == task.status)
            .is_some_and(|state| state.kind == api_types::StateKind::Terminal)
        {
            return Err(ServiceError::invalid_operation(format!(
                "task {} is in terminal state {}; cannot reassign role",
                task.id, task.status
            )));
        }
        Ok(task)
    }

    async fn preflight_legacy_singleton_role_mutation(
        &self,
        task: &Task,
        role_name: &str,
        incoming: Option<&CreateTaskRoleAssignment>,
    ) -> Result<()> {
        let authoritative = crate::task_service::current_role_memberships_authoritative(
            &self.db, &task.id, role_name,
        )
        .await?;
        if let Some(memberships) = authoritative.as_ref() {
            let incoming_is_same_active = incoming.is_some_and(|input| {
                legacy_assignment_actor_ref(input)
                    .ok()
                    .flatten()
                    .is_some_and(|actor_ref| {
                        let (actor_kind, actor_id) = match actor_ref {
                            ActorRef::Agent(id) => (db::ActorKind::Agent, id),
                            ActorRef::Human(id) => (db::ActorKind::Human, id),
                        };
                        memberships.iter().any(|membership| {
                            membership.status == db::RoleMembershipStatus::Active
                                && membership.actor_kind == actor_kind
                                && membership.actor_id == actor_id
                        })
                    })
            });
            if memberships.len() > 1 && !incoming_is_same_active {
                return Err(ServiceError::conflict(
                    "legacy singleton role mutation cannot replace multiple current memberships",
                ));
            }
        }

        if let Some(input) = incoming {
            if db::canonical_task_role_name(role_name).is_some() {
                if input.assignee_type.as_ref() == Some(&AssigneeKind::User)
                    && input.assignee_id.as_deref() == Some("human")
                {
                    if authoritative.is_some() {
                        return Err(ServiceError::invalid_operation(
                            "the human sentinel cannot be assigned to a replacement TaskRole",
                        ));
                    }
                    return Ok(());
                }
                if let Some(actor_ref) = legacy_assignment_actor_ref(input)? {
                    self.validate_actor_for_task(task, &actor_ref).await?;
                }
            }
        }
        Ok(())
    }

    async fn workflow_for_task(&self, task: &Task) -> Result<api_types::WorkflowDefinition> {
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        Ok(WorkflowEngine::resolve_workflow(
            &project.workflow_definition,
        ))
    }

    async fn enforce_mode_specific_role_guards(
        &self,
        task: &Task,
        role_name: &str,
        _incoming: Option<&CreateTaskRoleAssignment>,
    ) -> Result<RoleGuardAction> {
        if let Some(memberships) = crate::task_service::current_role_memberships_authoritative(
            &self.db, &task.id, role_name,
        )
        .await?
        {
            let incoming_is_same_active = _incoming.is_some_and(|incoming| {
                incoming
                    .assignee_type
                    .as_ref()
                    .zip(incoming.assignee_id.as_ref())
                    .is_some_and(|(assignee_type, assignee_id)| {
                        let actor_kind = match assignee_type {
                            AssigneeKind::Agent => db::ActorKind::Agent,
                            AssigneeKind::User => db::ActorKind::Human,
                        };
                        memberships.iter().any(|membership| {
                            membership.status == db::RoleMembershipStatus::Active
                                && membership.actor_kind == actor_kind
                                && membership.actor_id == assignee_id.as_str()
                        })
                    })
            });
            if memberships.len() > 1 && !incoming_is_same_active {
                return Err(ServiceError::conflict(
                    "legacy singleton role reassignment cannot replace multiple current memberships",
                ));
            }
        }
        if role_name != "coder" {
            return Ok(RoleGuardAction::Continue);
        }

        if task.parent_task_id.is_some() {
            return Err(ServiceError::invalid_operation(
                "subtask coder assignment is managed by the root task",
            ));
        }

        if TaskRepo::list_subtasks_ordered(&*self.db, &task.id)
            .await?
            .iter()
            .any(|s| {
                s.status != default_states::TODO
                    && s.status != default_states::DONE
                    && s.status != default_states::CANCELLED
            })
        {
            return Err(ServiceError::task_sequence_already_started(task.id.clone()));
        }

        Ok(RoleGuardAction::Continue)
    }

    async fn assign_role_assignment(
        &self,
        input: CreateTaskRoleAssignment,
    ) -> Result<TaskRoleAssignment> {
        if db::canonical_task_role_name(&input.role_name).is_some() {
            self.assign_role_membership(input).await
        } else {
            TaskRoleAssignmentRepo::assign(&*self.db, input)
                .await
                .map_err(Into::into)
        }
    }

    async fn active_work_role(&self, task: &Task) -> Result<Option<String>> {
        let workflow = self.workflow_for_task(task).await?;
        Ok(workflow
            .states
            .iter()
            .find(|state| state.name == default_states::IN_PROGRESS)
            .and_then(crate::workflow::effective_role)
            .map(str::to_owned)
            .or_else(|| {
                workflow
                    .states
                    .iter()
                    .find(|state| state.kind == api_types::StateKind::Active)
                    .and_then(crate::workflow::effective_role)
                    .map(str::to_owned)
            }))
    }

    async fn active_execution_for_role(
        &self,
        task: &Task,
        role_name: &str,
    ) -> Result<Option<Execution>> {
        if self.role_for_state(task, &task.status).await?.as_deref() != Some(role_name) {
            return Ok(None);
        }
        let page = ExecutionRepo::list_by_task(
            &*self.db,
            &task.id,
            PageRequest {
                cursor: None,
                limit: 20,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await?;
        Ok(page.items.into_iter().find(|execution| {
            execution.status == ExecutionStatus::Running
                && (execution.role == role_name || execution.role == "executor")
        }))
    }

    pub(super) async fn cancel_active_execution(
        &self,
        execution: &Execution,
        reason: &str,
        stop_reason: db::StopReason,
        actor: &api_types::Actor,
        resume_policy: db::ResumePolicy,
    ) -> Result<()> {
        let reconciliation_reason = match &stop_reason {
            db::StopReason::RoleReassigned => "stopped because task moved".to_owned(),
            _ => reason.to_owned(),
        };
        let preserve_resume_context = resume_policy == db::ResumePolicy::Manual;
        if self.daemon_connections.is_some() || self.task_executor.is_some() {
            if let Err(error) = self.cancel_execution_with_provider(execution, reason).await {
                if matches!(error, ServiceError::DaemonUnavailable { .. }) {
                    return Err(error);
                }
                tracing::warn!(
                    execution_id = %execution.id,
                    %error,
                    "executor cancellation failed; marking execution cancelled"
                );
            }
        }
        ExecutionRepo::update(
            &*self.db,
            db::UpdateExecution {
                id: execution.id.clone(),
                status: Some(ExecutionStatus::Cancelled),
                stop_reason: Some(Some(stop_reason)),
                stopped_by: Some(Some(actor.display())),
                resume_policy: Some(Some(resume_policy)),
                stopped_at: Some(Some(now_rfc3339())),
                // Manual user stops retain the session and executor snapshot so the
                // task façade can resume the same worker thread. Lifecycle and task
                // cancellation paths clear only the legacy projection; the DB
                // repository ignores that clear when HarnessSession is authoritative.
                agent_session_id: (!preserve_resume_context).then_some(None),
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: Some(Some(reason.to_owned())),
                executor_config_snapshot_json: (!preserve_resume_context).then_some(None),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .map_err(|error| ServiceError::invalid_operation(format!("cancel failed: {error}")))?;
        self.revoke_active_workspace_lease_for_execution(&execution.task_id, &execution.id)
            .await;
        self.publish(ForgeEvent {
            event_type: "task.execution_cancelled".to_owned(),
            entity_id: execution.task_id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskExecutionCancelled {
                task_id: execution.task_id.clone(),
                execution_id: execution.id.clone(),
                reason: reason.to_owned(),
            },
        });
        self.publish(ForgeEvent {
            event_type: "reconciliation.event".to_owned(),
            entity_id: execution.task_id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ReconciliationEvent {
                task_id: Some(execution.task_id.clone()),
                execution_id: Some(execution.id.clone()),
                reason: reconciliation_reason,
            },
        });
        Ok(())
    }

    async fn apply_reassignment_reset(
        &self,
        task: &Task,
        execution: &Execution,
        reset_workspace: bool,
        reset_worktree: bool,
    ) -> Result<(bool, bool)> {
        if reset_workspace {
            let mut workspace_id = execution.workspace_id.clone();
            if workspace_id.is_none() {
                workspace_id = WorkspaceRepo::get_by_task_id(&*self.db, &task.id)
                    .await?
                    .map(|workspace| workspace.id);
            }
            if let (Some(cleanup_scheduler), Some(workspace_id)) =
                (self.cleanup_scheduler.as_ref(), workspace_id)
            {
                cleanup_scheduler.cleanup_now(workspace_id).await?;
                return Ok((true, false));
            }
            return Ok((false, false));
        }

        if reset_worktree {
            let repo_id = task
                .repo_id
                .as_deref()
                .ok_or_else(|| ServiceError::invalid_operation("task has no associated repo"))?;
            let repo = RepoRepo::get_by_id(&*self.db, repo_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
            let repo_url = repo
                .local_path
                .filter(|path| !path.trim().is_empty())
                .unwrap_or(repo.remote_url);
            let repo_name = reassignment_repo_name(&repo_url);
            let workspace_root = self
                .cleanup_scheduler
                .as_ref()
                .map(|scheduler| scheduler.workspace_root().to_path_buf())
                .unwrap_or_else(|| self.workspace_root.clone());
            let manager = WorkspaceManager::new(workspace_root);
            manager
                .reset_worktree(&task.id, &repo_name)
                .await
                .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            return Ok((false, true));
        }

        Ok((false, false))
    }

    fn workflow_engine(&self) -> WorkflowEngine {
        WorkflowEngine {
            db: Arc::clone(&self.db),
            event_bus: Arc::clone(&self.event_bus),
            review_runner: self.review_runner.clone(),
            merge_service: self.merge_service.clone(),
            cleanup_scheduler: self.cleanup_scheduler.clone(),
            task_executor: self.task_executor.clone(),
            daemon_connections: self.daemon_connections.clone(),
            workspace_exec_locks: self.workspace_exec_locks.clone(),
            terminal_activity: self.terminal_activity.clone(),
            workspace_root: self.workspace_root.clone(),
            repo_cache_locks: self.repo_cache_locks.clone(),
        }
    }

    fn publish_role_reassigned(
        &self,
        task_id: &str,
        role_name: &str,
        previous_assignment: Option<&TaskRoleAssignment>,
        new_assignment: Option<&TaskRoleAssignment>,
        flags: RoleReassignmentEventFlags,
    ) {
        self.publish(ForgeEvent {
            event_type: "task.role_reassigned".to_owned(),
            entity_id: task_id.to_owned(),
            timestamp: event_timestamp(),
            context: EventContext::TaskRoleReassigned {
                task_id: task_id.to_owned(),
                role_name: role_name.to_owned(),
                previous_assignment: previous_assignment.map(snapshot),
                new_assignment: new_assignment.map(snapshot),
                triggered_cancellation: flags.triggered_cancellation,
                reset_workspace: flags.reset_workspace,
                reset_worktree: flags.reset_worktree,
                transitioned_to_todo: flags.transitioned_to_todo,
            },
        });
    }
}

fn legacy_assignment_actor_ref(input: &CreateTaskRoleAssignment) -> Result<Option<ActorRef>> {
    match (input.assignee_type.as_ref(), input.assignee_id.as_deref()) {
        (Some(AssigneeKind::Agent), Some(id)) => Ok(Some(ActorRef::Agent(id.to_owned()))),
        (Some(AssigneeKind::User), Some("human")) => Ok(None),
        (Some(AssigneeKind::User), Some(id)) => Ok(Some(ActorRef::Human(id.to_owned()))),
        (None, None) => Ok(None),
        _ => Err(ServiceError::invalid_operation(
            "role assignment actor type and id must be provided together",
        )),
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RoleReassignmentEventFlags {
    triggered_cancellation: bool,
    reset_workspace: bool,
    reset_worktree: bool,
    transitioned_to_todo: bool,
}

enum RoleGuardAction {
    Continue,
}

pub(crate) struct RoleSweepEvent {
    pub(crate) task_id: String,
    pub(crate) role_name: String,
    pub(crate) previous_assignment: TaskRoleAssignment,
    pub(crate) new_assignment: TaskRoleAssignment,
}

fn same_assignment(
    previous: Option<&TaskRoleAssignment>,
    next: Option<&CreateTaskRoleAssignment>,
) -> bool {
    match (previous, next) {
        (Some(previous), Some(next)) => {
            previous.assignee_type == next.assignee_type && previous.assignee_id == next.assignee_id
        }
        (None, None) => true,
        _ => false,
    }
}

fn snapshot(assignment: &TaskRoleAssignment) -> RoleAssignmentSnapshot {
    RoleAssignmentSnapshot {
        assignee_type: assignment.assignee_type.as_ref().map(ToString::to_string),
        assignee_id: assignment.assignee_id.clone(),
    }
}

fn workflow_initial_state(workflow: &api_types::WorkflowDefinition) -> Result<String> {
    workflow
        .states
        .iter()
        .find(|state| state.kind == api_types::StateKind::Initial)
        .map(|state| state.name.clone())
        .ok_or_else(|| ServiceError::invalid_operation("workflow has no initial state"))
}

fn reassignment_repo_name(repo_url: &str) -> String {
    let trimmed = repo_url.trim_end_matches(['/', '\\']);
    let last_component = trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|component| !component.is_empty())
        .unwrap_or("repo");

    last_component
        .strip_suffix(".git")
        .unwrap_or(last_component)
        .to_owned()
}
