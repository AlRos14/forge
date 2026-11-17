use super::*;
use crate::{ActorRef, CoordinationMode, DbError, ProjectTaskRoleDefault};
use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;

#[async_trait]
impl ProjectTaskRoleDefaultsRepo for SqliteDb {
    async fn get_task_role_defaults(&self, project_id: &str) -> Result<ProjectTaskRoleDefaults> {
        let mut transaction = self.pool.begin().await?;
        let defaults = self
            .get_task_role_defaults_in_tx(&mut transaction, project_id)
            .await?;
        transaction.commit().await?;
        Ok(defaults)
    }

    async fn get_task_role_defaults_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        project_id: &str,
    ) -> Result<ProjectTaskRoleDefaults> {
        let header = sqlx::query(
            "SELECT version, updated_at FROM project_task_role_defaults WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(DbError::NotFound)?;
        let version: i64 = header.try_get("version")?;
        let updated_at: String = header.try_get("updated_at")?;
        let rows = sqlx::query(
            "SELECT role.role, role.coordination_mode, role.configuration_json,
                    member.actor_kind, member.actor_id
             FROM project_task_role_default_role AS role
             LEFT JOIN project_task_role_default_member AS member
               ON member.project_id = role.project_id AND member.role = role.role
             WHERE role.project_id = ?
             ORDER BY role.role, member.actor_kind, member.actor_id",
        )
        .bind(project_id)
        .fetch_all(&mut **transaction)
        .await?;

        let mut roles: BTreeMap<String, ProjectTaskRoleDefault> = BTreeMap::new();
        for row in rows {
            let role_name: String = row.try_get("role")?;
            let coordination_mode =
                CoordinationMode::from_str(&row.try_get::<String, _>("coordination_mode")?)
                    .map_err(|_| DbError::InvalidTransition)?;
            let role = roles
                .entry(role_name.clone())
                .or_insert_with(|| ProjectTaskRoleDefault {
                    role: role_name,
                    coordination_mode,
                    configuration_json: row
                        .try_get("configuration_json")
                        .expect("selected configuration_json has a stable SQLite type"),
                    members: Vec::new(),
                });
            if let (Some(actor_kind), Some(actor_id)) = (
                row.try_get::<Option<String>, _>("actor_kind")?,
                row.try_get::<Option<String>, _>("actor_id")?,
            ) {
                role.members.push(match actor_kind.as_str() {
                    "human" => ActorRef::Human(actor_id),
                    "agent" => ActorRef::Agent(actor_id),
                    _ => return Err(DbError::InvalidTransition),
                });
            }
        }

        Ok(ProjectTaskRoleDefaults {
            project_id: project_id.to_owned(),
            version,
            roles: roles.into_values().collect(),
            updated_at,
        })
    }

    async fn replace_task_role_defaults(
        &self,
        project_id: &str,
        caller_user_id: &str,
        expected_version: i64,
        roles: Vec<ProjectTaskRoleDefault>,
        updated_at: &str,
    ) -> Result<ProjectTaskRoleDefaults> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let project_owner_id: Option<String> =
            sqlx::query_scalar("SELECT owner_id FROM project WHERE id = ?")
                .bind(project_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(DbError::NotFound)?;
        let is_admin: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM project_member
                WHERE project_id = ? AND user_id = ? AND role IN ('owner', 'admin')
            )",
        )
        .bind(project_id)
        .bind(caller_user_id)
        .fetch_one(&mut *transaction)
        .await?;
        if is_admin == 0 {
            return Err(DbError::ProjectAdminRequired);
        }

        let current = self
            .get_task_role_defaults_in_tx(&mut transaction, project_id)
            .await?;
        if current.version != expected_version {
            return Err(DbError::VersionConflict);
        }

        let mut seen_roles = HashSet::new();
        for role in &roles {
            let canonical = crate::canonical_task_role_name(&role.role)
                .ok_or_else(|| DbError::Check("role is not a TaskRole".to_owned()))?;
            if canonical != role.role || role.role.len() > 64 {
                return Err(DbError::Check(
                    "role must be a canonical TaskRole name of at most 64 bytes".to_owned(),
                ));
            }
            if !seen_roles.insert(role.role.clone()) {
                return Err(DbError::Check(format!(
                    "duplicate Project default role: {}",
                    role.role
                )));
            }
        }
        let available_roles = seen_roles.clone();
        seen_roles.clear();
        let mut coordinator_count = 0;
        for role in &roles {
            let canonical = crate::canonical_task_role_name(&role.role)
                .ok_or_else(|| DbError::Check("role is not a TaskRole".to_owned()))?;
            if canonical != role.role || role.role.len() > 64 {
                return Err(DbError::Check(
                    "role must be a canonical TaskRole name of at most 64 bytes".to_owned(),
                ));
            }
            if !seen_roles.insert(role.role.clone()) {
                return Err(DbError::Check(format!(
                    "duplicate Project default role: {}",
                    role.role
                )));
            }
            validate_project_role_configuration(&role.configuration_json, &available_roles)?;
            let configuration: serde_json::Value =
                serde_json::from_str(&role.configuration_json)
                    .map_err(|error| DbError::Check(error.to_string()))?;
            if let Some(value) = configuration
                .get("coordinator_actor")
                .filter(|value| !value.is_null())
            {
                coordinator_count += 1;
                let actor: ActorRef = serde_json::from_value(value.clone())
                    .map_err(|error| DbError::Check(error.to_string()))?;
                let orchestration = configuration
                    .get("capabilities")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|items| items.iter().any(|item| item == "orchestration"));
                if coordinator_count > 1
                    || !orchestration
                    || actor.kind() != crate::ActorKind::Agent
                    || !role.members.contains(&actor)
                {
                    return Err(DbError::Check("Choose exactly one eligible Agent as the Project's default Task coordinator".to_owned()));
                }
            }
            let mut seen_members = HashSet::new();
            for actor in &role.members {
                let (kind, actor_id) = match actor {
                    ActorRef::Human(actor_id) => ("human", actor_id.as_str()),
                    ActorRef::Agent(actor_id) => ("agent", actor_id.as_str()),
                };
                if actor_id.trim().is_empty() || actor_id == "human" {
                    return Err(DbError::Check(
                        "Project default members require a durable Actor identity".to_owned(),
                    ));
                }
                if !seen_members.insert((kind, actor_id)) {
                    return Err(DbError::Check(format!(
                        "duplicate {kind} member in Project default role {}",
                        role.role
                    )));
                }
                let valid: i64 = match kind {
                    "human" => {
                        sqlx::query_scalar(
                            "SELECT EXISTS(
                            SELECT 1 FROM project_member AS member
                            JOIN user ON user.id = member.user_id
                            WHERE member.project_id = ? AND member.user_id = ?
                        )",
                        )
                        .bind(project_id)
                        .bind(actor_id)
                        .fetch_one(&mut *transaction)
                        .await?
                    }
                    _ => {
                        sqlx::query_scalar(
                            "SELECT EXISTS(
                            SELECT 1 FROM agent_current
                            WHERE id = ? AND owner_id IS ?
                              AND owner_id IS NOT NULL
                        )",
                        )
                        .bind(actor_id)
                        .bind(project_owner_id.as_deref())
                        .fetch_one(&mut *transaction)
                        .await?
                    }
                };
                if valid == 0 {
                    return Err(DbError::Check(match kind {
                        "human" => {
                            "Project default humans must be current Project members".to_owned()
                        }
                        _ => "Project default Agents must be owned by the Project owner".to_owned(),
                    }));
                }
            }
        }

        sqlx::query("DELETE FROM project_task_role_default_role WHERE project_id = ?")
            .bind(project_id)
            .execute(&mut *transaction)
            .await?;
        for role in roles {
            sqlx::query(
                "INSERT INTO project_task_role_default_role
                    (project_id, role, coordination_mode, configuration_json, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(project_id)
            .bind(&role.role)
            .bind(role.coordination_mode.to_string())
            .bind(&role.configuration_json)
            .bind(updated_at)
            .bind(updated_at)
            .execute(&mut *transaction)
            .await?;
            for actor in role.members {
                let (actor_kind, actor_id) = match actor {
                    ActorRef::Human(id) => ("human", id),
                    ActorRef::Agent(id) => ("agent", id),
                };
                sqlx::query(
                    "INSERT INTO project_task_role_default_member
                        (project_id, role, actor_kind, actor_id)
                     VALUES (?, ?, ?, ?)",
                )
                .bind(project_id)
                .bind(&role.role)
                .bind(actor_kind)
                .bind(actor_id)
                .execute(&mut *transaction)
                .await?;
            }
        }
        let result = sqlx::query(
            "UPDATE project_task_role_defaults
             SET version = version + 1, updated_at = ?
             WHERE project_id = ? AND version = ?",
        )
        .bind(updated_at)
        .bind(project_id)
        .bind(expected_version)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }
        let defaults = self
            .get_task_role_defaults_in_tx(&mut transaction, project_id)
            .await?;
        transaction.commit().await?;
        Ok(defaults)
    }
}

fn validate_project_role_configuration(raw: &str, available_roles: &HashSet<String>) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
        DbError::Check(format!("invalid Project TaskRole configuration: {error}"))
    })?;
    let object = value.as_object().ok_or_else(|| {
        DbError::Check("Project TaskRole configuration must be an object".to_owned())
    })?;
    if object
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|version| version != 1)
    {
        return Err(DbError::Check(
            "unsupported Project TaskRole configuration schema version".to_owned(),
        ));
    }
    if let Some(capabilities) = object.get("capabilities") {
        let capabilities = capabilities.as_array().ok_or_else(|| {
            DbError::Check("Project TaskRole capabilities must be an array".to_owned())
        })?;
        let mut seen = HashSet::new();
        for capability in capabilities {
            let name = capability.as_str().ok_or_else(|| {
                DbError::Check("Project TaskRole capability must be a string".to_owned())
            })?;
            if !matches!(name, "formal_review" | "orchestration" | "request_rework")
                || !seen.insert(name)
            {
                return Err(DbError::Check(format!(
                    "unknown or duplicate Project TaskRole capability: {name}"
                )));
            }
        }
    }
    if let Some(label) = object.get("display_label") {
        if !label.is_null()
            && (!label
                .as_str()
                .is_some_and(|label| !label.trim().is_empty() && label.len() <= 120))
        {
            return Err(DbError::Check(
                "Project TaskRole display_label must be a non-empty string of at most 120 bytes"
                    .to_owned(),
            ));
        }
    }
    if let Some(rework) = object.get("rework").filter(|value| !value.is_null()) {
        let rework = rework.as_object().ok_or_else(|| {
            DbError::Check("Project TaskRole rework configuration must be an object".to_owned())
        })?;
        let destinations = rework
            .get("allowed_destination_roles")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                DbError::Check("Project rework destinations must be an array".to_owned())
            })?;
        for destination in destinations {
            let name = destination.as_str().ok_or_else(|| {
                DbError::Check("Project rework destination must be a role name".to_owned())
            })?;
            if !available_roles.contains(name) {
                return Err(DbError::Check(format!(
                    "Project rework destination role is not configured: {name}"
                )));
            }
        }
        if let Some(next_review) = rework
            .get("next_review_role")
            .and_then(serde_json::Value::as_str)
        {
            if !available_roles.contains(next_review) {
                return Err(DbError::Check(format!(
                    "Project next review role is not configured: {next_review}"
                )));
            }
        }
    }
    Ok(())
}
