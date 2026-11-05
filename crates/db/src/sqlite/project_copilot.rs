use super::*;
use crate::{
    CopilotMessagePageQuery, CreateProjectCopilotMessage, CreateProjectCopilotRun, DbError,
    ProjectCopilotBinding, ProjectCopilotMessage, ProjectCopilotRepo, ProjectCopilotRun,
    ProjectCopilotRunAdmission, ProjectCopilotRunGrant, ProjectCopilotRunStatus,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sqlx::{Row, Sqlite, Transaction};
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessageCursor {
    created_at: String,
    id: String,
}

fn encode_message_cursor(created_at: &str, id: &str) -> crate::Result<String> {
    let encoded = serde_json::to_vec(&MessageCursor {
        created_at: created_at.to_owned(),
        id: id.to_owned(),
    })
    .map_err(|_| DbError::InvalidCursor)?;
    Ok(URL_SAFE_NO_PAD.encode(encoded))
}

fn decode_message_cursor(cursor: Option<&str>) -> crate::Result<Option<MessageCursor>> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| DbError::InvalidCursor)?;
    let cursor: MessageCursor =
        serde_json::from_slice(&bytes).map_err(|_| DbError::InvalidCursor)?;
    if cursor.created_at.trim().is_empty() || cursor.id.trim().is_empty() {
        return Err(DbError::InvalidCursor);
    }
    Ok(Some(cursor))
}

fn map_binding(row: sqlx::sqlite::SqliteRow) -> crate::Result<ProjectCopilotBinding> {
    Ok(ProjectCopilotBinding {
        project_id: row.try_get("project_id")?,
        agent_id: row.try_get("agent_id")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_run(row: sqlx::sqlite::SqliteRow) -> crate::Result<ProjectCopilotRun> {
    let status: String = row.try_get("status")?;
    Ok(ProjectCopilotRun {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        agent_id: row.try_get("agent_id")?,
        user_id: row.try_get("user_id")?,
        binding_version: row.try_get("binding_version")?,
        agent_version: row.try_get("agent_version")?,
        agent_snapshot_json: row.try_get("agent_snapshot_json")?,
        context_task_id: row.try_get("context_task_id")?,
        status: ProjectCopilotRunStatus::from_str(&status).map_err(DbError::Check)?,
        idempotency_key: row.try_get("idempotency_key")?,
        request_digest: row.try_get("request_digest")?,
        mcp_token_hash: row.try_get("mcp_token_hash")?,
        mcp_token_expires_at: row.try_get("mcp_token_expires_at")?,
        failure_code: row.try_get("failure_code")?,
        failure_message: row.try_get("failure_message")?,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_message(row: sqlx::sqlite::SqliteRow) -> crate::Result<ProjectCopilotMessage> {
    Ok(ProjectCopilotMessage {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        run_id: row.try_get("run_id")?,
        role: row.try_get("role")?,
        actor_type: row.try_get("actor_type")?,
        actor_id: row.try_get("actor_id")?,
        content: row.try_get("content")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn get_run_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    run_id: &str,
) -> crate::Result<Option<ProjectCopilotRun>> {
    sqlx::query("SELECT * FROM project_copilot_run WHERE id = ?")
        .bind(run_id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(map_run)
        .transpose()
}

async fn get_message_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    run_id: &str,
    role: &str,
) -> crate::Result<Option<ProjectCopilotMessage>> {
    sqlx::query("SELECT * FROM project_copilot_message WHERE run_id = ? AND role = ?")
        .bind(run_id)
        .bind(role)
        .fetch_optional(&mut **transaction)
        .await?
        .map(map_message)
        .transpose()
}

async fn require_owner_or_admin(
    transaction: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    user_id: &str,
) -> crate::Result<String> {
    let row = sqlx::query(
        "SELECT p.owner_id,
                EXISTS(SELECT 1 FROM user WHERE id = ?) AS user_exists,
                EXISTS(SELECT 1 FROM project_member owner_member
                       WHERE owner_member.project_id = p.id
                         AND owner_member.user_id = p.owner_id
                         AND owner_member.role = 'owner') AS owner_membership_valid,
                (SELECT member.role FROM project_member member
                 WHERE member.project_id = p.id AND member.user_id = ?) AS caller_role
         FROM project AS p WHERE p.id = ?",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(DbError::NotFound)?;
    let owner_id: Option<String> = row.try_get("owner_id")?;
    let user_exists: bool = row.try_get("user_exists")?;
    let owner_membership_valid: bool = row.try_get("owner_membership_valid")?;
    let caller_role: Option<String> = row.try_get("caller_role")?;
    if !user_exists
        || !owner_membership_valid
        || !matches!(caller_role.as_deref(), Some("owner" | "admin"))
    {
        return Err(DbError::ProjectAdminRequired);
    }
    owner_id.ok_or(DbError::ProjectAdminRequired)
}

async fn require_project_writer(
    transaction: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    user_id: &str,
) -> crate::Result<String> {
    let row = sqlx::query(
        "SELECT p.owner_id,
                EXISTS(SELECT 1 FROM user WHERE id = ?) AS user_exists,
                EXISTS(SELECT 1 FROM project_member owner_member
                       WHERE owner_member.project_id = p.id
                         AND owner_member.user_id = p.owner_id
                         AND owner_member.role = 'owner') AS owner_membership_valid,
                (SELECT member.role FROM project_member member
                 WHERE member.project_id = p.id AND member.user_id = ?) AS caller_role
         FROM project AS p WHERE p.id = ?",
    )
    .bind(user_id)
    .bind(user_id)
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(DbError::NotFound)?;
    let owner_id: Option<String> = row.try_get("owner_id")?;
    let user_exists: bool = row.try_get("user_exists")?;
    let owner_membership_valid: bool = row.try_get("owner_membership_valid")?;
    let caller_role: Option<String> = row.try_get("caller_role")?;
    if !user_exists
        || !owner_membership_valid
        || !matches!(caller_role.as_deref(), Some("owner" | "admin" | "member"))
    {
        return Err(DbError::ProjectWriteRequired);
    }
    owner_id.ok_or(DbError::ProjectWriteRequired)
}

async fn copilot_run_authority_valid(
    transaction: &mut Transaction<'_, Sqlite>,
    run_id: &str,
    expected_status: &str,
) -> crate::Result<Option<sqlx::sqlite::SqliteRow>> {
    let sql = if expected_status == "queued" {
        "SELECT run.*, agent.version AS current_agent_version,
                agent.paused AS agent_paused, agent.daemon_id AS daemon_id,
                p.owner_id AS project_owner_id, agent.owner_id AS agent_owner_id,
                binding.agent_id AS current_agent_id,
                binding.version AS current_binding_version,
                user_member.role AS user_role,
                owner_member.role AS owner_role
         FROM project_copilot_run AS run
         JOIN project_copilot AS binding ON binding.project_id = run.project_id
         JOIN project AS p ON p.id = run.project_id
         JOIN agent_current AS agent ON agent.id = run.agent_id
         JOIN user ON user.id = run.user_id
         JOIN project_member AS user_member
           ON user_member.project_id = run.project_id AND user_member.user_id = run.user_id
         JOIN project_member AS owner_member
           ON owner_member.project_id = run.project_id AND owner_member.user_id = p.owner_id
         WHERE run.id = ? AND run.status = 'queued'
           AND p.owner_id = agent.owner_id
           AND owner_member.role = 'owner'
           AND user_member.role IN ('owner', 'admin', 'member')
           AND binding.agent_id = run.agent_id
           AND binding.version = run.binding_version
           AND agent.version = run.agent_version
           AND (run.context_task_id IS NULL OR EXISTS (
               SELECT 1 FROM task context_task WHERE context_task.id = run.context_task_id
                 AND context_task.project_id = run.project_id AND context_task.deleted_at IS NULL
           ))"
    } else {
        "SELECT run.*, agent.version AS current_agent_version,
                agent.paused AS agent_paused, agent.daemon_id AS daemon_id,
                p.owner_id AS project_owner_id, agent.owner_id AS agent_owner_id,
                binding.agent_id AS current_agent_id,
                binding.version AS current_binding_version,
                user_member.role AS user_role,
                owner_member.role AS owner_role
         FROM project_copilot_run AS run
         JOIN project_copilot AS binding ON binding.project_id = run.project_id
         JOIN project AS p ON p.id = run.project_id
         JOIN agent_current AS agent ON agent.id = run.agent_id
         JOIN user ON user.id = run.user_id
         JOIN project_member AS user_member
           ON user_member.project_id = run.project_id AND user_member.user_id = run.user_id
         JOIN project_member AS owner_member
           ON owner_member.project_id = run.project_id AND owner_member.user_id = p.owner_id
         WHERE run.id = ? AND run.status = 'running'
           AND run.mcp_token_hash IS NOT NULL
           AND run.mcp_token_expires_at IS NOT NULL
           AND julianday(run.mcp_token_expires_at) > julianday('now')
           AND p.owner_id = agent.owner_id
           AND owner_member.role = 'owner'
           AND user_member.role IN ('owner', 'admin', 'member')
           AND binding.agent_id = run.agent_id
           AND binding.version = run.binding_version
           AND agent.version = run.agent_version
           AND (run.context_task_id IS NULL OR EXISTS (
               SELECT 1 FROM task context_task WHERE context_task.id = run.context_task_id
                 AND context_task.project_id = run.project_id AND context_task.deleted_at IS NULL
           ))"
    };
    let row = sqlx::query(sql)
        .bind(run_id)
        .fetch_optional(&mut **transaction)
        .await?;
    let Some(row) = row else { return Ok(None) };
    if !copilot_daemon_authority_valid(transaction, &row).await? {
        return Ok(None);
    }
    Ok(Some(row))
}

async fn copilot_daemon_authority_valid(
    transaction: &mut Transaction<'_, Sqlite>,
    run: &sqlx::sqlite::SqliteRow,
) -> crate::Result<bool> {
    let snapshot: serde_json::Value =
        match serde_json::from_str(run.try_get::<&str, _>("agent_snapshot_json")?) {
            Ok(snapshot) => snapshot,
            Err(_) => return Ok(false),
        };
    let Some(agent) = sqlx::query("SELECT * FROM agent_current WHERE id=?")
        .bind(run.try_get::<&str, _>("agent_id")?)
        .fetch_optional(&mut **transaction)
        .await?
        .map(map_agent)
        .transpose()?
    else {
        return Ok(false);
    };
    let Some(daemon_id) = agent.daemon_id.as_deref() else {
        return Ok(false);
    };
    let Some(daemon) = sqlx::query("SELECT * FROM daemon WHERE id=?")
        .bind(daemon_id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(map_daemon)
        .transpose()?
    else {
        return Ok(false);
    };
    let owner_is_admin: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user WHERE id=? AND is_admin=1)")
            .bind(agent.owner_id.as_deref())
            .fetch_one(&mut **transaction)
            .await?;
    Ok(!agent.paused
        && daemon.machine_id.starts_with("embedded:")
        && crate::daemon_execution_authorized(
            &daemon,
            agent.owner_id.as_deref(),
            owner_is_admin,
            &agent.executor_type,
        )
        && snapshot.get("daemon_authority")
            == Some(&crate::daemon_authority_snapshot(&agent, &daemon)))
}

#[async_trait]
impl ProjectCopilotRepo for SqliteDb {
    async fn get_copilot_binding(&self, project_id: &str) -> crate::Result<ProjectCopilotBinding> {
        sqlx::query("SELECT * FROM project_copilot WHERE project_id = ?")
            .bind(project_id)
            .fetch_optional(self.pool())
            .await?
            .map(map_binding)
            .transpose()?
            .ok_or(DbError::NotFound)
    }

    async fn update_copilot_binding(
        &self,
        project_id: &str,
        caller_user_id: &str,
        expected_version: i64,
        agent_id: Option<&str>,
        updated_at: &str,
    ) -> crate::Result<ProjectCopilotBinding> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let owner_id = require_owner_or_admin(&mut tx, project_id, caller_user_id).await?;
        let current = sqlx::query("SELECT * FROM project_copilot WHERE project_id = ?")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_binding)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if current.version != expected_version {
            return Err(DbError::VersionConflict);
        }
        let active = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM project_copilot_run
             WHERE project_id = ? AND status IN ('queued', 'running'))",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        if active != 0 {
            return Err(DbError::ProjectCopilotActiveRun);
        }
        if let Some(agent_id) = agent_id {
            let agent = sqlx::query("SELECT owner_id FROM agent_current WHERE id = ?")
                .bind(agent_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(DbError::NotFound)?;
            let agent_owner: Option<String> = agent.try_get("owner_id")?;
            if agent_owner.as_deref() != Some(owner_id.as_str()) {
                return Err(DbError::ProjectAdminRequired);
            }
        }
        sqlx::query(
            "UPDATE project_copilot
             SET agent_id = ?, version = version + 1, updated_at = ?
             WHERE project_id = ? AND version = ?",
        )
        .bind(agent_id)
        .bind(updated_at)
        .bind(project_id)
        .bind(expected_version)
        .execute(&mut *tx)
        .await?;
        let binding = map_binding(
            sqlx::query("SELECT * FROM project_copilot WHERE project_id = ?")
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?,
        )?;
        tx.commit().await?;
        Ok(binding)
    }

    async fn create_copilot_run(
        &self,
        run: CreateProjectCopilotRun,
        user_message: CreateProjectCopilotMessage,
    ) -> crate::Result<ProjectCopilotRunAdmission> {
        if user_message.run_id != run.id
            || user_message.project_id != run.project_id
            || user_message.role != "user"
            || user_message.actor_type != "human"
            || user_message.actor_id != run.user_id
        {
            return Err(DbError::Check(
                "Project Copilot user message does not match run identity".to_owned(),
            ));
        }
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        require_project_writer(&mut tx, &run.project_id, &run.user_id).await?;

        if let Some(existing_row) = sqlx::query(
            "SELECT * FROM project_copilot_run
             WHERE project_id = ? AND idempotency_key = ?",
        )
        .bind(&run.project_id)
        .bind(&run.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            let existing = map_run(existing_row)?;
            if existing.user_id != run.user_id || existing.request_digest != run.request_digest {
                return Err(DbError::IdempotencyConflict);
            }
            let existing_message = get_message_tx(&mut tx, &existing.id, "user")
                .await?
                .ok_or_else(|| DbError::Check("Project Copilot run has no user message".into()))?;
            tx.commit().await?;
            return Ok(ProjectCopilotRunAdmission {
                run: existing,
                user_message: existing_message,
                created: false,
            });
        }

        let active = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM project_copilot_run
             WHERE project_id = ? AND status IN ('queued', 'running'))",
        )
        .bind(&run.project_id)
        .fetch_one(&mut *tx)
        .await?;
        if active != 0 {
            return Err(DbError::ProjectCopilotActiveRun);
        }

        let binding = sqlx::query(
            "SELECT binding.agent_id, binding.version, p.owner_id
             FROM project_copilot AS binding
             JOIN project AS p ON p.id = binding.project_id
             WHERE binding.project_id = ?",
        )
        .bind(&run.project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let binding_agent: Option<String> = binding.try_get("agent_id")?;
        let binding_version: i64 = binding.try_get("version")?;
        let project_owner: Option<String> = binding.try_get("owner_id")?;
        if binding_agent.as_deref() != Some(run.agent_id.as_str())
            || binding_version != run.binding_version
            || project_owner.is_none()
        {
            return Err(DbError::VersionConflict);
        }

        let agent = sqlx::query("SELECT owner_id, version FROM agent_current WHERE id = ?")
            .bind(&run.agent_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(DbError::NotFound)?;
        let agent_owner: Option<String> = agent.try_get("owner_id")?;
        let agent_version: i64 = agent.try_get("version")?;
        if agent_owner != project_owner || agent_version != run.agent_version {
            return Err(DbError::VersionConflict);
        }

        if let Some(task_id) = run.context_task_id.as_deref() {
            let task_exists = sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM task
                 WHERE id = ? AND project_id = ? AND deleted_at IS NULL)",
            )
            .bind(task_id)
            .bind(&run.project_id)
            .fetch_one(&mut *tx)
            .await?;
            if task_exists == 0 {
                return Err(DbError::NotFound);
            }
        }

        sqlx::query(
            "INSERT INTO project_copilot_run (
                id, project_id, agent_id, user_id, binding_version, agent_version,
                agent_snapshot_json, context_task_id, status, idempotency_key,
                request_digest, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?)",
        )
        .bind(&run.id)
        .bind(&run.project_id)
        .bind(&run.agent_id)
        .bind(&run.user_id)
        .bind(run.binding_version)
        .bind(run.agent_version)
        .bind(&run.agent_snapshot_json)
        .bind(run.context_task_id.as_deref())
        .bind(&run.idempotency_key)
        .bind(&run.request_digest)
        .bind(&run.created_at)
        .bind(&run.created_at)
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            if error.to_string().contains("project_copilot_one_active_run") {
                DbError::ProjectCopilotActiveRun
            } else if error.to_string().contains("project_copilot_run.project_id") {
                DbError::IdempotencyConflict
            } else {
                DbError::from(error)
            }
        })?;
        sqlx::query(
            "INSERT INTO project_copilot_message (
                id, project_id, run_id, role, actor_type, actor_id, content, created_at
             ) VALUES (?, ?, ?, 'user', 'human', ?, ?, ?)",
        )
        .bind(&user_message.id)
        .bind(&user_message.project_id)
        .bind(&user_message.run_id)
        .bind(&user_message.actor_id)
        .bind(&user_message.content)
        .bind(&user_message.created_at)
        .execute(&mut *tx)
        .await?;
        let created_run = get_run_tx(&mut tx, &run.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let created_message = get_message_tx(&mut tx, &run.id, "user")
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(ProjectCopilotRunAdmission {
            run: created_run,
            user_message: created_message,
            created: true,
        })
    }

    async fn get_copilot_run_by_idempotency(
        &self,
        project_id: &str,
        user_id: &str,
        idempotency_key: &str,
        request_digest: &str,
    ) -> crate::Result<Option<ProjectCopilotRunAdmission>> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        require_project_writer(&mut tx, project_id, user_id).await?;
        let Some(row) = sqlx::query(
            "SELECT * FROM project_copilot_run
             WHERE project_id = ? AND idempotency_key = ?",
        )
        .bind(project_id)
        .bind(idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        else {
            tx.commit().await?;
            return Ok(None);
        };
        let run = map_run(row)?;
        if run.user_id != user_id || run.request_digest != request_digest {
            return Err(DbError::IdempotencyConflict);
        }
        let user_message = get_message_tx(&mut tx, &run.id, "user")
            .await?
            .ok_or_else(|| DbError::Check("Project Copilot run has no user message".into()))?;
        tx.commit().await?;
        Ok(Some(ProjectCopilotRunAdmission {
            run,
            user_message,
            created: false,
        }))
    }

    async fn get_copilot_run(
        &self,
        project_id: &str,
        run_id: &str,
    ) -> crate::Result<Option<ProjectCopilotRun>> {
        sqlx::query("SELECT * FROM project_copilot_run WHERE project_id = ? AND id = ?")
            .bind(project_id)
            .bind(run_id)
            .fetch_optional(self.pool())
            .await?
            .map(map_run)
            .transpose()
    }

    async fn get_copilot_run_by_id(
        &self,
        run_id: &str,
    ) -> crate::Result<Option<ProjectCopilotRun>> {
        sqlx::query("SELECT * FROM project_copilot_run WHERE id = ?")
            .bind(run_id)
            .fetch_optional(self.pool())
            .await?
            .map(map_run)
            .transpose()
    }

    async fn get_active_copilot_run(
        &self,
        project_id: &str,
    ) -> crate::Result<Option<ProjectCopilotRun>> {
        sqlx::query(
            "SELECT * FROM project_copilot_run
             WHERE project_id = ? AND status IN ('queued', 'running')",
        )
        .bind(project_id)
        .fetch_optional(self.pool())
        .await?
        .map(map_run)
        .transpose()
    }

    async fn get_copilot_message(
        &self,
        run_id: &str,
        role: &str,
    ) -> crate::Result<Option<ProjectCopilotMessage>> {
        sqlx::query("SELECT * FROM project_copilot_message WHERE run_id = ? AND role = ?")
            .bind(run_id)
            .bind(role)
            .fetch_optional(self.pool())
            .await?
            .map(map_message)
            .transpose()
    }

    async fn list_copilot_messages(
        &self,
        project_id: &str,
        query: CopilotMessagePageQuery,
    ) -> crate::Result<Page<ProjectCopilotMessage>> {
        let cursor = decode_message_cursor(query.cursor.as_deref())?;
        let limit = query.limit.clamp(1, 100);
        let rows = if let Some(cursor) = cursor {
            sqlx::query(
                "SELECT * FROM project_copilot_message
                 WHERE project_id = ?
                   AND (created_at < ? OR (created_at = ? AND id < ?))
                 ORDER BY created_at DESC, id DESC LIMIT ?",
            )
            .bind(project_id)
            .bind(&cursor.created_at)
            .bind(&cursor.created_at)
            .bind(&cursor.id)
            .bind(limit + 1)
            .fetch_all(self.pool())
            .await?
        } else {
            sqlx::query(
                "SELECT * FROM project_copilot_message
                 WHERE project_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
            )
            .bind(project_id)
            .bind(limit + 1)
            .fetch_all(self.pool())
            .await?
        };
        let mut items = rows
            .into_iter()
            .map(map_message)
            .collect::<crate::Result<Vec<_>>>()?;
        let has_more = items.len() > limit as usize;
        if has_more {
            items.pop();
        }
        let next_cursor = if has_more {
            items
                .last()
                .map(|message| encode_message_cursor(&message.created_at, &message.id))
                .transpose()?
        } else {
            None
        };
        // API pages render chronologically, while the cursor walks toward
        // older history. The initial page always includes the latest turn.
        items.reverse();
        Ok(Page {
            items,
            next_cursor,
            total_count: None,
        })
    }

    async fn list_copilot_prompt_messages(
        &self,
        project_id: &str,
        max_messages: i64,
    ) -> crate::Result<Vec<ProjectCopilotMessage>> {
        let mut messages = sqlx::query(
            "SELECT * FROM project_copilot_message
             WHERE project_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(project_id)
        .bind(max_messages.max(1))
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(map_message)
        .collect::<crate::Result<Vec<_>>>()?;
        messages.reverse();
        Ok(messages)
    }

    async fn claim_copilot_run(
        &self,
        run_id: &str,
        token_hash: &str,
        token_expires_at: &str,
        now: &str,
    ) -> crate::Result<Option<ProjectCopilotRun>> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let Some(run) = get_run_tx(&mut tx, run_id).await? else {
            return Err(DbError::NotFound);
        };
        if run.status != ProjectCopilotRunStatus::Queued {
            tx.rollback().await?;
            return Ok(None);
        }
        let authority = copilot_run_authority_valid(&mut tx, run_id, "queued").await?;
        let safe_failure = if authority.is_none() {
            Some((
                "authorization_revoked",
                "Project Copilot authority changed before the harness started.",
            ))
        } else if authority
            .as_ref()
            .is_some_and(|row| row.try_get::<bool, _>("agent_paused").unwrap_or(true))
        {
            Some((
                "agent_paused",
                "The assigned Project Copilot Agent is paused.",
            ))
        } else {
            None
        };
        if let Some((failure_code, failure_message)) = safe_failure {
            sqlx::query(
                "UPDATE project_copilot_run
                 SET status = 'failed', failure_code = ?, failure_message = ?,
                     completed_at = ?, updated_at = ?, mcp_token_hash = NULL,
                     mcp_token_expires_at = NULL
                 WHERE id = ? AND status = 'queued'",
            )
            .bind(failure_code)
            .bind(failure_message)
            .bind(now)
            .bind(now)
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
            let failed = get_run_tx(&mut tx, run_id).await?;
            tx.commit().await?;
            return Ok(failed);
        }
        sqlx::query(
            "UPDATE project_copilot_run
             SET status = 'running', mcp_token_hash = ?, mcp_token_expires_at = ?,
                 started_at = ?, updated_at = ?
             WHERE id = ? AND status = 'queued'",
        )
        .bind(token_hash)
        .bind(token_expires_at)
        .bind(now)
        .bind(now)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        let claimed = get_run_tx(&mut tx, run_id).await?;
        tx.commit().await?;
        Ok(claimed)
    }

    async fn finish_copilot_run(
        &self,
        run_id: &str,
        status: ProjectCopilotRunStatus,
        assistant_message: Option<CreateProjectCopilotMessage>,
        failure_code: Option<&str>,
        failure_message: Option<&str>,
        now: &str,
    ) -> crate::Result<Option<ProjectCopilotRun>> {
        let mut status = status;
        let mut assistant_message = assistant_message;
        let mut failure_code = failure_code;
        let mut failure_message = failure_message;
        if !matches!(
            status,
            ProjectCopilotRunStatus::Succeeded
                | ProjectCopilotRunStatus::Failed
                | ProjectCopilotRunStatus::Cancelled
                | ProjectCopilotRunStatus::Interrupted
        ) {
            return Err(DbError::Check(
                "Project Copilot completion requires a terminal status".to_owned(),
            ));
        }
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let Some(run) = get_run_tx(&mut tx, run_id).await? else {
            return Ok(None);
        };
        if !matches!(
            run.status,
            ProjectCopilotRunStatus::Queued | ProjectCopilotRunStatus::Running
        ) {
            tx.rollback().await?;
            return Ok(Some(run));
        }
        if status == ProjectCopilotRunStatus::Succeeded
            && copilot_run_authority_valid(&mut tx, run_id, "running")
                .await?
                .is_none()
        {
            status = ProjectCopilotRunStatus::Failed;
            assistant_message = None;
            failure_code = Some("authorization_revoked");
            failure_message = Some("Project Copilot authority changed during the turn. Inspect its activity before delegating again.");
        }
        if let Some(message) = assistant_message.as_ref() {
            if status != ProjectCopilotRunStatus::Succeeded
                || message.run_id != run.id
                || message.project_id != run.project_id
                || message.role != "assistant"
                || message.actor_type != "agent"
                || message.actor_id != run.agent_id
            {
                return Err(DbError::Check(
                    "Project Copilot assistant message does not match run identity".to_owned(),
                ));
            }
            sqlx::query(
                "INSERT INTO project_copilot_message (
                    id, project_id, run_id, role, actor_type, actor_id, content, created_at
                 ) VALUES (?, ?, ?, 'assistant', 'agent', ?, ?, ?)",
            )
            .bind(&message.id)
            .bind(&message.project_id)
            .bind(&message.run_id)
            .bind(&message.actor_id)
            .bind(&message.content)
            .bind(&message.created_at)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE project_copilot_run
             SET status = ?, failure_code = ?, failure_message = ?,
                 completed_at = ?, updated_at = ?, mcp_token_hash = NULL,
                 mcp_token_expires_at = NULL
             WHERE id = ? AND status IN ('queued', 'running')",
        )
        .bind(status.to_string())
        .bind(failure_code)
        .bind(failure_message)
        .bind(now)
        .bind(now)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        let completed = get_run_tx(&mut tx, run_id).await?;
        tx.commit().await?;
        Ok(completed)
    }

    async fn cancel_copilot_run(
        &self,
        project_id: &str,
        run_id: &str,
        caller_user_id: &str,
        now: &str,
    ) -> crate::Result<Option<ProjectCopilotRun>> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        require_project_writer(&mut tx, project_id, caller_user_id).await?;
        let Some(run) = get_run_tx(&mut tx, run_id).await? else {
            return Ok(None);
        };
        if run.project_id != project_id {
            return Ok(None);
        }
        if matches!(
            run.status,
            ProjectCopilotRunStatus::Queued | ProjectCopilotRunStatus::Running
        ) {
            sqlx::query(
                "UPDATE project_copilot_run
                 SET status = 'cancelled', failure_code = 'cancelled_by_user',
                     failure_message = 'Project Copilot run was cancelled.',
                     completed_at = ?, updated_at = ?, mcp_token_hash = NULL,
                     mcp_token_expires_at = NULL
                 WHERE id = ? AND status IN ('queued', 'running')",
            )
            .bind(now)
            .bind(now)
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
        }
        let run = get_run_tx(&mut tx, run_id).await?;
        tx.commit().await?;
        Ok(run)
    }

    async fn recover_active_copilot_runs(&self, now: &str) -> crate::Result<i64> {
        let result = sqlx::query(
            "UPDATE project_copilot_run
             SET status = 'interrupted', failure_code = 'server_restarted',
                 failure_message = 'The server restarted before this Project Copilot turn completed.',
                 completed_at = ?, updated_at = ?, mcp_token_hash = NULL,
                 mcp_token_expires_at = NULL
             WHERE status IN ('queued', 'running')",
        )
        .bind(now)
        .bind(now)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() as i64)
    }

    async fn get_copilot_run_id_by_token_hash(
        &self,
        token_hash: &str,
    ) -> crate::Result<Option<String>> {
        sqlx::query_scalar(
            "SELECT id FROM project_copilot_run
             WHERE mcp_token_hash = ? AND status = 'running'
               AND mcp_token_expires_at IS NOT NULL
               AND julianday(mcp_token_expires_at) > julianday('now')",
        )
        .bind(token_hash)
        .fetch_optional(self.pool())
        .await
        .map_err(Into::into)
    }

    async fn authorize_run(&self, run_id: &str) -> crate::Result<ProjectCopilotRunGrant> {
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let row = copilot_run_authority_valid(&mut tx, run_id, "running")
            .await?
            .ok_or(DbError::ProjectCopilotRunAuthority)?;
        let grant = ProjectCopilotRunGrant {
            run_id: row.try_get("id")?,
            project_id: row.try_get("project_id")?,
            agent_id: row.try_get("agent_id")?,
            user_id: row.try_get("user_id")?,
            binding_version: row.try_get("binding_version")?,
            context_task_id: row.try_get("context_task_id")?,
        };
        tx.commit().await?;
        Ok(grant)
    }

    async fn authorize_run_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        grant: &ProjectCopilotRunGrant,
        scope_type: &str,
        scope_id: &str,
        actor_type: &str,
        actor_id: &str,
    ) -> crate::Result<()> {
        let scope_valid = match scope_type {
            "project" => scope_id == grant.project_id,
            "task" => {
                if grant
                    .context_task_id
                    .as_deref()
                    .is_some_and(|id| id != scope_id)
                {
                    return Err(DbError::ProjectCopilotRunAuthority);
                }
                sqlx::query_scalar::<_, i64>(
                    "SELECT EXISTS(SELECT 1 FROM task
                     WHERE id = ? AND project_id = ? AND deleted_at IS NULL)",
                )
                .bind(scope_id)
                .bind(&grant.project_id)
                .fetch_one(&mut **transaction)
                .await?
                    != 0
            }
            _ => false,
        };
        if !scope_valid || actor_type != "agent" || actor_id != grant.agent_id {
            return Err(DbError::ProjectCopilotRunAuthority);
        }
        let authority = copilot_run_authority_valid(transaction, &grant.run_id, "running").await?;
        let valid = authority.is_some_and(|row| {
            row.try_get::<String, _>("project_id").ok().as_deref()
                == Some(grant.project_id.as_str())
                && row.try_get::<String, _>("agent_id").ok().as_deref()
                    == Some(grant.agent_id.as_str())
                && row.try_get::<String, _>("user_id").ok().as_deref()
                    == Some(grant.user_id.as_str())
                && row.try_get::<i64, _>("binding_version").ok() == Some(grant.binding_version)
                && row.try_get::<Option<String>, _>("context_task_id").ok()
                    == Some(grant.context_task_id.clone())
        });
        if valid {
            Ok(())
        } else {
            Err(DbError::ProjectCopilotRunAuthority)
        }
    }
}
