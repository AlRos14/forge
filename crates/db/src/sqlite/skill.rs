use super::*;

#[async_trait]
impl SkillRepo for SqliteDb {
    async fn create(&self, input: CreateSkill) -> Result<Skill> {
        sqlx::query("INSERT INTO skill (id, project_id, name, content, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(&input.id)
            .bind(&input.project_id)
            .bind(&input.name)
            .bind(&input.content)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&self.pool)
            .await?;
        SkillRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Skill>> {
        sqlx::query("SELECT * FROM skill WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_skill)
            .transpose()
    }

    async fn list_by_project(&self, project_id: &str, page: PageRequest) -> Result<Page<Skill>> {
        let offset = decode_offset(&page.cursor)?;
        let sql = format!(
            "SELECT * FROM skill WHERE project_id = ? ORDER BY {} LIMIT ? OFFSET ?",
            order_clause_without_priority(&page)
        );
        let rows = sqlx::query(&sql)
            .bind(project_id)
            .bind(limit(&page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows
            .into_iter()
            .map(map_skill)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM skill WHERE project_id = ?")
                    .bind(project_id)
                    .fetch_one(&self.pool)
                    .await?,
            )
        } else {
            None
        };
        page_from_items(items, &page, offset, total)
    }

    async fn update(&self, input: UpdateSkill) -> Result<Skill> {
        let mut skill = SkillRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        if let Some(name) = input.name {
            skill.name = name;
        }
        if let Some(content) = input.content {
            skill.content = content;
        }
        skill.updated_at = input.updated_at;
        sqlx::query("UPDATE skill SET name = ?, content = ?, updated_at = ? WHERE id = ?")
            .bind(&skill.name)
            .bind(&skill.content)
            .bind(&skill.updated_at)
            .bind(&skill.id)
            .execute(&self.pool)
            .await?;
        Ok(skill)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM skill WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}
