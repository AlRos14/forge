use super::*;

#[async_trait]
impl ProjectAgentLinkRepo for SqliteDb {
    async fn create(&self, input: CreateProjectAgentLink) -> Result<ProjectAgentLink> {
        sqlx::query(
            "INSERT INTO project_agent_link (id, project_id, agent_id, linked_by_user_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.project_id)
        .bind(&input.agent_id)
        .bind(&input.linked_by_user_id)
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if e.to_string().contains("UNIQUE") {
                DbError::Check("agent already linked to project".into())
            } else {
                DbError::from(e)
            }
        })?;

        Ok(ProjectAgentLink {
            id: input.id,
            project_id: input.project_id,
            agent_id: input.agent_id,
            linked_by_user_id: input.linked_by_user_id,
            created_at: input.created_at,
            updated_at: input.updated_at,
        })
    }

    async fn list_by_project(&self, project_id: &str) -> Result<Vec<ProjectAgentLink>> {
        let rows = sqlx::query(
            "SELECT * FROM project_agent_link WHERE project_id = ? ORDER BY created_at ASC",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_project_agent_link).collect()
    }

    async fn delete_by_project_and_agent(&self, project_id: &str, agent_id: &str) -> Result<()> {
        let result =
            sqlx::query("DELETE FROM project_agent_link WHERE project_id = ? AND agent_id = ?")
                .bind(project_id)
                .bind(agent_id)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn get_by_project_and_agent(
        &self,
        project_id: &str,
        agent_id: &str,
    ) -> Result<Option<ProjectAgentLink>> {
        sqlx::query("SELECT * FROM project_agent_link WHERE project_id = ? AND agent_id = ?")
            .bind(project_id)
            .bind(agent_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_project_agent_link)
            .transpose()
    }
}

fn map_project_agent_link(row: SqliteRow) -> Result<ProjectAgentLink> {
    Ok(ProjectAgentLink {
        id: row.get("id"),
        project_id: row.get("project_id"),
        agent_id: row.get("agent_id"),
        linked_by_user_id: row.get("linked_by_user_id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}
