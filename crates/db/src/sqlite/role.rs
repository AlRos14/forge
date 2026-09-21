use super::*;
use crate::{
    ActorKind, CoordinationMode, CreateRoleMembership, CreateTaskRole, DbError,
    RoleMembership, RoleMembershipRepo, RoleMembershipStatus, TaskRole, TaskRoleRepo,
    UpdateRoleMembership, UpdateTaskRole,
};
use sqlx::{sqlite::SqliteRow, Row};
use std::str::FromStr;

fn map_role_sqlx_error(error: sqlx::Error) -> DbError {
    match error {
        sqlx::Error::RowNotFound => DbError::NotFound,
        other => DbError::Sqlx(other),
    }
}

fn map_task_role(row: &SqliteRow) -> crate::Result<TaskRole> {
    Ok(TaskRole {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        role: row.try_get("role")?,
        coordination_mode: row
            .try_get::<Option<String>, _>("coordination_mode")?
            .map(|value| CoordinationMode::from_str(&value).map_err(|_| DbError::InvalidTransition))
            .transpose()?,
        policy_json: row.try_get("policy_json")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_membership(row: &SqliteRow) -> crate::Result<RoleMembership> {
    Ok(RoleMembership {
        id: row.try_get("id")?,
        task_role_id: row.try_get("task_role_id")?,
        actor_kind: ActorKind::from_str(&row.try_get::<String, _>("actor_kind")?)
            .map_err(|_| DbError::InvalidTransition)?,
        actor_id: row.try_get("actor_id")?,
        status: RoleMembershipStatus::from_str(&row.try_get::<String, _>("status")?)
            .map_err(|_| DbError::InvalidTransition)?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        ended_at: row.try_get("ended_at")?,
    })
}

const TASK_ROLE_COLUMNS: &str =
    "id, task_id, role, coordination_mode, policy_json, version, created_at, updated_at";
const MEMBERSHIP_COLUMNS: &str =
    "id, task_role_id, actor_kind, actor_id, status, version, created_at, updated_at, ended_at";

#[async_trait]
impl TaskRoleRepo for SqliteDb {
    async fn create(&self, input: CreateTaskRole) -> crate::Result<TaskRole> {
        sqlx::query(
            "INSERT INTO task_role
                (id, task_id, role, coordination_mode, policy_json, version, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.role)
        .bind(input.coordination_mode.map(|mode| mode.to_string()))
        .bind(&input.policy_json)
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .execute(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;

        self.get_by_task_and_role(&input.task_id, &input.role)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_task_and_role(
        &self,
        task_id: &str,
        role: &str,
    ) -> crate::Result<Option<TaskRole>> {
        let row = sqlx::query(&format!(
            "SELECT {TASK_ROLE_COLUMNS} FROM task_role WHERE task_id = ? AND role = ?"
        ))
        .bind(task_id)
        .bind(role)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;
        row.as_ref().map(map_task_role).transpose()
    }

    async fn get_by_id(&self, id: &str) -> crate::Result<Option<TaskRole>> {
        let row = sqlx::query(&format!(
            "SELECT {TASK_ROLE_COLUMNS} FROM task_role WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;
        row.as_ref().map(map_task_role).transpose()
    }

    async fn list_by_task(&self, task_id: &str) -> crate::Result<Vec<TaskRole>> {
        let rows = sqlx::query(&format!(
            "SELECT {TASK_ROLE_COLUMNS} FROM task_role WHERE task_id = ? ORDER BY role, id"
        ))
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;
        rows.iter().map(map_task_role).collect()
    }

    async fn update(&self, input: UpdateTaskRole) -> crate::Result<TaskRole> {
        let current = sqlx::query(&format!(
            "SELECT {TASK_ROLE_COLUMNS} FROM task_role WHERE id = ?"
        ))
        .bind(&input.id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?
        .ok_or(DbError::NotFound)?;
        let current = map_task_role(&current)?;
        let coordination_mode = input
            .coordination_mode
            .map(|mode| mode.map(|mode| mode.to_string()))
            .unwrap_or_else(|| current.coordination_mode.map(|mode| mode.to_string()));
        let policy_json = input.policy_json.unwrap_or(current.policy_json);
        let result = sqlx::query(
            "UPDATE task_role
             SET coordination_mode = ?, policy_json = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(coordination_mode)
        .bind(policy_json)
        .bind(&input.updated_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;
        if result.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }
        let row = sqlx::query(&format!(
            "SELECT {TASK_ROLE_COLUMNS} FROM task_role WHERE id = ?"
        ))
        .bind(&input.id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;
        map_task_role(&row)
    }
}

#[async_trait]
impl RoleMembershipRepo for SqliteDb {
    async fn add(&self, input: CreateRoleMembership) -> crate::Result<RoleMembership> {
        let ended_at = (input.status == RoleMembershipStatus::Ended)
            .then(|| input.updated_at.clone());
        sqlx::query(
            "INSERT INTO role_membership
                (id, task_role_id, actor_kind, actor_id, status, version, created_at, updated_at, ended_at)
             VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_role_id)
        .bind(input.actor_kind.to_string())
        .bind(&input.actor_id)
        .bind(input.status.to_string())
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .bind(ended_at)
        .execute(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;
        self.get(&input.id).await?.ok_or(DbError::NotFound)
    }

    async fn get(&self, id: &str) -> crate::Result<Option<RoleMembership>> {
        let row = sqlx::query(&format!(
            "SELECT {MEMBERSHIP_COLUMNS} FROM role_membership WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;
        row.as_ref().map(map_membership).transpose()
    }

    async fn list_by_role(
        &self,
        task_role_id: &str,
        include_ended: bool,
    ) -> crate::Result<Vec<RoleMembership>> {
        let status_clause = if include_ended {
            ""
        } else {
            " AND status IN ('active', 'suspended')"
        };
        let rows = sqlx::query(&format!(
            "SELECT {MEMBERSHIP_COLUMNS}
             FROM role_membership WHERE task_role_id = ?{status_clause}
             ORDER BY created_at, id"
        ))
        .bind(task_role_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;
        rows.iter().map(map_membership).collect()
    }

    async fn list_by_task(
        &self,
        task_id: &str,
        include_ended: bool,
    ) -> crate::Result<Vec<(TaskRole, RoleMembership)>> {
        let status_clause = if include_ended {
            ""
        } else {
            " AND membership.status IN ('active', 'suspended')"
        };
        let rows = sqlx::query(&format!(
            "SELECT
                role.id AS role_id, role.task_id AS role_task_id, role.role AS role_name,
                role.coordination_mode, role.policy_json, role.version AS role_version,
                role.created_at AS role_created_at, role.updated_at AS role_updated_at,
                membership.id, membership.task_role_id, membership.actor_kind,
                membership.actor_id, membership.status, membership.version,
                membership.created_at, membership.updated_at, membership.ended_at
             FROM task_role AS role
             JOIN role_membership AS membership ON membership.task_role_id = role.id
             WHERE role.task_id = ?{status_clause}
             ORDER BY role.role, membership.created_at, membership.id"
        ))
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;

        rows.iter()
            .map(|row| {
                let task_role = TaskRole {
                    id: row.try_get("role_id")?,
                    task_id: row.try_get("role_task_id")?,
                    role: row.try_get("role_name")?,
                    coordination_mode: row
                        .try_get::<Option<String>, _>("coordination_mode")?
                        .map(|value| {
                            CoordinationMode::from_str(&value)
                                .map_err(|_| DbError::InvalidTransition)
                        })
                        .transpose()?,
                    policy_json: row.try_get("policy_json")?,
                    version: row.try_get("role_version")?,
                    created_at: row.try_get("role_created_at")?,
                    updated_at: row.try_get("role_updated_at")?,
                };
                Ok((task_role, map_membership(row)?))
            })
            .collect()
    }

    async fn update(&self, input: UpdateRoleMembership) -> crate::Result<RoleMembership> {
        let ended_at = match input.status {
            RoleMembershipStatus::Ended => Some(input.ended_at.unwrap_or_else(crate::now_rfc3339)),
            RoleMembershipStatus::Active | RoleMembershipStatus::Suspended => None,
        };
        let result = sqlx::query(
            "UPDATE role_membership
             SET status = ?, version = version + 1, updated_at = ?, ended_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(input.status.to_string())
        .bind(&input.updated_at)
        .bind(ended_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&self.pool)
        .await
        .map_err(map_role_sqlx_error)?;
        if result.rows_affected() == 0 {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM role_membership WHERE id = ?")
                .bind(&input.id)
                .fetch_one(&self.pool)
                .await
                .map_err(map_role_sqlx_error)?;
            return Err(if exists == 0 {
                DbError::NotFound
            } else {
                DbError::VersionConflict
            });
        }
        self.get(&input.id).await?.ok_or(DbError::NotFound)
    }
}
