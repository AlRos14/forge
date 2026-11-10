use super::*;
use api_types::ActorRef;
use db::{
    canonical_task_role_name, new_uuid_v4, now_rfc3339, ActorKind, CoordinationMode,
    CreateRoleMembership, CreateTaskRole, ProjectRepo, ProjectTaskRoleDefaultsRepo, RoleMembership,
    RoleMembershipRepo, RoleMembershipStatus, TaskRole, TaskRoleRepo, UpdateRoleMembership,
    UpdateTaskRole, UpdateTaskRoleWithConfiguration, UserRepo,
};
use sqlx::{Sqlite, Transaction};
use std::collections::HashMap;

use crate::agent_service::{compute_effective_status, EffectiveStatus};
use crate::project_actor_scope;

impl TaskService {
    pub(crate) async fn apply_project_task_role_defaults_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        task_id: &str,
        created_at: &str,
    ) -> Result<()> {
        let project = ProjectRepo::get_by_id_in_tx(&*self.db, transaction, project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))?;
        let defaults = ProjectTaskRoleDefaultsRepo::get_task_role_defaults_in_tx(
            &*self.db,
            transaction,
            project_id,
        )
        .await?;

        let task_role_ids = defaults
            .roles
            .iter()
            .map(|default| (default.role.clone(), new_uuid_v4()))
            .collect::<HashMap<_, _>>();

        let mut preferred_coordinator: Option<(String, api_types::ActorRef)> = None;
        for default in defaults.roles {
            let role = canonical_task_role_name(&default.role).ok_or_else(|| {
                ServiceError::invalid_operation("Project default role is not a TaskRole")
            })?;
            if role != default.role {
                return Err(ServiceError::invalid_operation(
                    "Project default role is not canonical",
                ));
            }
            let project_configuration: api_types::ProjectTaskRoleConfiguration =
                serde_json::from_str(&default.configuration_json).map_err(|error| {
                    ServiceError::invalid_operation(format!(
                        "Project TaskRole configuration is invalid: {error}"
                    ))
                })?;
            if let Some(actor) = project_configuration.coordinator_actor.as_ref() {
                if preferred_coordinator.is_some() {
                    return Err(ServiceError::invalid_operation(
                        "Project defaults name multiple primary coordinators",
                    ));
                }
                preferred_coordinator = Some((task_role_ids[&role].clone(), actor.clone()));
            }
            let configuration = crate::task_role_configuration::project_configuration_to_task_role(
                &project_configuration,
                &task_role_ids,
            )?;
            let configuration_json =
                crate::task_role_configuration::store_task_role_configuration(&configuration)?;
            let task_role = TaskRoleRepo::create_with_configuration_in_tx(
                &*self.db,
                transaction,
                CreateTaskRole {
                    id: task_role_ids[&role].clone(),
                    task_id: task_id.to_owned(),
                    role,
                    coordination_mode: Some(default.coordination_mode),
                    policy_json: "{}".to_owned(),
                    created_at: created_at.to_owned(),
                    updated_at: created_at.to_owned(),
                },
                &configuration_json,
            )
            .await?;

            for actor in default.members {
                let actor_ref = match actor {
                    db::ActorRef::Human(id) => ActorRef::Human(id),
                    db::ActorRef::Agent(id) => ActorRef::Agent(id),
                };
                let valid = project_actor_scope::actor_is_valid_for_project_in_tx(
                    transaction,
                    project_id,
                    project.owner_id.as_deref(),
                    &actor_ref,
                )
                .await?;
                let exact_agent_owner = match &actor_ref {
                    ActorRef::Human(_) => true,
                    ActorRef::Agent(agent_id) => {
                        let owner_id: Option<String> =
                            sqlx::query_scalar("SELECT owner_id FROM agent_current WHERE id = ?")
                                .bind(agent_id)
                                .fetch_optional(&mut **transaction)
                                .await?
                                .flatten();
                        owner_id.is_some() && owner_id == project.owner_id
                    }
                };
                if !valid || !exact_agent_owner {
                    return Err(ServiceError::invalid_operation(
                        "Project default Actor is no longer authorized for this Project",
                    ));
                }
                let (actor_kind, actor_id) = match actor_ref {
                    ActorRef::Human(id) => (ActorKind::Human, id),
                    ActorRef::Agent(id) => (ActorKind::Agent, id),
                };
                RoleMembershipRepo::add_in_tx(
                    &*self.db,
                    transaction,
                    CreateRoleMembership {
                        id: new_uuid_v4(),
                        task_role_id: task_role.id.clone(),
                        actor_kind,
                        actor_id,
                        status: RoleMembershipStatus::Active,
                        created_at: created_at.to_owned(),
                        updated_at: created_at.to_owned(),
                    },
                )
                .await?;
            }
        }
        let envelope =
            serde_json::to_string(&crate::task_coordination::CoordinationEnvelope::default())
                .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let digest = <sha2::Sha256 as sha2::Digest>::digest(envelope.as_bytes());
        sqlx::query("UPDATE task_coordination SET envelope_json=?,envelope_digest=? WHERE task_id=? AND mode='manual' AND authorization_version=1")
            .bind(envelope).bind(hex::encode(digest)).bind(task_id).execute(&mut **transaction).await?;
        if let Some((role_id, api_types::ActorRef::Agent(actor_id))) = preferred_coordinator {
            // Preference only: authorization remains manual until the Human
            // starts this Task. Later Project edits do not rewrite the hint.
            sqlx::query("UPDATE task_coordination SET coordinator_task_role_id=?,coordinator_actor_kind='agent',coordinator_actor_id=? WHERE task_id=? AND mode='manual' AND authorization_version=1")
                .bind(role_id).bind(actor_id).bind(task_id).execute(&mut **transaction).await?;
        }
        Ok(())
    }

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
        let configuration_json = crate::task_role_configuration::store_task_role_configuration(
            &api_types::TaskRoleConfiguration::default(),
        )?;
        self.create_task_role_with_configuration(
            task_id,
            role,
            coordination_mode,
            policy_json,
            configuration_json,
        )
        .await
    }

    pub async fn create_task_role_with_configuration(
        &self,
        task_id: &str,
        role: &str,
        coordination_mode: CoordinationMode,
        policy_json: String,
        configuration_json: String,
    ) -> Result<TaskRole> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let role = canonical_task_role_name(role)
            .ok_or_else(|| ServiceError::invalid_operation("role is not a TaskRole"))?;
        validate_policy_json(&policy_json)?;
        let configuration =
            crate::task_role_configuration::parse_task_role_configuration(&configuration_json)?;
        self.validate_role_configuration_references(&task.id, &configuration)
            .await?;
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
        let mut transaction = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let task_role = TaskRoleRepo::create_with_configuration_in_tx(
            &*self.db,
            &mut transaction,
            CreateTaskRole {
                id: new_uuid_v4(),
                task_id: task.id,
                role,
                coordination_mode: Some(coordination_mode),
                policy_json,
                created_at: now.clone(),
                updated_at: now,
            },
            &configuration_json,
        )
        .await?;
        transaction.commit().await?;
        Ok(task_role)
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
            let configuration =
                db::TaskRoleCapabilityConfigurationRepo::get_by_task_role_id(&*self.db, &role.id)
                    .await?
                    .ok_or_else(|| {
                        ServiceError::invalid_operation(
                            "TaskRole capability configuration is missing",
                        )
                    })?;
            let configuration = crate::task_role_configuration::parse_task_role_configuration(
                &configuration.configuration_json,
            )?;
            self.validate_role_configuration_references(&role.task_id, &configuration)
                .await?;
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

    pub async fn update_task_role_with_configuration(
        &self,
        task_id: &str,
        role_name: &str,
        expected_version: i64,
        coordination_mode: Option<CoordinationMode>,
        policy_json: Option<String>,
        configuration_json: String,
    ) -> Result<TaskRole> {
        let role = self.task_role_for_task(task_id, role_name).await?;
        if let Some(policy_json) = policy_json.as_deref() {
            validate_policy_json(policy_json)?;
        }
        let configuration =
            crate::task_role_configuration::parse_task_role_configuration(&configuration_json)?;
        self.validate_role_configuration_references(&role.task_id, &configuration)
            .await?;
        TaskRoleRepo::update_with_configuration(
            &*self.db,
            UpdateTaskRoleWithConfiguration {
                id: role.id,
                expected_version,
                coordination_mode: coordination_mode.map(Some),
                policy_json,
                configuration_json,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .map_err(Into::into)
    }

    async fn validate_role_configuration_references(
        &self,
        task_id: &str,
        configuration: &api_types::TaskRoleConfiguration,
    ) -> Result<()> {
        let Some(rework) = configuration.rework.as_ref() else {
            return Ok(());
        };
        let roles = TaskRoleRepo::list_by_task(&*self.db, task_id).await?;
        let mut by_id = HashMap::with_capacity(roles.len());
        for role in &roles {
            by_id.insert(role.id.as_str(), role);
        }
        for role_id in rework
            .allowed_destination_role_ids
            .iter()
            .chain(rework.next_review_role_id.iter())
        {
            if !by_id.contains_key(role_id.as_str()) {
                return Err(ServiceError::invalid_operation(format!(
                    "rework route references a TaskRole outside this Task: {role_id}"
                )));
            }
        }
        if let Some(review_role_id) = rework.next_review_role_id.as_deref() {
            let capability_config = db::TaskRoleCapabilityConfigurationRepo::get_by_task_role_id(
                &*self.db,
                review_role_id,
            )
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "next review TaskRole capability configuration is missing",
                )
            })?;
            let capabilities = crate::task_role_configuration::parse_task_role_configuration(
                &capability_config.configuration_json,
            )?
            .capabilities;
            if !capabilities.contains(&api_types::TaskRoleCapability::FormalReview) {
                return Err(ServiceError::invalid_operation(
                    "next review TaskRole must have formal_review capability",
                ));
            }
        }
        Ok(())
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
        let membership = RoleMembershipRepo::add(
            &*self.db,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id: task_role.id,
                actor_kind: actor_kind(&actor_ref),
                actor_id: actor_id(&actor_ref).to_owned(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;
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
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let updated_at = now_rfc3339();
        let ended_at = (status == RoleMembershipStatus::Ended).then(|| updated_at.clone());
        let updated = RoleMembershipRepo::update(
            &*self.db,
            UpdateRoleMembership {
                id: membership.id.clone(),
                expected_version,
                status,
                updated_at,
                ended_at,
            },
        )
        .await?;
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
}

/// Canonical roles always resolve to the target membership set. A missing
/// TaskRole is an empty set; it never falls back to a singular assignment.
/// `None` is reserved for execution labels outside TaskRole vocabulary.
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
        return Ok(Some(Vec::new()));
    };
    RoleMembershipRepo::list_by_role(db, &task_role.id, false)
        .await
        .map(Some)
        .map_err(Into::into)
}

pub(crate) async fn human_is_active_task_role_member_authoritative(
    db: &db::SqliteDb,
    task_id: &str,
    task_role_id: &str,
    user_id: &str,
) -> Result<bool> {
    let Some(role) = TaskRoleRepo::get_by_id(db, task_role_id).await? else {
        return Ok(false);
    };
    if role.task_id != task_id {
        return Ok(false);
    }
    Ok(RoleMembershipRepo::list_by_role(db, task_role_id, false)
        .await?
        .iter()
        .any(|membership| {
            membership.actor_kind == ActorKind::Human
                && membership.actor_id == user_id
                && membership.status == RoleMembershipStatus::Active
        }))
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

/// Return whether an Agent meets generic Project eligibility for a repository
/// WorkspaceLease. TaskRole membership is checked at the lease authority
/// boundary; historical Main/Project bindings do not affect eligibility.
pub(crate) async fn repository_worker_identity_is_eligible(
    db: &db::SqliteDb,
    project_id: &str,
    principal_id: &str,
) -> Result<bool> {
    let Some(project) = ProjectRepo::get_by_id(db, project_id).await? else {
        return Ok(false);
    };
    project_actor_scope::actor_is_valid_for_project(
        db,
        &project,
        &ActorRef::Agent(principal_id.to_owned()),
    )
    .await
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
