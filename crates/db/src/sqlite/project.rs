use super::*;
use crate::now_rfc3339;

#[async_trait]
impl ProjectRepo for SqliteDb {
    async fn create(&self, input: CreateProject) -> Result<Project> {
        sqlx::query("INSERT INTO project (id, name, settings, workflow_definition, workflow_template_name, primary_repo_id, owner_id, created_at, updated_at) VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?)")
            .bind(&input.id)
            .bind(&input.name)
            .bind(&input.settings)
            .bind(&input.workflow_definition)
            .bind(&input.primary_repo_id)
            .bind(&input.owner_id)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&self.pool)
            .await?;
        ProjectRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Project>> {
        sqlx::query(&format!(
            "SELECT {PROJECT_COLUMNS} FROM project WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .map(map_project)
        .transpose()
    }

    async fn list(&self, page: PageRequest) -> Result<Page<Project>> {
        let offset = decode_offset(&page.cursor)?;
        let sql = format!(
            "SELECT {PROJECT_COLUMNS} FROM project ORDER BY {} LIMIT ? OFFSET ?",
            order_clause_without_priority(&page)
        );
        let rows = sqlx::query(&sql)
            .bind(limit(&page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows
            .into_iter()
            .map(map_project)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            total_count(&self.pool, "SELECT COUNT(*) FROM project").await?
        } else {
            None
        };
        page_from_items(items, &page, offset, total)
    }

    async fn update(&self, input: UpdateProject) -> Result<Project> {
        let mut project = ProjectRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        if let Some(name) = input.name {
            project.name = name;
        }
        if let Some(settings) = input.settings {
            project.settings = settings;
        }
        if let Some(primary_repo_id) = input.primary_repo_id {
            project.primary_repo_id = primary_repo_id;
        }
        if let Some(paused_at) = input.paused_at {
            project.paused_at = paused_at;
        }
        project.updated_at = input.updated_at;
        sqlx::query(
            "UPDATE project SET name = ?, settings = ?, primary_repo_id = ?, paused_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&project.name)
        .bind(&project.settings)
        .bind(project.primary_repo_id.as_deref())
        .bind(project.paused_at.as_deref())
        .bind(&project.updated_at)
        .bind(&project.id)
        .execute(&self.pool)
        .await?;
        Ok(project)
    }

    async fn set_paused_at(&self, id: &str, paused_at: Option<String>) -> Result<()> {
        let result = sqlx::query("UPDATE project SET paused_at = ?, updated_at = ? WHERE id = ?")
            .bind(paused_at.as_deref())
            .bind(now_rfc3339())
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM project WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}
