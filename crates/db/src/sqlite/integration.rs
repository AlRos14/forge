use super::*;

fn map_integration(row: &SqliteRow) -> Result<ProjectIntegration> {
    Ok(ProjectIntegration {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        platform: parse_enum(row.try_get::<String, _>("platform")?)?,
        base_url: row.try_get("base_url")?,
        owner: row.try_get("owner")?,
        repo: row.try_get("repo")?,
        token_secret_ref: row.try_get("token_secret_ref")?,
        poll_interval_secs: row.try_get("poll_interval_secs")?,
        sync_filter: row.try_get("sync_filter")?,
        default_task_state: row.try_get("default_task_state")?,
        default_assignee_type: row.try_get("default_assignee_type")?,
        default_assignee_id: row.try_get("default_assignee_id")?,
        enabled: row.try_get::<i64, _>("enabled")? != 0,
        last_polled_at: row.try_get("last_polled_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[async_trait]
impl IntegrationRepo for SqliteDb {
    async fn create_integration(
        &self,
        input: CreateProjectIntegration,
    ) -> Result<ProjectIntegration> {
        sqlx::query(
            "INSERT INTO project_integration (id, project_id, platform, base_url, owner, repo, token_secret_ref, poll_interval_secs, sync_filter, default_task_state, default_assignee_type, default_assignee_id, enabled, last_polled_at, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.project_id)
        .bind(input.platform.to_string())
        .bind(&input.base_url)
        .bind(&input.owner)
        .bind(&input.repo)
        .bind(&input.token_secret_ref)
        .bind(input.poll_interval_secs)
        .bind(&input.sync_filter)
        .bind(input.default_task_state.as_deref())
        .bind(input.default_assignee_type.as_deref())
        .bind(input.default_assignee_id.as_deref())
        .bind(if input.enabled { 1 } else { 0 })
        .bind(input.last_polled_at.as_deref())
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .execute(&self.pool)
        .await
        .map_err(check_error)?;

        IntegrationRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<ProjectIntegration>> {
        sqlx::query("SELECT * FROM project_integration WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(|row| map_integration(&row))
            .transpose()
    }

    async fn get_by_project_id(&self, project_id: &str) -> Result<Option<ProjectIntegration>> {
        sqlx::query("SELECT * FROM project_integration WHERE project_id = ?")
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await?
            .map(|row| map_integration(&row))
            .transpose()
    }

    async fn update_integration(
        &self,
        input: UpdateProjectIntegration,
    ) -> Result<ProjectIntegration> {
        let mut query = sqlx::QueryBuilder::<Sqlite>::new("UPDATE project_integration SET ");
        let mut needs_comma = false;
        macro_rules! push_assignment {
            ($column:literal, $value:expr) => {{
                if needs_comma {
                    query.push(", ");
                }
                needs_comma = true;
                query.push($column).push(" = ").push_bind($value);
            }};
        }

        if let Some(project_id) = input.project_id {
            push_assignment!("project_id", project_id);
        }
        if let Some(platform) = input.platform {
            push_assignment!("platform", platform.to_string());
        }
        if let Some(base_url) = input.base_url {
            push_assignment!("base_url", base_url);
        }
        if let Some(owner) = input.owner {
            push_assignment!("owner", owner);
        }
        if let Some(repo) = input.repo {
            push_assignment!("repo", repo);
        }
        if let Some(token_secret_ref) = input.token_secret_ref {
            push_assignment!("token_secret_ref", token_secret_ref);
        }
        if let Some(poll_interval_secs) = input.poll_interval_secs {
            push_assignment!("poll_interval_secs", poll_interval_secs);
        }
        if let Some(sync_filter) = input.sync_filter {
            push_assignment!("sync_filter", sync_filter);
        }
        if let Some(default_task_state) = input.default_task_state {
            push_assignment!("default_task_state", default_task_state);
        }
        if let Some(default_assignee_type) = input.default_assignee_type {
            push_assignment!("default_assignee_type", default_assignee_type);
        }
        if let Some(default_assignee_id) = input.default_assignee_id {
            push_assignment!("default_assignee_id", default_assignee_id);
        }
        if let Some(enabled) = input.enabled {
            push_assignment!("enabled", if enabled { 1 } else { 0 });
        }
        if let Some(last_polled_at) = input.last_polled_at {
            push_assignment!("last_polled_at", last_polled_at);
        }
        if needs_comma {
            query.push(", ");
        }
        query.push("updated_at = ").push_bind(&input.updated_at);
        query.push(" WHERE id = ").push_bind(&input.id);

        query
            .build()
            .execute(&self.pool)
            .await
            .map_err(check_error)?;
        IntegrationRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn update_last_polled_at(
        &self,
        id: &str,
        last_polled_at: &str,
        updated_at: &str,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE project_integration SET last_polled_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(last_polled_at)
        .bind(updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn list_enabled(&self) -> Result<Vec<ProjectIntegration>> {
        let rows = sqlx::query(
            "SELECT * FROM project_integration WHERE enabled = 1 ORDER BY created_at ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(map_integration).collect()
    }

    async fn delete_integration(&self, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM project_integration WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}
