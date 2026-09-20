use super::*;
use api_types::ActorRef;
use db::{
    canonical_task_role_name, new_uuid_v4, now_rfc3339, ActorKind, CoordinationMode,
    CreateRoleMembership, CreateTaskRole, ProjectMemberRepo, ProjectRepo, RoleMembership,
    RoleMembershipRepo,
    RoleMembershipStatus, TaskRole, TaskRoleRepo, UpdateRoleMembership, UpdateTaskRole, UserRepo,
};

impl TaskService {
    /// Load every TaskRole, including roles that currently have no members.
    /// `include_ended` controls whether ended historical membership records are
    /// returned; current eligibility is always limited to active/suspended
    /// records by callers that pass `false`.
    pub async fn list_task_roles(
        &self,
        task_id: &str,
        include_ended: bool,
    ) -> Result<Vec<(TaskRole, Vec<RoleMembership>)>> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let roles = TaskRoleRepo::list_by_task(&*self.db, &task.id).await?;
        let mut result = Vec::with_capacity(roles.len());
        for role in roles {
            let members = RoleMembershipRepo::list_by_role(&*self.db, &role.id, include_ended)
                .await?;
            result.push((role, members));
        }
        Ok(result)
    }

    pub async fn create_task_role(
        &self,
        task_id: &str,
        role: &str,
        coordination_mode: CoordinationMode,
        policy_json: String,
    ) -> Result<TaskRole> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let role = canonical_task_role_name(role)
            .ok_or_else(|| ServiceError::invalid_operation("role is not a TaskRole"))?;
        validate_policy_json(&policy_json)?;
        if TaskRoleRepo::get_by_task_and_role(&*self.db, &task.id, &role)
            .await?
            .is_some()
        {
            return Err(ServiceError::conflict(format!(
                "TaskRole '{role}' already exists for task {}",
                task.id
            )));
        }
        let now = now_rfc3339();
        TaskRoleRepo::create(
            &*self.db,
            CreateTaskRole {
                id: new_uuid_v4(),
                task_id: task.id,
                role,
                coordination_mode: Some(coordination_mode),
                policy_json,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .map_err(Into::into)
    }

    pub async fn update_task_role(
        &self,
        task_id: &str,
        role: &str,
        expected_version: i64,
        coordination_mode: Option<CoordinationMode>,
        policy_json: Option<String>,
    ) -> Result<TaskRole> {
        let role = self.task_role_for_task(task_id, role).await?;
        if let Some(policy_json) = policy_json.as_deref() {
            validate_policy_json(policy_json)?;
        }
        TaskRoleRepo::update(
            &*self.db,
            UpdateTaskRole {
                id: role.id,
                expected_version,
                coordination_mode: coordination_mode.map(Some),
                policy_json,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .map_err(Into::into)
    }

    pub async fn add_task_role_member(
        &self,
        task_id: &str,
        role: &str,
        actor_ref: ActorRef,
    ) -> Result<RoleMembership> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let task_role = self.task_role_for_task(&task.id, role).await?;
        let existing = RoleMembershipRepo::list_by_role(&*self.db, &task_role.id, false).await?;
        if existing.iter().any(|member| {
            member.actor_kind == actor_kind(&actor_ref) && member.actor_id == actor_id(&actor_ref)
        }) {
            return Err(ServiceError::conflict(
                "actor is already an active or suspended member of this role",
            ));
        }
        if task_role.coordination_mode.is_none() && !existing.is_empty() {
            return Err(ServiceError::invalid_operation(
                "set coordination_mode before adding a second current member",
            ));
        }
        self.validate_actor_for_task(&task, &actor_ref).await?;
        let now = now_rfc3339();
        let task_role_id = task_role.id.clone();
        let membership = RoleMembershipRepo::add(
            &*self.db,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id,
                actor_kind: actor_kind(&actor_ref),
                actor_id: actor_id(&actor_ref).to_owned(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .map_err(|error| match error {
            db::DbError::Sqlx(sqlx::Error::Database(database_error))
                if database_error.message().contains("UNIQUE") => {
                    ServiceError::conflict("actor already has a current membership in this role")
                }
            db::DbError::Sqlx(sqlx::Error::Database(database_error))
                if database_error
                    .message()
                    .contains("coordination_mode is required") => ServiceError::invalid_operation(
                "set coordination_mode before adding a second current member",
            ),
            other => other.into(),
        })?;
        self.sync_legacy_role_projection(&task, &task_role).await?;
        self.publish(ForgeEvent {
            event_type: "task.updated".to_owned(),
            entity_id: task.id,
            timestamp: event_timestamp(),
            context: EventContext::TaskUpdated {
                project_id: task.project_id,
            },
        });
        Ok(membership)
    }

    pub async fn update_task_role_member(
        &self,
        task_id: &str,
        membership_id: &str,
        expected_version: i64,
        status: RoleMembershipStatus,
    ) -> Result<RoleMembership> {
        let membership = RoleMembershipRepo::get(&*self.db, membership_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("role_membership", membership_id.to_owned()))?;
        let role = TaskRoleRepo::get_by_id(&*self.db, &membership.task_role_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task_role", membership.task_role_id.clone()))?;
        if role.task_id != task_id {
            return Err(ServiceError::not_found(
                "role_membership",
                membership_id.to_owned(),
            ));
        }
        if membership.status == RoleMembershipStatus::Ended {
            return Err(ServiceError::invalid_operation(
                "ended role memberships are historical and cannot be changed",
            ));
        }
        let updated = RoleMembershipRepo::update(
            &*self.db,
            UpdateRoleMembership {
                id: membership.id,
                expected_version,
                status,
                updated_at: now_rfc3339(),
                ended_at: None,
            },
        )
        .await
        .map_err(|error| match error {
            db::DbError::Sqlx(sqlx::Error::Database(database_error))
                if database_error
                    .message()
                    .contains("coordination_mode is required") => ServiceError::invalid_operation(
                "set coordination_mode before restoring a second current member",
            ),
            other => other.into(),
        })?;
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        self.sync_legacy_role_projection(&task, &role).await?;
        self.publish(ForgeEvent {
            event_type: "task.updated".to_owned(),
            entity_id: task.id,
            timestamp: event_timestamp(),
            context: EventContext::TaskUpdated {
                project_id: task.project_id,
            },
        });
        Ok(updated)
    }

    pub async fn task_role_for_task(&self, task_id: &str, role: &str) -> Result<TaskRole> {
        let role = canonical_task_role_name(role)
            .ok_or_else(|| ServiceError::invalid_operation("role is not a TaskRole"))?;
        TaskRoleRepo::get_by_task_and_role(&*self.db, task_id, &role)
            .await?
            .ok_or_else(|| ServiceError::not_found("task_role", format!("{task_id}:{role}")))
    }

    async fn validate_actor_for_task(&self, task: &db::Task, actor_ref: &ActorRef) -> Result<()> {
        match actor_ref {
            ActorRef::Agent(agent_id) => {
                AgentRepo::get_by_id(&*self.db, agent_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
            }
            ActorRef::Human(user_id) => {
                UserRepo::get_user_by_id(&*self.db, user_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("user", user_id.clone()))?;
                let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
                if project.owner_id.as_deref() != Some(user_id)
                    && ProjectMemberRepo::get_member(&*self.db, &task.project_id, user_id)
                        .await?
                        .is_none()
                {
                    return Err(ServiceError::invalid_operation(
                        "human actor must be a project member",
                    ));
                }
            }
        }
        Ok(())
    }

    async fn sync_legacy_role_projection(
        &self,
        task: &db::Task,
        task_role: &TaskRole,
    ) -> Result<()> {
        let members = RoleMembershipRepo::list_by_role(&*self.db, &task_role.id, false)
            .await?
            .into_iter()
            .filter(|member| member.status == RoleMembershipStatus::Active)
            .collect::<Vec<_>>();
        let legacy_assignments = TaskRoleAssignmentRepo::list_by_task(&*self.db, &task.id).await?;
        let mut projection_roles = legacy_assignments
            .iter()
            .filter(|assignment| {
                db::canonical_task_role_name(&assignment.role_name).as_deref()
                    == Some(task_role.role.as_str())
            })
            .map(|assignment| assignment.role_name.clone())
            .collect::<Vec<_>>();
        if projection_roles.is_empty() {
            if let Some(role) = self.workflow_role_for_canonical(task, &task_role.role).await? {
                projection_roles.push(role);
            }
        }
        // The task-level assignee is also a compatibility projection for the
        // canonical implementation role.  Keep it synchronized even when a
        // workflow does not expose a legacy role label for this TaskRole.
        if projection_roles.is_empty() && task_role.role != "implementer" {
            return Ok(());
        }

        let Some(member) = members.iter().min_by_key(|member| {
            (
                if member.actor_kind == ActorKind::Agent {
                    0_u8
                } else {
                    1_u8
                },
                member.created_at.clone(),
                member.id.clone(),
            )
        }) else {
            if task_role.role == "implementer" {
                sqlx::query(
                    "UPDATE task
                     SET assignee_type = NULL, assignee_id = NULL, updated_at = ?
                     WHERE id = ?",
                )
                .bind(now_rfc3339())
                .bind(&task.id)
                .execute(self.db.pool())
                .await?;
            }
            for projection_role in projection_roles {
                sqlx::query(
                    "DELETE FROM task_role_assignment WHERE task_id = ? AND role_name = ?",
                )
                .bind(&task.id)
                .bind(projection_role)
                .execute(self.db.pool())
                .await?;
            }
            return Ok(());
        };
        let assignee_type = match member.actor_kind {
            ActorKind::Human => "user",
            ActorKind::Agent => "agent",
        };
        let now = now_rfc3339();
        if task_role.role == "implementer" {
            sqlx::query(
                "UPDATE task
                 SET assignee_type = ?, assignee_id = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(assignee_type)
            .bind(&member.actor_id)
            .bind(&now)
            .bind(&task.id)
            .execute(self.db.pool())
            .await?;
        }
        if projection_roles.is_empty() {
            return Ok(());
        }
        for projection_role in projection_roles {
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
            .bind(&task.id)
            .bind(projection_role)
            .bind(assignee_type)
            .bind(&member.actor_id)
            .bind(&member.created_at)
            .bind(&now)
            .execute(self.db.pool())
            .await?;
        }
        Ok(())
    }

    async fn workflow_role_for_canonical(
        &self,
        task: &db::Task,
        canonical: &str,
    ) -> Result<Option<String>> {
        // The task's workflow is the only compatibility context that can tell
        // whether the old public label was `coder`, `worker`, or another
        // workflow role.  This projection is never used for eligibility.
        let Some(project) = ProjectRepo::get_by_id(&*self.db, &task.project_id).await? else {
            return Ok(None);
        };
        let workflow = crate::workflow::engine::WorkflowEngine::resolve_workflow(&project.workflow_definition);
        Ok(workflow
            .roles
            .into_iter()
            .find(|role| db::canonical_task_role_name(&role.name).as_deref() == Some(canonical))
            .map(|role| role.name))
    }
}

/// `None` means this task has not yet acquired a replacement TaskRole record;
/// callers may use the bounded legacy projection for that pre-migration
/// fixture/data case. `Some(empty)` is authoritative unassigned membership.
pub async fn current_role_memberships_authoritative(
    db: &db::SqliteDb,
    task_id: &str,
    role: &str,
) -> Result<Option<Vec<RoleMembership>>> {
    let Some(role) = canonical_task_role_name(role) else {
        return Ok(Some(Vec::new()));
    };
    let Some(task_role) = TaskRoleRepo::get_by_task_and_role(db, task_id, &role).await? else {
        return Ok(None);
    };
    RoleMembershipRepo::list_by_role(db, &task_role.id, false)
        .await
        .map(Some)
        .map_err(Into::into)
}

fn actor_kind(actor_ref: &ActorRef) -> ActorKind {
    match actor_ref {
        ActorRef::Human(_) => ActorKind::Human,
        ActorRef::Agent(_) => ActorKind::Agent,
    }
}

fn actor_id(actor_ref: &ActorRef) -> &str {
    match actor_ref {
        ActorRef::Human(id) | ActorRef::Agent(id) => id,
    }
}

fn validate_policy_json(policy_json: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(policy_json)
        .map_err(|error| ServiceError::invalid_operation(format!("invalid role policy: {error}")))?;
    if !value.is_object() {
        return Err(ServiceError::invalid_operation(
            "role policy must be a JSON object",
        ));
    }
    Ok(())
}
