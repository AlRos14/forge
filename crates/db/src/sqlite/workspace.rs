use super::*;

#[async_trait]
impl WorkspaceRepo for SqliteDb {
    async fn create(&self, input: CreateWorkspace) -> Result<Workspace> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO workspace (id, task_id, repo_id, worktree_path, branch, status, before_sha, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&input.id)
            .bind(&input.task_id)
            .bind(&input.repo_id)
            .bind(&input.worktree_path)
            .bind(&input.branch)
            .bind(input.status.to_string())
            .bind(input.before_sha.as_deref())
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO workspace_scope (workspace_id, task_id, scope_kind, work_unit_id, created_at)
             VALUES (?, ?, 'integration', NULL, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query("SELECT * FROM workspace WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await?;
        let workspace = map_workspace(row)?;
        tx.commit().await?;
        Ok(workspace)
    }

    async fn get_task_id(&self, id: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT task_id FROM workspace WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Workspace>> {
        sqlx::query("SELECT * FROM workspace WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_workspace)
            .transpose()
    }

    async fn get_by_task_id(&self, task_id: &str) -> Result<Option<Workspace>> {
        sqlx::query(
            "SELECT w.* FROM workspace w
             JOIN workspace_scope s ON s.workspace_id = w.id AND s.task_id = w.task_id
             WHERE s.task_id = ? AND s.scope_kind = 'integration'",
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await?
        .map(map_workspace)
        .transpose()
    }

    async fn set_cleanup_after(
        &self,
        id: &str,
        cleanup_after: Option<String>,
        updated_at: &str,
    ) -> Result<Workspace> {
        let result =
            sqlx::query("UPDATE workspace SET cleanup_after = ?, updated_at = ? WHERE id = ?")
                .bind(cleanup_after.as_deref())
                .bind(updated_at)
                .bind(id)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        WorkspaceRepo::get_by_id(self, id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn claim_work_unit_cleanup(
        &self,
        id: &str,
        task_id: &str,
        work_unit_id: &str,
        updated_at: &str,
    ) -> Result<Option<Workspace>> {
        let result = sqlx::query(
            "UPDATE workspace
             SET status = 'cleaning', cleanup_after = NULL, error = NULL, updated_at = ?
             WHERE id = ? AND task_id = ? AND status = 'ready'
               AND EXISTS (
                   SELECT 1 FROM workspace_scope ws
                   JOIN work_unit wu ON wu.id = ws.work_unit_id AND wu.task_id = ws.task_id
                   WHERE ws.workspace_id = workspace.id AND ws.task_id = workspace.task_id
                     AND ws.scope_kind = 'work_unit' AND ws.work_unit_id = ?
               )
               AND NOT EXISTS (
                   SELECT 1 FROM execution e
                   WHERE e.workspace_id = workspace.id AND e.status = 'running'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM workspace_lease wl
                   WHERE wl.workspace_id = workspace.id AND wl.status = 'active'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM task_integration_operation op
                   WHERE op.task_id = workspace.task_id AND op.status = 'running'
               )",
        )
        .bind(updated_at)
        .bind(id)
        .bind(task_id)
        .bind(work_unit_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Ok(None);
        }
        WorkspaceRepo::get_by_id(self, id).await
    }

    async fn finish_work_unit_cleanup(
        &self,
        id: &str,
        task_id: &str,
        work_unit_id: &str,
        updated_at: &str,
    ) -> Result<Workspace> {
        let result = sqlx::query(
            "UPDATE workspace
             SET status = 'cleaned', cleanup_after = NULL, error = NULL, updated_at = ?
             WHERE id = ? AND task_id = ? AND status = 'cleaning'
               AND EXISTS (
                   SELECT 1 FROM workspace_scope ws
                   JOIN work_unit wu ON wu.id = ws.work_unit_id AND wu.task_id = ws.task_id
                   WHERE ws.workspace_id = workspace.id AND ws.task_id = workspace.task_id
                     AND ws.scope_kind = 'work_unit' AND ws.work_unit_id = ?
               )
               AND NOT EXISTS (
                   SELECT 1 FROM execution e
                   WHERE e.workspace_id = workspace.id AND e.status = 'running'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM workspace_lease wl
                   WHERE wl.workspace_id = workspace.id AND wl.status = 'active'
               )",
        )
        .bind(updated_at)
        .bind(id)
        .bind(task_id)
        .bind(work_unit_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        WorkspaceRepo::get_by_id(self, id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn mark_cleaned(&self, id: &str, updated_at: &str) -> Result<Workspace> {
        let result = sqlx::query(
            "UPDATE workspace SET status = 'cleaned', cleanup_after = NULL, error = NULL, updated_at = ?
             WHERE id = ? AND EXISTS (
                 SELECT 1 FROM workspace_scope ws
                 WHERE ws.workspace_id = workspace.id AND ws.task_id = workspace.task_id
                   AND ws.scope_kind = 'integration'
             )",
        )
        .bind(updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        WorkspaceRepo::get_by_id(self, id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn list_pending_cleanup(&self, now: &str) -> Result<Vec<Workspace>> {
        let rows = sqlx::query(
            "SELECT * FROM workspace
             WHERE (status = 'cleaning' AND error IS NULL AND EXISTS (
                       SELECT 1 FROM workspace_scope ws
                       WHERE ws.workspace_id = workspace.id
                         AND ws.task_id = workspace.task_id
                         AND ws.scope_kind = 'work_unit'
                   ))
                OR (cleanup_after IS NOT NULL AND cleanup_after <= ?
                    AND status NOT IN ('cleaned', 'cleaning'))
             ORDER BY COALESCE(cleanup_after, updated_at) ASC, id ASC",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_workspace).collect()
    }

    async fn update_status(
        &self,
        id: &str,
        status: WorkspaceStatus,
        error: Option<String>,
        updated_at: &str,
    ) -> Result<Workspace> {
        let result =
            sqlx::query("UPDATE workspace SET status = ?, error = ?, updated_at = ? WHERE id = ?")
                .bind(status.to_string())
                .bind(error.as_deref())
                .bind(updated_at)
                .bind(id)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        WorkspaceRepo::get_by_id(self, id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM workspace WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}
