use super::*;

#[async_trait]
impl TaskDependencyRepo for SqliteDb {
    async fn add_dependency(&self, task_id: &str, depends_on_id: &str, now: &str) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if task_id == depends_on_id {
            return Err(DbError::CycleDetected);
        }

        let cycle_count = sqlx::query_scalar::<_, i64>(
            "WITH RECURSIVE reachable(id) AS (
                SELECT depends_on_id FROM task_dependency WHERE task_id = ?
                UNION
                SELECT td.depends_on_id FROM task_dependency td JOIN reachable r ON td.task_id = r.id
            )
            SELECT COUNT(*) FROM reachable WHERE id = ?",
        )
        .bind(depends_on_id)
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;
        if cycle_count > 0 {
            return Err(DbError::CycleDetected);
        }

        sqlx::query(
            "INSERT INTO task_dependency (task_id, depends_on_id, created_at) VALUES (?, ?, ?)",
        )
        .bind(task_id)
        .bind(depends_on_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(project_dependency_error)?;
        tx.commit().await?;
        Ok(())
    }

    async fn remove_dependency(&self, task_id: &str, depends_on_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM task_dependency WHERE task_id = ? AND depends_on_id = ?")
            .bind(task_id)
            .bind(depends_on_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_dependencies(&self, task_id: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT depends_on_id FROM task_dependency WHERE task_id = ?",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn list_dependents(&self, depends_on_id: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT task_id FROM task_dependency WHERE depends_on_id = ?",
        )
        .bind(depends_on_id)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn unsatisfied_dependencies(&self, task_id: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT depends_on_id FROM project_task_dependency_readiness WHERE task_id = ? AND satisfied = 0",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?)
    }
}

fn project_dependency_error(error: sqlx::Error) -> DbError {
    if let sqlx::Error::Database(db) = &error {
        return match db.message() {
            "FORGE_PROJECT_DEPENDENCY_CYCLE" => DbError::CycleDetected,
            "FORGE_PROJECT_DEPENDENCY_INVALID" | "FORGE_PROJECT_DEPENDENCY_IMMUTABLE" => {
                DbError::Check(db.message().to_owned())
            }
            _ => error.into(),
        };
    }
    error.into()
}
