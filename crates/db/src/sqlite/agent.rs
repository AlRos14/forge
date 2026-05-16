use super::*;
use crate::now_rfc3339;

#[async_trait]
impl AgentRepo for SqliteDb {
    async fn create(&self, input: CreateAgent) -> Result<Agent> {
        if input.is_default {
            sqlx::query("UPDATE agent SET is_default = 0 WHERE executor_type = ? AND id != ?")
                .bind(&input.executor_type)
                .bind(&input.id)
                .execute(&self.pool)
                .await?;
        }
        sqlx::query("INSERT INTO agent (id, name, description, executor_type, model, reasoning_effort, permission_policy, prompt_template, capabilities_json, config_json, daemon_id, max_concurrent_tasks, heartbeat_interval_seconds, max_missed_heartbeats, status, last_heartbeat_at, is_default, paused, owner_id, visibility, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&input.id)
            .bind(&input.name)
            .bind(input.description.as_deref())
            .bind(&input.executor_type)
            .bind(input.model.as_deref())
            .bind(input.reasoning_effort.as_deref())
            .bind(input.permission_policy.as_deref())
            .bind(input.prompt_template.as_deref())
            .bind(&input.capabilities_json)
            .bind(&input.config_json)
            .bind(input.daemon_id.as_deref())
            .bind(input.max_concurrent_tasks)
            .bind(input.heartbeat_interval_seconds)
            .bind(input.max_missed_heartbeats)
            .bind(input.status.to_string())
            .bind(input.last_heartbeat_at.as_deref())
            .bind(if input.is_default { 1 } else { 0 })
            .bind(if input.paused { 1 } else { 0 })
            .bind(input.owner_id.as_deref())
            .bind(&input.visibility)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&self.pool)
            .await?;
        AgentRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Agent>> {
        sqlx::query("SELECT * FROM agent WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_agent)
            .transpose()
    }

    async fn list(&self, query: AgentListQuery) -> Result<Page<Agent>> {
        let offset = decode_offset(&query.page.cursor)?;
        let mut where_parts = Vec::new();
        if query.status.is_some() {
            where_parts.push("agent.status = ?");
        }
        if query.executor_type.is_some() {
            where_parts.push("agent.executor_type = ?");
        }
        where_parts.extend(std::iter::repeat_n(
            "agent.capabilities_json LIKE ?",
            query.capabilities.len(),
        ));
        let where_sql = if where_parts.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", where_parts.join(" AND "))
        };
        let order_sql = match (&query.page.sort_by, &query.page.sort_order) {
            (SortBy::CreatedAt, SortOrder::Asc) => "agent.created_at ASC, agent.id ASC",
            (SortBy::CreatedAt, SortOrder::Desc) => "agent.created_at DESC, agent.id DESC",
            (SortBy::UpdatedAt, SortOrder::Asc) => "agent.updated_at ASC, agent.id ASC",
            (SortBy::UpdatedAt, SortOrder::Desc) => "agent.updated_at DESC, agent.id DESC",
            (SortBy::Id, SortOrder::Asc) => "agent.id ASC",
            (SortBy::Id, SortOrder::Desc) => "agent.id DESC",
            (SortBy::Priority, SortOrder::Asc) => "agent.created_at ASC, agent.id ASC",
            (SortBy::Priority, SortOrder::Desc) => "agent.created_at DESC, agent.id DESC",
            (SortBy::BoardPosition, SortOrder::Asc) => "agent.created_at ASC, agent.id ASC",
            (SortBy::BoardPosition, SortOrder::Desc) => "agent.created_at DESC, agent.id DESC",
            (SortBy::Title, SortOrder::Asc) => "agent.name ASC, agent.id ASC",
            (SortBy::Title, SortOrder::Desc) => "agent.name DESC, agent.id DESC",
            (SortBy::Status, SortOrder::Asc) => "agent.status ASC, agent.id ASC",
            (SortBy::Status, SortOrder::Desc) => "agent.status DESC, agent.id DESC",
            (SortBy::Agent, SortOrder::Asc) | (SortBy::TaskType, SortOrder::Asc) => {
                "agent.created_at ASC, agent.id ASC"
            }
            (SortBy::Agent, SortOrder::Desc) | (SortBy::TaskType, SortOrder::Desc) => {
                "agent.created_at DESC, agent.id DESC"
            }
        };
        let sql = format!(
            "SELECT agent.* FROM agent{} ORDER BY {} LIMIT ? OFFSET ?",
            where_sql, order_sql
        );
        let mut q = sqlx::query(&sql);
        if let Some(status) = &query.status {
            q = q.bind(status.to_string());
        }
        if let Some(executor_type) = &query.executor_type {
            q = q.bind(executor_type);
        }
        for capability in &query.capabilities {
            q = q.bind(format!("%\"{capability}\"%"));
        }
        let rows = q
            .bind(limit(&query.page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows
            .into_iter()
            .map(map_agent)
            .collect::<Result<Vec<_>>>()?;
        let total = if query.page.include_total {
            let count_sql = format!("SELECT COUNT(*) FROM agent{}", where_sql);
            let mut q = sqlx::query_scalar::<_, i64>(&count_sql);
            if let Some(status) = &query.status {
                q = q.bind(status.to_string());
            }
            if let Some(executor_type) = &query.executor_type {
                q = q.bind(executor_type);
            }
            for capability in &query.capabilities {
                q = q.bind(format!("%\"{capability}\"%"));
            }
            Some(q.fetch_one(&self.pool).await?)
        } else {
            None
        };
        page_from_items(items, &query.page, offset, total)
    }

    async fn update(&self, input: UpdateAgent) -> Result<Agent> {
        let mut agent = AgentRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        if agent.version != input.expected_version {
            return Err(DbError::VersionConflict);
        }
        if let Some(name) = input.name {
            agent.name = name;
        }
        if let Some(description) = input.description {
            agent.description = description;
        }
        if let Some(model) = input.model {
            agent.model = model;
        }
        if let Some(reasoning_effort) = input.reasoning_effort {
            agent.reasoning_effort = reasoning_effort;
        }
        if let Some(permission_policy) = input.permission_policy {
            agent.permission_policy = permission_policy;
        }
        if let Some(prompt_template) = input.prompt_template {
            agent.prompt_template = prompt_template;
        }
        if let Some(capabilities_json) = input.capabilities_json {
            agent.capabilities_json = capabilities_json;
        }
        if let Some(config_json) = input.config_json {
            agent.config_json = config_json;
        }
        if let Some(daemon_id) = input.daemon_id {
            agent.daemon_id = daemon_id;
        }
        if let Some(max_concurrent_tasks) = input.max_concurrent_tasks {
            agent.max_concurrent_tasks = max_concurrent_tasks;
        }
        if let Some(heartbeat_interval_seconds) = input.heartbeat_interval_seconds {
            agent.heartbeat_interval_seconds = heartbeat_interval_seconds;
        }
        if let Some(max_missed_heartbeats) = input.max_missed_heartbeats {
            agent.max_missed_heartbeats = max_missed_heartbeats;
        }
        if let Some(status) = input.status {
            agent.status = status;
        }
        if let Some(last_heartbeat_at) = input.last_heartbeat_at {
            agent.last_heartbeat_at = last_heartbeat_at;
        }
        if let Some(is_default) = input.is_default {
            agent.is_default = is_default;
        }
        if let Some(paused) = input.paused {
            agent.paused = paused;
        }
        if agent.is_default {
            sqlx::query("UPDATE agent SET is_default = 0 WHERE executor_type = ? AND id != ?")
                .bind(&agent.executor_type)
                .bind(&agent.id)
                .execute(&self.pool)
                .await?;
        }
        agent.updated_at = input.updated_at;
        agent.version += 1;
        let result = sqlx::query("UPDATE agent SET name = ?, description = ?, model = ?, reasoning_effort = ?, permission_policy = ?, prompt_template = ?, capabilities_json = ?, config_json = ?, daemon_id = ?, max_concurrent_tasks = ?, heartbeat_interval_seconds = ?, max_missed_heartbeats = ?, status = ?, last_heartbeat_at = ?, is_default = ?, paused = ?, version = version + 1, updated_at = ? WHERE id = ? AND version = ?")
            .bind(&agent.name)
            .bind(agent.description.as_deref())
            .bind(agent.model.as_deref())
            .bind(agent.reasoning_effort.as_deref())
            .bind(agent.permission_policy.as_deref())
            .bind(agent.prompt_template.as_deref())
            .bind(&agent.capabilities_json)
            .bind(&agent.config_json)
            .bind(agent.daemon_id.as_deref())
            .bind(agent.max_concurrent_tasks)
            .bind(agent.heartbeat_interval_seconds)
            .bind(agent.max_missed_heartbeats)
            .bind(agent.status.to_string())
            .bind(agent.last_heartbeat_at.as_deref())
            .bind(if agent.is_default { 1 } else { 0 })
            .bind(if agent.paused { 1 } else { 0 })
            .bind(&agent.updated_at)
            .bind(&agent.id)
            .bind(input.expected_version)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }
        Ok(agent)
    }

    async fn set_paused(&self, id: &str, paused: bool) -> Result<()> {
        let result = sqlx::query("UPDATE agent SET paused = ?, updated_at = ? WHERE id = ?")
            .bind(if paused { 1 } else { 0 })
            .bind(now_rfc3339())
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn duplicate_agent(
        &self,
        source_id: &str,
        new_id: String,
        new_name: String,
        now: String,
    ) -> Result<Agent> {
        let source = AgentRepo::get_by_id(self, source_id)
            .await?
            .ok_or(DbError::NotFound)?;
        AgentRepo::create(
            self,
            CreateAgent {
                id: new_id.clone(),
                name: new_name,
                description: source.description,
                executor_type: source.executor_type,
                model: source.model,
                reasoning_effort: source.reasoning_effort,
                permission_policy: source.permission_policy,
                prompt_template: source.prompt_template,
                capabilities_json: source.capabilities_json,
                config_json: source.config_json,
                daemon_id: source.daemon_id,
                max_concurrent_tasks: source.max_concurrent_tasks,
                heartbeat_interval_seconds: source.heartbeat_interval_seconds,
                max_missed_heartbeats: source.max_missed_heartbeats,
                status: AgentStatus::Idle,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: source.owner_id,
                visibility: source.visibility,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM agent WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn count_active_tasks(&self, agent_id: &str) -> Result<i64> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT
                (
                    SELECT COUNT(DISTINCT task.id)
                    FROM task
                    JOIN task_role_assignment ON task_role_assignment.task_id = task.id
                    JOIN project ON project.id = task.project_id
                    WHERE task_role_assignment.assignee_type = 'agent'
                      AND task_role_assignment.assignee_id = ?
                      AND task.deleted_at IS NULL
                      AND (
                          EXISTS (
                              SELECT 1
                              FROM json_each(
                                  CASE
                                      WHEN json_valid(project.workflow_definition)
                                      THEN project.workflow_definition
                                      ELSE '{\"states\":[]}'
                                  END,
                                  '$.states'
                              ) AS workflow_state
                              WHERE json_extract(workflow_state.value, '$.name') = task.status
                                AND json_extract(workflow_state.value, '$.kind') IN ('active', 'gate')
                          )
                          OR (
                              task.status IN ('in_progress', 'review', 'merging')
                              AND NOT EXISTS (
                                  SELECT 1
                                  FROM json_each(
                                      CASE
                                          WHEN json_valid(project.workflow_definition)
                                          THEN project.workflow_definition
                                          ELSE '{\"states\":[]}'
                                      END,
                                      '$.states'
                                  ) AS workflow_state
                                  WHERE json_extract(workflow_state.value, '$.name') = task.status
                              )
                          )
                      )
                ) +
                (
                    SELECT COUNT(DISTINCT conversation.id)
                    FROM conversation
                    JOIN conversation_message ON conversation_message.conversation_id = conversation.id
                    WHERE conversation.agent_id = ?
                      AND conversation_message.role = 'assistant'
                      AND conversation_message.status = 'streaming'
                )",
        )
        .bind(agent_id)
        .bind(agent_id)
        .fetch_one(&self.pool)
        .await?)
    }
}

impl SqliteDb {
    pub async fn list_agents_usable_in_project(
        &self,
        project_id: &str,
        user_id: &str,
    ) -> Result<Vec<Agent>> {
        let rows = sqlx::query(
            "SELECT DISTINCT agent.*
             FROM agent
             LEFT JOIN project_agent_link ON project_agent_link.agent_id = agent.id
             WHERE agent.visibility = 'global'
                OR (agent.visibility = 'account' AND agent.owner_id = ?)
                OR (project_agent_link.project_id = ? AND project_agent_link.agent_id = agent.id)
             ORDER BY agent.created_at ASC, agent.id ASC",
        )
        .bind(user_id)
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(map_agent).collect()
    }
}
