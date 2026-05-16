use super::*;

#[async_trait]
impl RepoRepo for SqliteDb {
    async fn create(&self, input: CreateRepo) -> Result<Repo> {
        sqlx::query("INSERT INTO repo (id, project_id, name, remote_url, local_path, work_mode, default_branch, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&input.id)
            .bind(&input.project_id)
            .bind(&input.name)
            .bind(&input.remote_url)
            .bind(&input.local_path)
            .bind(input.work_mode.to_string())
            .bind(&input.default_branch)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&self.pool)
            .await
            .map_err(check_error)?;
        RepoRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Repo>> {
        sqlx::query("SELECT * FROM repo WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_repo)
            .transpose()
    }

    async fn list_by_project(&self, project_id: &str, page: PageRequest) -> Result<Page<Repo>> {
        let offset = decode_offset(&page.cursor)?;
        let sql = format!(
            "SELECT * FROM repo WHERE project_id = ? ORDER BY {} LIMIT ? OFFSET ?",
            order_clause_without_priority(&page)
        );
        let rows = sqlx::query(&sql)
            .bind(project_id)
            .bind(limit(&page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows.into_iter().map(map_repo).collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM repo WHERE project_id = ?")
                    .bind(project_id)
                    .fetch_one(&self.pool)
                    .await?,
            )
        } else {
            None
        };
        page_from_items(items, &page, offset, total)
    }

    async fn update(&self, input: UpdateRepo) -> Result<Repo> {
        let mut repo = RepoRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        if let Some(name) = input.name {
            repo.name = name;
        }
        if let Some(local_path) = input.local_path {
            repo.local_path = local_path;
        }
        if let Some(remote_url) = input.remote_url {
            repo.remote_url = remote_url;
        }
        if let Some(work_mode) = input.work_mode {
            repo.work_mode = work_mode;
        }
        if let Some(default_branch) = input.default_branch {
            repo.default_branch = default_branch;
        }
        repo.updated_at = input.updated_at;
        sqlx::query(
            "UPDATE repo SET name = ?, remote_url = ?, local_path = ?, work_mode = ?, default_branch = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&repo.name)
        .bind(&repo.remote_url)
        .bind(&repo.local_path)
        .bind(repo.work_mode.to_string())
        .bind(&repo.default_branch)
        .bind(&repo.updated_at)
        .bind(&repo.id)
        .execute(&self.pool)
        .await
        .map_err(check_error)?;
        Ok(repo)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        sqlx::query("UPDATE project SET primary_repo_id = NULL WHERE primary_repo_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        let result = sqlx::query("DELETE FROM repo WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}
