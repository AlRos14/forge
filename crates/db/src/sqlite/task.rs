use super::*;
use std::collections::HashSet;

async fn update_task_fields_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    input: UpdateTask,
    parent_structure: Option<(Option<String>, Option<i64>)>,
) -> Result<Task> {
    let row = sqlx::query(&format!("SELECT {TASK_COLUMNS} FROM task WHERE id = ?"))
        .bind(&input.id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(DbError::NotFound)?;
    let mut task = map_task(row)?;
    if task.deleted_at.is_some() {
        return Err(DbError::InvalidSoftDelete);
    }
    if task.version != input.expected_version {
        return Err(DbError::VersionConflict);
    }

    if let Some(title) = input.title {
        task.title = title;
    }
    if let Some(description) = input.description {
        task.description = description;
    }
    if let Some(priority) = input.priority {
        task.priority = priority;
    }
    if let Some(merge_config) = input.merge_config {
        task.merge_config = merge_config;
    }
    if let Some(error_annotation) = input.error_annotation {
        task.error_annotation = error_annotation;
    }
    let blocked_json_update = input.blocked_json;
    let failed_json_update = input.failed_json;
    let set_blocked_json = blocked_json_update.is_some();
    let set_failed_json = failed_json_update.is_some();
    let clear_failed_json = matches!(blocked_json_update, Some(Some(_)));
    let clear_blocked_json = matches!(failed_json_update, Some(Some(_)));
    if let Some(blocked_json) = blocked_json_update {
        task.blocked_json = blocked_json;
        if task.blocked_json.is_some() {
            task.failed_json = None;
        }
    }
    if let Some(failed_json) = failed_json_update {
        task.failed_json = failed_json;
        if task.failed_json.is_some() {
            task.blocked_json = None;
        }
    }
    let has_parent_structure = parent_structure.is_some();
    if let Some((parent_task_id, subtask_order)) = parent_structure {
        task.parent_task_id = parent_task_id;
        task.subtask_order = subtask_order;
    }
    task.updated_at = input.updated_at;
    task.version += 1;

    let mut query = sqlx::QueryBuilder::<Sqlite>::new("UPDATE task SET title = ");
    query
        .push_bind(&task.title)
        .push(", description = ")
        .push_bind(task.description.as_deref())
        .push(", priority = ")
        .push_bind(task.priority)
        .push(", merge_config = ")
        .push_bind(task.merge_config.as_deref())
        .push(", error_annotation = ")
        .push_bind(task.error_annotation.as_deref());
    if set_blocked_json || clear_blocked_json {
        query
            .push(", blocked_json = ")
            .push_bind(task.blocked_json.as_deref());
    }
    if set_failed_json || clear_failed_json {
        query
            .push(", failed_json = ")
            .push_bind(task.failed_json.as_deref());
    }
    if has_parent_structure {
        query
            .push(", parent_task_id = ")
            .push_bind(task.parent_task_id.as_deref())
            .push(", subtask_order = ")
            .push_bind(task.subtask_order);
    }
    query
        .push(", version = version + 1, updated_at = ")
        .push_bind(&task.updated_at)
        .push(" WHERE id = ")
        .push_bind(&task.id)
        .push(" AND version = ")
        .push_bind(input.expected_version)
        .push(" AND deleted_at IS NULL");
    let result = query.build().execute(&mut **transaction).await?;
    if result.rows_affected() == 0 {
        return Err(DbError::VersionConflict);
    }
    Ok(task)
}

#[async_trait]
impl TaskRepo for SqliteDb {
    async fn create(&self, input: CreateTask) -> Result<Task> {
        let mut transaction = self.pool.begin().await?;
        let task = TaskRepo::create_in_tx(self, &mut transaction, input).await?;
        transaction.commit().await?;
        Ok(task)
    }

    async fn create_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: CreateTask,
    ) -> Result<Task> {
        sqlx::query("INSERT INTO task (id, project_id, repo_id, parent_task_id, title, description, task_type, is_automation, priority, board_position, subtask_order, merge_config, metadata_json, created_at, updated_at) SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, COALESCE(MAX(board_position), 0.0) + 1.0, ?, ?, ?, ?, ? FROM task WHERE project_id = ?")
            .bind(&input.id)
            .bind(&input.project_id)
            .bind(input.repo_id.as_deref())
            .bind(input.parent_task_id.as_deref())
            .bind(&input.title)
            .bind(input.description.as_deref())
            .bind(&input.task_type)
            .bind(if input.is_automation { 1 } else { 0 })
            .bind(input.priority)
            .bind(input.subtask_order)
            .bind(input.merge_config.as_deref())
            .bind(Option::<&str>::None)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .bind(&input.project_id)
            .execute(&mut **transaction)
            .await?;
        sqlx::query(
            "INSERT INTO task_lifecycle (task_id, state, version, reason_kind, reason_ref, created_at, updated_at) VALUES (?, ?, 1, NULL, NULL, ?, ?)",
        )
        .bind(&input.id)
        .bind(input.lifecycle_state.to_string())
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .execute(&mut **transaction)
        .await?;
        let row = sqlx::query(&format!("SELECT {TASK_COLUMNS} FROM task WHERE id = ?"))
            .bind(&input.id)
            .fetch_one(&mut **transaction)
            .await?;
        map_task(row)
    }

    async fn get_by_id(&self, id: &str, include_deleted: bool) -> Result<Option<Task>> {
        let sql = if include_deleted {
            format!("SELECT {TASK_COLUMNS} FROM task WHERE id = ?")
        } else {
            format!("SELECT {TASK_COLUMNS} FROM task WHERE id = ? AND deleted_at IS NULL")
        };
        sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task)
            .transpose()
    }

    async fn get_by_id_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        id: &str,
        include_deleted: bool,
    ) -> Result<Option<Task>> {
        let sql = if include_deleted {
            format!("SELECT {TASK_COLUMNS} FROM task WHERE id = ?")
        } else {
            format!("SELECT {TASK_COLUMNS} FROM task WHERE id = ? AND deleted_at IS NULL")
        };
        sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .map(map_task)
            .transpose()
    }

    async fn list(&self, query: TaskListQuery) -> Result<Page<Task>> {
        let offset = decode_offset(&query.page.cursor)?;
        let mut where_parts = vec!["project_id = ?"];
        if !query.include_deleted {
            where_parts.push("deleted_at IS NULL");
        }
        let explicitly_includes_cancelled = query
            .lifecycle_states
            .contains(&crate::TaskLifecycleState::Cancelled);
        if !query.include_cancelled && !explicitly_includes_cancelled {
            where_parts
                .push("id IN (SELECT task_id FROM task_lifecycle WHERE state != 'cancelled')");
        }
        if !query.lifecycle_states.is_empty() {
            where_parts.push(
                "id IN (SELECT task_id FROM task_lifecycle WHERE state IN (__LIFECYCLE_STATES__))",
            );
        }
        if !query.agent_ids.is_empty() {
            where_parts.push(
                "id IN (
                SELECT task_role.task_id
                FROM task_role
                JOIN role_membership ON role_membership.task_role_id = task_role.id
                WHERE role_membership.actor_kind = 'agent'
                  AND role_membership.status = 'active'
                  AND role_membership.actor_id IN (__AGENTS__)
            )",
            );
        }
        if !query.assignee_types.is_empty() || !query.assignee_ids.is_empty() {
            where_parts.push(
                "id IN (
                SELECT task_role.task_id
                FROM task_role
                JOIN role_membership ON role_membership.task_role_id = task_role.id
                WHERE (__ASSIGNEE_TYPE_FILTER_MEMBERSHIP__)
                  AND (__ASSIGNEE_ID_FILTER_MEMBERSHIP__)
                  AND role_membership.status IN ('active', 'suspended')
            )",
            );
        }
        if query.priority.is_some() {
            where_parts.push("priority = ?");
        }
        let search_pattern = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|term| !term.is_empty())
            .map(search_like_pattern);
        if search_pattern.is_some() {
            where_parts.push("(LOWER(title) LIKE ? ESCAPE '\\' OR LOWER(COALESCE(description, '')) LIKE ? ESCAPE '\\')");
        }
        let lifecycle_placeholders = vec!["?"; query.lifecycle_states.len()].join(", ");
        let agent_placeholders = vec!["?"; query.agent_ids.len()].join(", ");
        let assignee_type_placeholders = vec!["?"; query.assignee_types.len()].join(", ");
        let assignee_id_placeholders = vec!["?"; query.assignee_ids.len()].join(", ");
        let assignee_type_filter_membership = if query.assignee_types.is_empty() {
            "1 = 1".to_owned()
        } else {
            format!(
                "CASE role_membership.actor_kind WHEN 'human' THEN 'user' ELSE 'agent' END IN ({assignee_type_placeholders})"
            )
        };
        let assignee_id_filter_membership = if query.assignee_ids.is_empty() {
            "1 = 1".to_owned()
        } else {
            format!("role_membership.actor_id IN ({assignee_id_placeholders})")
        };
        let where_sql = where_parts
            .join(" AND ")
            .replace("__LIFECYCLE_STATES__", &lifecycle_placeholders)
            .replace("__AGENTS__", &agent_placeholders)
            .replace(
                "__ASSIGNEE_TYPE_FILTER_MEMBERSHIP__",
                &assignee_type_filter_membership,
            )
            .replace(
                "__ASSIGNEE_ID_FILTER_MEMBERSHIP__",
                &assignee_id_filter_membership,
            );
        let sql = format!(
            "SELECT {TASK_COLUMNS} FROM task WHERE {} ORDER BY {} LIMIT ? OFFSET ?",
            where_sql,
            order_clause(&query.page)
        );
        let mut q = sqlx::query(&sql).bind(&query.project_id);
        for state in &query.lifecycle_states {
            q = q.bind(state.to_string());
        }
        for agent_id in &query.agent_ids {
            q = q.bind(agent_id);
        }
        for assignee_type in &query.assignee_types {
            q = q.bind(assignee_type);
        }
        for assignee_id in &query.assignee_ids {
            q = q.bind(assignee_id);
        }
        if let Some(priority) = query.priority {
            q = q.bind(priority);
        }
        if let Some(search_pattern) = search_pattern.as_ref() {
            q = q.bind(search_pattern).bind(search_pattern);
        }
        let rows = q
            .bind(limit(&query.page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows.into_iter().map(map_task).collect::<Result<Vec<_>>>()?;
        let total = if query.page.include_total {
            let count_sql = format!("SELECT COUNT(*) FROM task WHERE {}", where_sql);
            let mut q = sqlx::query_scalar::<_, i64>(&count_sql).bind(&query.project_id);
            for state in &query.lifecycle_states {
                q = q.bind(state.to_string());
            }
            for agent_id in &query.agent_ids {
                q = q.bind(agent_id);
            }
            for assignee_type in &query.assignee_types {
                q = q.bind(assignee_type);
            }
            for assignee_id in &query.assignee_ids {
                q = q.bind(assignee_id);
            }
            if let Some(priority) = query.priority {
                q = q.bind(priority);
            }
            if let Some(search_pattern) = search_pattern.as_ref() {
                q = q.bind(search_pattern).bind(search_pattern);
            }
            Some(q.fetch_one(&self.pool).await?)
        } else {
            None
        };
        page_from_items(items, &query.page, offset, total)
    }

    async fn list_by_executing_agent(&self, query: AgentTaskListQuery) -> Result<Page<Task>> {
        let offset = decode_offset(&query.page.cursor)?;
        let mut where_parts =
            vec!["id IN (SELECT DISTINCT task_id FROM execution WHERE agent_id = ?)".to_owned()];
        if !query.include_deleted {
            where_parts.push("deleted_at IS NULL".to_owned());
        }
        if !query.include_cancelled {
            where_parts.push(
                "id IN (SELECT task_id FROM task_lifecycle WHERE state != 'cancelled')".to_owned(),
            );
        }
        let where_sql = where_parts.join(" AND ");
        let sql = format!(
            "SELECT {TASK_COLUMNS} FROM task WHERE {} ORDER BY {} LIMIT ? OFFSET ?",
            where_sql,
            order_clause(&query.page)
        );
        let rows = sqlx::query(&sql)
            .bind(&query.agent_id)
            .bind(limit(&query.page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows.into_iter().map(map_task).collect::<Result<Vec<_>>>()?;
        let total = if query.page.include_total {
            let count_sql = format!("SELECT COUNT(*) FROM task WHERE {where_sql}");
            Some(
                sqlx::query_scalar::<_, i64>(&count_sql)
                    .bind(&query.agent_id)
                    .fetch_one(&self.pool)
                    .await?,
            )
        } else {
            None
        };
        page_from_items(items, &query.page, offset, total)
    }

    async fn list_subtasks_ordered(&self, parent_task_id: &str) -> Result<Vec<Task>> {
        let sql = format!(
            "SELECT {TASK_COLUMNS} FROM task WHERE parent_task_id = ? AND deleted_at IS NULL ORDER BY subtask_order ASC, created_at ASC, id ASC"
        );
        let rows = sqlx::query(&sql)
            .bind(parent_task_id)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(map_task).collect()
    }

    async fn next_subtask_order(&self, parent_task_id: &str) -> Result<i64> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(subtask_order) + 1, 0) FROM task WHERE parent_task_id = ? AND deleted_at IS NULL",
        )
        .bind(parent_task_id)
        .fetch_one(&self.pool)
        .await?)
    }

    async fn next_subtask_order_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        parent_task_id: &str,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(subtask_order) + 1, 0) FROM task WHERE parent_task_id = ? AND deleted_at IS NULL",
        )
        .bind(parent_task_id)
        .fetch_one(&mut **transaction)
        .await?)
    }

    async fn count_live_children_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        task_id: &str,
    ) -> Result<i64> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM task WHERE parent_task_id = ? AND deleted_at IS NULL",
        )
        .bind(task_id)
        .fetch_one(&mut **transaction)
        .await?)
    }

    async fn reorder_subtasks(
        &self,
        parent_task_id: &str,
        ordered_ids: &[String],
        updated_at: &str,
    ) -> Result<()> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if ordered_ids.iter().collect::<HashSet<_>>().len() != ordered_ids.len() {
            return Err(DbError::InvalidTransition);
        }

        for task_id in ordered_ids {
            let belongs_to_parent = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM task WHERE id = ? AND parent_task_id = ? AND deleted_at IS NULL",
            )
            .bind(task_id)
            .bind(parent_task_id)
            .fetch_one(&mut *transaction)
            .await?;
            if belongs_to_parent == 0 {
                return Err(DbError::NotFound);
            }
        }

        let sibling_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM task WHERE parent_task_id = ? AND deleted_at IS NULL",
        )
        .bind(parent_task_id)
        .fetch_one(&mut *transaction)
        .await?;
        if sibling_count != ordered_ids.len() as i64 {
            return Err(DbError::InvalidTransition);
        }

        for (subtask_order, task_id) in ordered_ids.iter().enumerate() {
            let result =
                sqlx::query("UPDATE task SET subtask_order = ?, updated_at = ? WHERE id = ?")
                    .bind(subtask_order as i64)
                    .bind(updated_at)
                    .bind(task_id)
                    .execute(&mut *transaction)
                    .await?;
            if result.rows_affected() == 0 {
                return Err(DbError::NotFound);
            }
        }

        transaction.commit().await?;
        Ok(())
    }

    async fn update(&self, input: UpdateTask) -> Result<Task> {
        if input.parent_task_id.is_some() {
            return Err(DbError::InvalidTransition);
        }
        let mut transaction = self.pool.begin().await?;
        let task = update_task_fields_in_tx(&mut transaction, input, None).await?;
        transaction.commit().await?;
        Ok(task)
    }

    async fn update_parent_structure_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: UpdateTask,
        subtask_order: Option<i64>,
    ) -> Result<Task> {
        let parent_task_id = input
            .parent_task_id
            .clone()
            .ok_or(DbError::InvalidTransition)?;
        update_task_fields_in_tx(transaction, input, Some((parent_task_id, subtask_order))).await
    }

    async fn soft_delete_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: crate::SoftDeleteTask,
    ) -> Result<Task> {
        let mut task = TaskRepo::get_by_id_in_tx(self, transaction, &input.id, true)
            .await?
            .ok_or(DbError::NotFound)?;
        if task.deleted_at.is_some()
            || matches!(
                task.lifecycle_state,
                TaskLifecycleState::Active
                    | TaskLifecycleState::ReadyToMerge
                    | TaskLifecycleState::Merging
            )
        {
            return Err(DbError::InvalidSoftDelete);
        }
        if task.version != input.expected_version {
            return Err(DbError::VersionConflict);
        }
        task.deleted_at = Some(input.deleted_at);
        task.updated_at = input.updated_at;
        task.version += 1;
        let result = sqlx::query("UPDATE task SET deleted_at = ?, version = version + 1, updated_at = ? WHERE id = ? AND version = ? AND deleted_at IS NULL")
            .bind(task.deleted_at.as_deref())
            .bind(&task.updated_at)
            .bind(&task.id)
            .bind(input.expected_version)
            .execute(&mut **transaction)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }
        Ok(task)
    }

    async fn set_metadata_json(
        &self,
        id: &str,
        metadata_json: Option<String>,
        updated_at: &str,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE task SET metadata_json = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(metadata_json.as_deref())
        .bind(updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn set_metadata_json_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        id: &str,
        metadata_json: Option<String>,
        updated_at: &str,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE task SET metadata_json = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(metadata_json.as_deref())
        .bind(updated_at)
        .bind(id)
        .execute(&mut **transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}

fn search_like_pattern(term: &str) -> String {
    let mut pattern = String::with_capacity(term.len() + 2);
    pattern.push('%');
    for ch in term.to_lowercase().chars() {
        if matches!(ch, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

#[async_trait]
impl crate::TaskBoardRepo for SqliteDb {
    async fn board_revision(&self, project_id: &str) -> Result<i64> {
        sqlx::query_scalar("SELECT board_revision FROM project WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(DbError::NotFound)
    }
}
