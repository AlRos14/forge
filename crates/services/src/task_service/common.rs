use super::*;

impl TaskService {
    pub async fn add_user_comment(
        &self,
        task_id: &str,
        author_name: String,
        content: String,
    ) -> Result<TaskComment> {
        let now = now_rfc3339();
        let comment = TaskCommentRepo::create_comment(
            &*self.db,
            CreateTaskComment {
                id: new_uuid_v4(),
                task_id: task_id.to_owned(),
                author_type: CommentAuthorType::User,
                author_id: None,
                author_name,
                content,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;
        if let Err(error) = self.index_task_comment_memory(task_id, &comment).await {
            tracing::warn!(error = %error, "memory indexing failed (non-fatal)");
        }
        self.publish(ForgeEvent {
            event_type: "comment.created".to_owned(),
            entity_id: comment.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::CommentCreated {
                task_id: task_id.to_owned(),
                comment_id: comment.id.clone(),
                author_type: "user".to_owned(),
                author_name: comment.author_name.clone(),
            },
        });
        Ok(comment)
    }

    pub(crate) async fn create_system_comment(&self, task_id: &str, content: String) -> Result<()> {
        let now = now_rfc3339();
        let comment = TaskCommentRepo::create_comment(
            &*self.db,
            CreateTaskComment {
                id: new_uuid_v4(),
                task_id: task_id.to_owned(),
                author_type: CommentAuthorType::System,
                author_id: None,
                author_name: "Forge".to_owned(),
                content,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;
        if let Err(error) = self.index_task_comment_memory(task_id, &comment).await {
            tracing::warn!(error = %error, "memory indexing failed (non-fatal)");
        }
        self.publish(ForgeEvent {
            event_type: "comment.created".to_owned(),
            entity_id: comment.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::CommentCreated {
                task_id: task_id.to_owned(),
                comment_id: comment.id,
                author_type: "system".to_owned(),
                author_name: "Forge".to_owned(),
            },
        });
        Ok(())
    }

    #[allow(dead_code)] // Legacy completion cascade writer retained until PR13 cleanup.
    pub(super) async fn create_agent_comment(
        &self,
        task_id: &str,
        agent_id: &str,
        content: String,
    ) -> Result<()> {
        let agent = match AgentRepo::get_by_id(&*self.db, agent_id).await? {
            Some(agent) => agent,
            None => return self.create_system_comment(task_id, content).await,
        };
        let now = now_rfc3339();
        let comment = TaskCommentRepo::create_comment(
            &*self.db,
            CreateTaskComment {
                id: new_uuid_v4(),
                task_id: task_id.to_owned(),
                author_type: CommentAuthorType::Agent,
                author_id: Some(agent.id.clone()),
                author_name: agent.name.clone(),
                content,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;
        if let Err(error) = self.index_task_comment_memory(task_id, &comment).await {
            tracing::warn!(error = %error, "memory indexing failed (non-fatal)");
        }
        self.publish(ForgeEvent {
            event_type: "comment.created".to_owned(),
            entity_id: comment.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::CommentCreated {
                task_id: task_id.to_owned(),
                comment_id: comment.id,
                author_type: "agent".to_owned(),
                author_name: agent.name,
            },
        });
        Ok(())
    }

    async fn index_task_comment_memory(&self, task_id: &str, comment: &TaskComment) -> Result<()> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, true)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        self.memory_service
            .record_task_comment(&task.project_id, comment)
            .await?;
        Ok(())
    }

    pub async fn reset_task_workspace(&self, task_id: &str) -> Result<Workspace> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        reset_workspace(
            &self.db,
            &self.workspace_root,
            &task,
            self.repo_cache_locks.clone(),
        )
        .await
    }
}
