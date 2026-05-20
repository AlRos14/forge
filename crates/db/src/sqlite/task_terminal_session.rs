use super::*;

#[async_trait]
impl TerminalSessionRepo for SqliteDb {
    async fn create_terminal_session(
        &self,
        input: CreateTerminalSession,
    ) -> Result<TerminalSession> {
        sqlx::query(
            "INSERT INTO task_terminal_session (id, task_id, workspace_id, daemon_id, status, rows, cols, pid, exit_code, exit_signal, exit_reason, created_by_user_id, created_at, started_at, last_activity_at, ended_at, version) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, NULL, ?, ?, NULL, NULL, NULL, 1)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.workspace_id)
        .bind(input.daemon_id.as_deref())
        .bind(TerminalSessionStatus::Starting.to_string())
        .bind(input.rows)
        .bind(input.cols)
        .bind(&input.created_by_user_id)
        .bind(&input.created_at)
        .execute(&self.pool)
        .await?;

        Self::get_terminal_session(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_terminal_session(&self, id: &str) -> Result<Option<TerminalSession>> {
        sqlx::query("SELECT * FROM task_terminal_session WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_terminal_session)
            .transpose()
    }

    async fn list_terminal_sessions_for_task(
        &self,
        task_id: &str,
        include_ended: bool,
    ) -> Result<Vec<TerminalSession>> {
        let sql = if include_ended {
            "SELECT * FROM task_terminal_session WHERE task_id = ? ORDER BY created_at ASC, id ASC"
        } else {
            "SELECT * FROM task_terminal_session WHERE task_id = ? AND status IN ('starting', 'running') ORDER BY created_at ASC, id ASC"
        };

        sqlx::query(sql)
            .bind(task_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(map_terminal_session)
            .collect()
    }

    async fn list_running_terminal_sessions_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<TerminalSession>> {
        sqlx::query(
            "SELECT * FROM task_terminal_session WHERE task_id = ? AND status IN ('starting', 'running') ORDER BY created_at ASC, id ASC",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_terminal_session)
        .collect()
    }

    async fn list_running_terminal_sessions_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<TerminalSession>> {
        sqlx::query(
            "SELECT * FROM task_terminal_session WHERE created_by_user_id = ? AND status IN ('starting', 'running') ORDER BY created_at ASC, id ASC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_terminal_session)
        .collect()
    }

    async fn list_running_terminal_sessions_for_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<TerminalSession>> {
        sqlx::query(
            "SELECT * FROM task_terminal_session WHERE workspace_id = ? AND status IN ('starting', 'running') ORDER BY created_at ASC, id ASC",
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_terminal_session)
        .collect()
    }

    async fn list_all_running_terminal_sessions(&self) -> Result<Vec<TerminalSession>> {
        sqlx::query(
            "SELECT * FROM task_terminal_session WHERE status IN ('starting', 'running') ORDER BY created_at ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_terminal_session)
        .collect()
    }

    async fn update_terminal_session_status(
        &self,
        id: &str,
        expected_version: i64,
        update: UpdateTerminalSessionStatus,
    ) -> Result<TerminalSession> {
        let result = sqlx::query(
            "UPDATE task_terminal_session SET status = ?, started_at = ?, last_activity_at = ?, ended_at = ?, pid = ?, exit_code = ?, exit_signal = ?, exit_reason = ?, version = version + 1 WHERE id = ? AND version = ?",
        )
        .bind(update.status.to_string())
        .bind(update.started_at.as_deref())
        .bind(update.last_activity_at.as_deref())
        .bind(update.ended_at.as_deref())
        .bind(update.pid)
        .bind(update.exit_code)
        .bind(update.exit_signal.as_deref())
        .bind(update.exit_reason.as_deref())
        .bind(id)
        .bind(expected_version)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }

        Self::get_terminal_session(self, id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn update_terminal_session_size(
        &self,
        id: &str,
        rows: i64,
        cols: i64,
        last_activity_at: &str,
    ) -> Result<TerminalSession> {
        let result = sqlx::query(
            "UPDATE task_terminal_session SET rows = ?, cols = ?, last_activity_at = ? WHERE id = ?",
        )
        .bind(rows)
        .bind(cols)
        .bind(last_activity_at)
        .bind(id)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }

        Self::get_terminal_session(self, id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn touch_terminal_session_activity(
        &self,
        id: &str,
        last_activity_at: &str,
    ) -> Result<()> {
        let result =
            sqlx::query("UPDATE task_terminal_session SET last_activity_at = ? WHERE id = ?")
                .bind(last_activity_at)
                .bind(id)
                .execute(&self.pool)
                .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }

        Ok(())
    }

    async fn delete_terminal_sessions_for_workspace(&self, workspace_id: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM task_terminal_session WHERE workspace_id = ?")
            .bind(workspace_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}
