use super::*;

#[async_trait]
impl ConversationRepo for SqliteDb {
    async fn create(&self, input: CreateConversation) -> Result<Conversation> {
        sqlx::query("INSERT INTO conversation (id, project_id, agent_id, title, status, system_prompt, message_count, last_message_at, agent_session_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&input.id)
            .bind(&input.project_id)
            .bind(input.agent_id.as_deref())
            .bind(&input.title)
            .bind(input.status.to_string())
            .bind(input.system_prompt.as_deref())
            .bind(input.message_count)
            .bind(input.last_message_at.as_deref())
            .bind(input.agent_session_id.as_deref())
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&self.pool)
            .await?;
        ConversationRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Conversation>> {
        sqlx::query("SELECT * FROM conversation WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_conversation)
            .transpose()
    }

    async fn list_by_project(&self, query: ConversationListQuery) -> Result<Page<Conversation>> {
        let offset = decode_offset(&query.page.cursor)?;
        let rows = match &query.status {
            Some(status) => {
                sqlx::query("SELECT * FROM conversation WHERE project_id = ? AND status = ? ORDER BY COALESCE(last_message_at, created_at) DESC, id DESC LIMIT ? OFFSET ?")
                    .bind(&query.project_id)
                    .bind(status.to_string())
                    .bind(limit(&query.page) + 1)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                sqlx::query("SELECT * FROM conversation WHERE project_id = ? ORDER BY COALESCE(last_message_at, created_at) DESC, id DESC LIMIT ? OFFSET ?")
                    .bind(&query.project_id)
                    .bind(limit(&query.page) + 1)
                    .bind(offset)
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        let items = rows
            .into_iter()
            .map(map_conversation)
            .collect::<Result<Vec<_>>>()?;
        let total_count = if query.page.include_total {
            let total = match &query.status {
                Some(status) => {
                    sqlx::query_scalar::<_, i64>(
                        "SELECT COUNT(*) FROM conversation WHERE project_id = ? AND status = ?",
                    )
                    .bind(&query.project_id)
                    .bind(status.to_string())
                    .fetch_one(&self.pool)
                    .await?
                }
                None => {
                    sqlx::query_scalar::<_, i64>(
                        "SELECT COUNT(*) FROM conversation WHERE project_id = ?",
                    )
                    .bind(&query.project_id)
                    .fetch_one(&self.pool)
                    .await?
                }
            };
            Some(total)
        } else {
            None
        };
        page_from_items(items, &query.page, offset, total_count)
    }

    async fn update(&self, input: UpdateConversation) -> Result<Conversation> {
        let mut conversation = ConversationRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        if conversation.version != input.expected_version {
            return Err(DbError::VersionConflict);
        }
        if let Some(title) = input.title {
            conversation.title = title;
        }
        if let Some(status) = input.status {
            conversation.status = status;
        }
        if let Some(system_prompt) = input.system_prompt {
            conversation.system_prompt = system_prompt;
        }
        if let Some(agent_id) = input.agent_id {
            conversation.agent_id = agent_id;
        }
        if let Some(message_count) = input.message_count {
            conversation.message_count = message_count;
        }
        if let Some(last_message_at) = input.last_message_at {
            conversation.last_message_at = last_message_at;
        }
        if let Some(agent_session_id) = input.agent_session_id {
            conversation.agent_session_id = agent_session_id;
        }
        conversation.updated_at = input.updated_at;
        conversation.version += 1;

        let result = sqlx::query("UPDATE conversation SET agent_id = ?, title = ?, status = ?, system_prompt = ?, message_count = ?, last_message_at = ?, agent_session_id = ?, version = version + 1, updated_at = ? WHERE id = ? AND version = ?")
            .bind(conversation.agent_id.as_deref())
            .bind(&conversation.title)
            .bind(conversation.status.to_string())
            .bind(conversation.system_prompt.as_deref())
            .bind(conversation.message_count)
            .bind(conversation.last_message_at.as_deref())
            .bind(conversation.agent_session_id.as_deref())
            .bind(&conversation.updated_at)
            .bind(&conversation.id)
            .bind(input.expected_version)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }
        Ok(conversation)
    }
}

#[async_trait]
impl ConversationMessageRepo for SqliteDb {
    async fn create(&self, input: CreateConversationMessage) -> Result<ConversationMessage> {
        sqlx::query("INSERT INTO conversation_message (id, conversation_id, role, content, status, model, token_usage_json, duration_ms, error, sequence, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&input.id)
            .bind(&input.conversation_id)
            .bind(input.role.to_string())
            .bind(&input.content)
            .bind(input.status.to_string())
            .bind(input.model.as_deref())
            .bind(input.token_usage_json.as_deref())
            .bind(input.duration_ms)
            .bind(input.error.as_deref())
            .bind(input.sequence)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&self.pool)
            .await?;
        ConversationMessageRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<ConversationMessage>> {
        sqlx::query("SELECT * FROM conversation_message WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_conversation_message)
            .transpose()
    }

    async fn list_by_conversation(
        &self,
        query: ConversationMessageListQuery,
    ) -> Result<Page<ConversationMessage>> {
        let offset = decode_offset(&query.page.cursor)?;
        let rows = if let Some(before_sequence) = query.before_sequence {
            sqlx::query(
                "SELECT * FROM conversation_message WHERE conversation_id = ? AND sequence < ? ORDER BY sequence DESC LIMIT ? OFFSET ?",
            )
            .bind(&query.conversation_id)
            .bind(before_sequence)
            .bind(limit(&query.page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT * FROM conversation_message WHERE conversation_id = ? ORDER BY sequence DESC LIMIT ? OFFSET ?",
            )
            .bind(&query.conversation_id)
            .bind(limit(&query.page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        };
        let mut items = rows
            .into_iter()
            .map(map_conversation_message)
            .collect::<Result<Vec<_>>>()?;
        let limit = limit(&query.page) as usize;
        let has_next = items.len() > limit;
        if has_next {
            items.truncate(limit);
        }
        items.reverse();
        let total_count = if query.page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM conversation_message WHERE conversation_id = ?",
                )
                .bind(&query.conversation_id)
                .fetch_one(&self.pool)
                .await?,
            )
        } else {
            None
        };
        Ok(Page {
            items,
            next_cursor: if has_next {
                Some(encode_offset(offset + limit as i64)?)
            } else {
                None
            },
            total_count,
        })
    }

    async fn update(&self, input: UpdateConversationMessage) -> Result<ConversationMessage> {
        let mut message = ConversationMessageRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        if let Some(content) = input.content {
            message.content = content;
        }
        if let Some(status) = input.status {
            message.status = status;
        }
        if let Some(model) = input.model {
            message.model = model;
        }
        if let Some(token_usage_json) = input.token_usage_json {
            message.token_usage_json = token_usage_json;
        }
        if let Some(duration_ms) = input.duration_ms {
            message.duration_ms = duration_ms;
        }
        if let Some(error) = input.error {
            message.error = error;
        }
        message.updated_at = input.updated_at;
        sqlx::query("UPDATE conversation_message SET content = ?, status = ?, model = ?, token_usage_json = ?, duration_ms = ?, error = ?, updated_at = ? WHERE id = ?")
            .bind(&message.content)
            .bind(message.status.to_string())
            .bind(message.model.as_deref())
            .bind(message.token_usage_json.as_deref())
            .bind(message.duration_ms)
            .bind(message.error.as_deref())
            .bind(&message.updated_at)
            .bind(&message.id)
            .execute(&self.pool)
            .await?;
        Ok(message)
    }

    async fn next_sequence(&self, conversation_id: &str) -> Result<i64> {
        let max = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT MAX(sequence) FROM conversation_message WHERE conversation_id = ?",
        )
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await?
        .unwrap_or(0);
        Ok(max + 1)
    }

    async fn get_active_streaming_message(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationMessage>> {
        sqlx::query("SELECT * FROM conversation_message WHERE conversation_id = ? AND role = 'assistant' AND status = 'streaming' ORDER BY sequence DESC LIMIT 1")
            .bind(conversation_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_conversation_message)
            .transpose()
    }
}
