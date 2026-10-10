use std::sync::Arc;

use db::{AgentChat, AgentChatRepo, ProjectMemberRepo, SqliteDb};

use crate::{Result, ServiceError};

/// Read-only access to historical Main and Project Agent Chat transcripts.
/// This service has no API for admitting turns or changing chat bindings.
#[derive(Clone)]
pub struct HistoricalAgentChatReader<R = SqliteDb> {
    db: Arc<R>,
}

impl<R> HistoricalAgentChatReader<R>
where
    R: AgentChatRepo + ProjectMemberRepo + Send + Sync,
{
    pub fn new(db: Arc<R>) -> Self {
        Self { db }
    }

    pub async fn get_authorized_chat(
        &self,
        actor_user_id: &str,
        chat_id: &str,
    ) -> Result<AgentChat> {
        let chat = AgentChatRepo::get_agent_chat(&*self.db, chat_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent_chat", chat_id.to_owned()))?;
        match chat.kind.as_str() {
            "account_main" if chat.account_id.as_deref() == Some(actor_user_id) => {}
            "project" => {
                let project_id = chat
                    .project_id
                    .as_deref()
                    .ok_or_else(|| ServiceError::not_found("agent_chat", chat.id.clone()))?;
                ProjectMemberRepo::get_member(&*self.db, project_id, actor_user_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("agent_chat", chat.id.clone()))?;
            }
            _ => return Err(ServiceError::not_found("agent_chat", chat.id)),
        }
        Ok(chat)
    }

    pub async fn list_authorized_chats(&self, actor_user_id: &str) -> Result<Vec<AgentChat>> {
        Ok(AgentChatRepo::list_agent_chats(&*self.db, actor_user_id).await?)
    }
}
