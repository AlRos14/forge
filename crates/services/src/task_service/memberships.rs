use super::*;
use api_types::ActorRef;
use db::{
    canonical_task_role_name, new_uuid_v4, now_rfc3339, ActorKind, AssigneeKind, CoordinationMode,
    CreateTaskRole, CreateTaskRoleAssignment, ProjectRepo, RoleMembership, RoleMembershipRepo,
    RoleMembershipStatus, TaskRole, TaskRoleAssignment, TaskRoleRepo, UpdateTaskRole, UserRepo,
};
use sqlx::{Row, Sqlite, Transaction};

use crate::agent_service::{compute_effective_status, EffectiveStatus};
use crate::project_actor_scope;

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
            let members =
                RoleMembershipRepo::list_by_role(&*self.db, &role.id, include_ended).await?;
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
        let fallback_role = self
            .workflow_role_for_canonical(&task, &task_role.role)
            .await?;
        let now = now_rfc3339();
        let membership_id = new_uuid_v4();
        let mut transaction = self.db.pool().begin().await?;
        sqlx::query(
            "INSERT INTO role_membership
                (id, task_role_id, actor_kind, actor_id, status, version, created_at, updated_at, ended_at)
             VALUES (?, ?, ?, ?, 'active', 1, ?, ?, NULL)",
        )
        .bind(&membership_id)
        .bind(&task_role.id)
        .bind(actor_kind(&actor_ref).to_string())
        .bind(actor_id(&actor_ref))
        .bind(&now)
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(map_membership_write_error)?;
        self.sync_legacy_role_projection_in_tx(
            &mut transaction,
            &task,
            &task_role.id,
            &task_role.role,
            fallback_role.as_deref(),
            None,
        )
        .await?;
        transaction.commit().await?;
        let membership = RoleMembershipRepo::get(&*self.db, &membership_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("role_membership", membership_id.clone()))?;
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

    /// Translate a legacy singleton assignment into the authoritative
    /// TaskRole/RoleMembership representation. The compatibility rows are
    /// projections maintained by the same transaction.
    pub(crate) async fn assign_role_membership(
        &self,
        input: CreateTaskRoleAssignment,
    ) -> Result<TaskRoleAssignment> {
        let Some(canonical_role) = canonical_task_role_name(&input.role_name) else {
            // interactive/merge_fixer/system remain execution labels, not
            // TaskRole vocabulary, until their later execution migration.
            return TaskRoleAssignmentRepo::assign(&*self.db, input)
                .await
                .map_err(Into::into);
        };
        if input.assignee_type.as_ref() == Some(&AssigneeKind::User)
            && input.assignee_id.as_deref() == Some("human")
        {
            // The pre-existing project-settings sentinel means "manual Human"
            // rather than a real user identity. Keep it in the bounded legacy
            // path; it must never become ActorRef::Human("human"). A
            // replacement TaskRole cannot accept the sentinel because it would
            // otherwise bypass authoritative Actor validation.
            let task = TaskRepo::get_by_id(&*self.db, &input.task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", input.task_id.clone()))?;
            if current_role_memberships_authoritative(&self.db, &task.id, &canonical_role)
                .await?
                .is_some()
            {
                return Err(ServiceError::invalid_operation(
                    "the human sentinel cannot be assigned to a replacement TaskRole",
                ));
            }
            return TaskRoleAssignmentRepo::assign(&*self.db, input)
                .await
                .map_err(Into::into);
        }
        let task = TaskRepo::get_by_id(&*self.db, &input.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", input.task_id.clone()))?;
        let actor_ref = match (input.assignee_type.clone(), input.assignee_id.as_deref()) {
            (Some(AssigneeKind::Agent), Some(id)) => Some(ActorRef::Agent(id.to_owned())),
            (Some(AssigneeKind::User), Some(id)) => Some(ActorRef::Human(id.to_owned())),
            (None, None) => None,
            _ => {
                return Err(ServiceError::invalid_operation(
                    "role assignment actor type and id must be provided together",
                ));
            }
        };
        if let Some(actor_ref) = actor_ref.as_ref() {
            self.validate_actor_for_task(&task, actor_ref).await?;
        }
        let fallback_role = self
            .workflow_role_for_canonical(&task, &canonical_role)
            .await?;

        let mut transaction = self.db.pool().begin().await?;
        let task_role_id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM task_role WHERE task_id = ? AND role = ?",
        )
        .bind(&task.id)
        .bind(&canonical_role)
        .fetch_optional(&mut *transaction)
        .await?;
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
            .bind(&task.id)
            .bind(&canonical_role)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&mut *transaction)
            .await
            .map_err(map_membership_write_error)?;
            task_role_id
        };

        let current_members = sqlx::query(
            "SELECT actor_kind, actor_id, status
             FROM role_membership
             WHERE task_role_id = ? AND status IN ('active', 'suspended')
             ORDER BY created_at, id",
        )
        .bind(&task_role_id)
        .fetch_all(&mut *transaction)
        .await?;
        let same_active = actor_ref.as_ref().is_some_and(|actor| {
            current_members.iter().any(|row| {
                row.get::<String, _>("actor_kind") == actor_kind(actor).to_string()
                    && row.get::<String, _>("actor_id") == actor_id(actor)
                    && row.get::<String, _>("status") == "active"
            })
        });
        if same_active {
            self.sync_legacy_role_projection_in_tx(
                &mut transaction,
                &task,
                &task_role_id,
                &canonical_role,
                fallback_role.as_deref(),
                Some(&input.role_name),
            )
            .await?;
            transaction.commit().await?;
            return self
                .legacy_assignment_projection(&task.id, &input.role_name)
                .await;
        }
        if current_members.len() > 1 {
            return Err(ServiceError::conflict(
                "legacy singleton role mutation cannot replace multiple current memberships",
            ));
        }

        sqlx::query(
            "UPDATE role_membership
             SET status = 'ended', ended_at = ?, updated_at = ?, version = version + 1
             WHERE task_role_id = ? AND status IN ('active', 'suspended')",
        )
        .bind(&input.updated_at)
        .bind(&input.updated_at)
        .bind(&task_role_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_membership_write_error)?;
        if let Some(actor_ref) = actor_ref.as_ref() {
            sqlx::query(
                "INSERT INTO role_membership
                    (id, task_role_id, actor_kind, actor_id, status, version, created_at, updated_at, ended_at)
                 VALUES (?, ?, ?, ?, 'active', 1, ?, ?, NULL)",
            )
            .bind(new_uuid_v4())
            .bind(&task_role_id)
            .bind(actor_kind(actor_ref).to_string())
            .bind(actor_id(actor_ref))
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&mut *transaction)
            .await
            .map_err(map_membership_write_error)?;
        }
        self.sync_legacy_role_projection_in_tx(
            &mut transaction,
            &task,
            &task_role_id,
            &canonical_role,
            fallback_role.as_deref(),
            Some(&input.role_name),
        )
        .await?;
        transaction.commit().await?;
        self.legacy_assignment_projection(&task.id, &input.role_name)
            .await
    }

    async fn legacy_assignment_projection(
        &self,
        task_id: &str,
        role_name: &str,
    ) -> Result<TaskRoleAssignment> {
        TaskRoleAssignmentRepo::get_by_task_and_role(&*self.db, task_id, role_name)
            .await?
            .ok_or_else(|| ServiceError::not_found("task_role_assignment", role_name.to_owned()))
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
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let fallback_role = self.workflow_role_for_canonical(&task, &role.role).await?;
        let updated_at = now_rfc3339();
        let ended_at = (status == RoleMembershipStatus::Ended).then(|| updated_at.clone());
        let mut transaction = self.db.pool().begin().await?;
        let result = sqlx::query(
            "UPDATE role_membership
             SET status = ?, version = version + 1, updated_at = ?, ended_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(status.to_string())
        .bind(&updated_at)
        .bind(ended_at.as_deref())
        .bind(&membership.id)
        .bind(expected_version)
        .execute(&mut *transaction)
        .await
        .map_err(map_membership_write_error)?;
        if result.rows_affected() == 0 {
            let exists: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM role_membership WHERE id = ?")
                    .bind(&membership.id)
                    .fetch_one(&mut *transaction)
                    .await?;
            return Err(if exists == 0 {
                ServiceError::Db(db::DbError::NotFound)
            } else {
                ServiceError::Db(db::DbError::VersionConflict)
            });
        }
        self.sync_legacy_role_projection_in_tx(
            &mut transaction,
            &task,
            &role.id,
            &role.role,
            fallback_role.as_deref(),
            None,
        )
        .await?;
        transaction.commit().await?;
        let updated = RoleMembershipRepo::get(&*self.db, &membership.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("role_membership", membership.id.clone()))?;
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

    pub(crate) async fn validate_actor_for_task(
        &self,
        task: &db::Task,
        actor_ref: &ActorRef,
    ) -> Result<()> {
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        self.validate_actor_for_project(&project, actor_ref).await
    }

    pub(crate) async fn validate_actor_for_project(
        &self,
        project: &db::Project,
        actor_ref: &ActorRef,
    ) -> Result<()> {
        match actor_ref {
            ActorRef::Agent(agent_id) => {
                if agent_id == "human" {
                    return Err(ServiceError::invalid_operation(
                        "the human sentinel is not an Actor identity",
                    ));
                }
                let _agent = AgentRepo::get_by_id(&*self.db, agent_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
                if !project_actor_scope::actor_is_valid_for_project(&self.db, project, actor_ref)
                    .await?
                {
                    return Err(ServiceError::invalid_operation(
                        "agent is not valid for the task project",
                    ));
                }
            }
            ActorRef::Human(user_id) => {
                if user_id == "human" {
                    return Err(ServiceError::invalid_operation(
                        "the human sentinel is not an Actor identity",
                    ));
                }
                UserRepo::get_user_by_id(&*self.db, user_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("user", user_id.clone()))?;
                if !project_actor_scope::actor_is_valid_for_project(&self.db, project, actor_ref)
                    .await?
                {
                    return Err(ServiceError::invalid_operation(
                        "human actor must be a project member",
                    ));
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn ensure_agent_membership_for_role(
        &self,
        task_id: &str,
        role: &str,
        agent_id: &str,
    ) -> Result<()> {
        if canonical_task_role_name(role).is_none() {
            return Ok(());
        }
        if let Some(memberships) =
            current_role_memberships_authoritative(&self.db, task_id, role).await?
        {
            if active_agent_membership(&memberships, agent_id).is_none() {
                return Err(ServiceError::conflict(format!(
                    "Agent {agent_id} is not an active member of role {role}"
                )));
            }
        }
        Ok(())
    }

    async fn sync_legacy_role_projection_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        task: &db::Task,
        task_role_id: &str,
        canonical_role: &str,
        fallback_role: Option<&str>,
        preferred_projection_role: Option<&str>,
    ) -> Result<()> {
        project_actor_scope::sync_legacy_role_projection_in_tx(
            transaction,
            &task.id,
            task_role_id,
            canonical_role,
            fallback_role,
            preferred_projection_role,
        )
        .await
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
        let workflow =
            crate::workflow::engine::WorkflowEngine::resolve_workflow(&project.workflow_definition);
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
        // Execution labels such as `interactive`, `merge_fixer`, and `system`
        // are not TaskRole vocabulary.  They therefore retain their existing
        // execution-path semantics instead of being mistaken for an
        // authoritative empty membership set.
        return Ok(None);
    };
    let Some(task_role) = TaskRoleRepo::get_by_task_and_role(db, task_id, &role).await? else {
        return Ok(None);
    };
    RoleMembershipRepo::list_by_role(db, &task_role.id, false)
        .await
        .map(Some)
        .map_err(Into::into)
}

/// Select the first currently usable Agent from an authoritative membership
/// set. The membership query already supplies the stable `(created_at, id)`
/// order; unusable candidates are skipped rather than terminating selection.
pub(crate) async fn select_usable_agent_id(
    db: &db::SqliteDb,
    memberships: &[RoleMembership],
) -> Result<Option<String>> {
    for membership in memberships {
        if membership.status != RoleMembershipStatus::Active
            || membership.actor_kind != ActorKind::Agent
        {
            continue;
        }
        let Some(agent) = AgentRepo::get_by_id(db, &membership.actor_id).await? else {
            continue;
        };
        if compute_effective_status(db, &agent).await? == EffectiveStatus::Active {
            return Ok(Some(agent.id));
        }
    }
    Ok(None)
}

/// Return whether an Agent can receive repository workspace authority. This
/// is deliberately narrower than TaskRole membership validity: active Main
/// and Project Agent bindings identify orchestration identities that may
/// participate in a role but cannot receive a WorkspaceLease.
pub(crate) async fn repository_worker_identity_is_eligible(
    db: &db::SqliteDb,
    project_id: &str,
    principal_id: &str,
) -> Result<bool> {
    let orchestration_binding_count: i64 = sqlx::query_scalar(
        "SELECT
            (SELECT COUNT(*) FROM project_agent_binding
             WHERE project_id = ? AND identity_id = ? AND state = 'active')
          + (SELECT COUNT(*) FROM account_main_agent_binding
             WHERE identity_id = ? AND state = 'active')",
    )
    .bind(project_id)
    .bind(principal_id)
    .bind(principal_id)
    .fetch_one(db.pool())
    .await?;
    Ok(orchestration_binding_count == 0)
}

/// Select a deterministic Agent that is both runtime-usable and capable of
/// receiving repository workspace authority. The generic selector remains
/// available for non-repository contexts.
pub(crate) async fn select_usable_repository_agent_id(
    db: &db::SqliteDb,
    project_id: &str,
    memberships: &[RoleMembership],
) -> Result<Option<String>> {
    for membership in memberships {
        if membership.status != RoleMembershipStatus::Active
            || membership.actor_kind != ActorKind::Agent
        {
            continue;
        }
        let Some(agent) = AgentRepo::get_by_id(db, &membership.actor_id).await? else {
            continue;
        };
        if compute_effective_status(db, &agent).await? != EffectiveStatus::Active {
            continue;
        }
        if repository_worker_identity_is_eligible(db, project_id, &agent.id).await? {
            return Ok(Some(agent.id));
        }
    }
    Ok(None)
}

/// Check whether a specific lineage Agent is still both eligible and usable.
/// This preserves lineage preference without allowing a paused, unavailable,
/// or full Agent to hide a later usable membership.
pub(crate) async fn is_usable_active_agent(
    db: &db::SqliteDb,
    memberships: &[RoleMembership],
    agent_id: &str,
) -> Result<bool> {
    let Some(membership) = active_agent_membership(memberships, agent_id) else {
        return Ok(false);
    };
    let Some(agent) = AgentRepo::get_by_id(db, &membership.actor_id).await? else {
        return Ok(false);
    };
    Ok(compute_effective_status(db, &agent).await? == EffectiveStatus::Active)
}

pub(crate) async fn is_usable_repository_agent(
    db: &db::SqliteDb,
    project_id: &str,
    memberships: &[RoleMembership],
    agent_id: &str,
) -> Result<bool> {
    if !is_usable_active_agent(db, memberships, agent_id).await? {
        return Ok(false);
    }
    repository_worker_identity_is_eligible(db, project_id, agent_id).await
}

pub(crate) fn active_agent_membership<'a>(
    memberships: &'a [RoleMembership],
    agent_id: &str,
) -> Option<&'a RoleMembership> {
    memberships.iter().find(|membership| {
        membership.status == RoleMembershipStatus::Active
            && membership.actor_kind == ActorKind::Agent
            && membership.actor_id == agent_id
    })
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
    let value: serde_json::Value = serde_json::from_str(policy_json).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid role policy: {error}"))
    })?;
    if !value.is_object() {
        return Err(ServiceError::invalid_operation(
            "role policy must be a JSON object",
        ));
    }
    Ok(())
}

fn map_membership_write_error(error: sqlx::Error) -> ServiceError {
    if let sqlx::Error::Database(database_error) = &error {
        if database_error.message().contains("UNIQUE") {
            return ServiceError::conflict("actor already has a current membership in this role");
        }
        if database_error
            .message()
            .contains("coordination_mode is required")
        {
            return ServiceError::invalid_operation(
                "set coordination_mode before adding a second current member",
            );
        }
    }
    ServiceError::Db(db::DbError::Sqlx(error))
}
