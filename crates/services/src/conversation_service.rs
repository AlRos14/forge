use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};

use db::{
    new_uuid_v4, now_rfc3339, AgentRepo, Conversation, ConversationListQuery, ConversationMessage,
    ConversationMessageListQuery, ConversationMessageRepo, ConversationMessageRole,
    ConversationMessageStatus, ConversationRepo, ConversationStatus, CreateConversation,
    CreateConversationMessage, Page, PageRequest, Project, ProjectRepo, RepoRepo, SqliteDb,
    UpdateConversation, UpdateConversationMessage,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use executors::{
    merge_overrides, resolve_config_value, ExecutionContext, ExecutionOutcome, ExecutionOverrides,
    ExecutorKind, LogEntry, LogKind, LogReader, TaskExecutor,
};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};

use crate::{
    agent_capacity::count_running_executions_excluding_conversation, Result, ServiceError,
};

#[derive(Clone)]
pub struct ConversationService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    inflight: Arc<Mutex<HashMap<String, String>>>,
}

impl ConversationService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            db,
            event_bus,
            inflight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn create_conversation(
        &self,
        project_id: String,
        agent_id: String,
        title: Option<String>,
        system_prompt: Option<String>,
    ) -> Result<Conversation> {
        ProjectRepo::get_by_id(&*self.db, &project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.clone()))?;
        AgentRepo::get_by_id(&*self.db, &agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;

        let now = now_rfc3339();
        let conversation = ConversationRepo::create(
            &*self.db,
            CreateConversation {
                id: new_uuid_v4(),
                project_id: project_id.clone(),
                agent_id: Some(agent_id),
                title: title
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "New Conversation".to_owned()),
                status: ConversationStatus::Active,
                system_prompt,
                message_count: 0,
                last_message_at: None,
                agent_session_id: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;

        self.publish_conversation_updated(&conversation);
        Ok(conversation)
    }

    pub async fn list_conversations(
        &self,
        project_id: String,
        status: Option<ConversationStatus>,
        page: PageRequest,
    ) -> Result<Page<Conversation>> {
        ConversationRepo::list_by_project(
            &*self.db,
            ConversationListQuery {
                project_id,
                status,
                page,
            },
        )
        .await
        .map_err(ServiceError::from)
    }

    pub async fn get_conversation(&self, id: String) -> Result<Conversation> {
        ConversationRepo::get_by_id(&*self.db, &id)
            .await?
            .ok_or_else(|| ServiceError::not_found("conversation", id))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_conversation(
        &self,
        id: String,
        version: i64,
        title: Option<String>,
        agent_id: Option<String>,
        system_prompt: Option<String>,
        status: Option<ConversationStatus>,
    ) -> Result<Conversation> {
        let current = self.get_conversation(id.clone()).await?;

        if let Some(agent_id) = agent_id.as_deref() {
            AgentRepo::get_by_id(&*self.db, agent_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("agent", agent_id.to_owned()))?;
        }

        let now = now_rfc3339();
        let mut updated = ConversationRepo::update(
            &*self.db,
            UpdateConversation {
                id: id.clone(),
                expected_version: version,
                agent_id: agent_id.clone().map(Some),
                title,
                status,
                system_prompt: system_prompt.map(Some),
                message_count: None,
                last_message_at: None,
                agent_session_id: if agent_id.is_some() { Some(None) } else { None },
                updated_at: now.clone(),
            },
        )
        .await?;

        if let Some(new_agent_id) = agent_id {
            if current.agent_id != Some(new_agent_id.clone()) {
                let previous = current.agent_id.unwrap_or_else(|| "none".to_owned());
                let sequence = ConversationMessageRepo::next_sequence(&*self.db, &id).await?;
                let system_message = ConversationMessageRepo::create(
                    &*self.db,
                    CreateConversationMessage {
                        id: new_uuid_v4(),
                        conversation_id: id.clone(),
                        role: ConversationMessageRole::System,
                        content: json!({
                            "type": "agent_changed",
                            "from": previous,
                            "to": new_agent_id,
                        })
                        .to_string(),
                        status: ConversationMessageStatus::Complete,
                        model: None,
                        token_usage_json: None,
                        duration_ms: None,
                        error: None,
                        sequence,
                        created_at: now.clone(),
                        updated_at: now.clone(),
                    },
                )
                .await?;
                updated = ConversationRepo::update(
                    &*self.db,
                    UpdateConversation {
                        id: id.clone(),
                        expected_version: updated.version,
                        agent_id: None,
                        title: None,
                        status: None,
                        system_prompt: None,
                        message_count: Some(updated.message_count + 1),
                        last_message_at: Some(Some(now.clone())),
                        agent_session_id: Some(None),
                        updated_at: now.clone(),
                    },
                )
                .await?;
                self.publish_message_created(&updated.project_id, &updated.id, &system_message);
            }
        }

        self.publish_conversation_updated(&updated);
        Ok(updated)
    }

    pub async fn archive_conversation(&self, id: String) -> Result<Conversation> {
        let current = self.get_conversation(id.clone()).await?;
        let updated = ConversationRepo::update(
            &*self.db,
            UpdateConversation {
                id,
                expected_version: current.version,
                agent_id: None,
                title: None,
                status: Some(ConversationStatus::Archived),
                system_prompt: None,
                message_count: None,
                last_message_at: None,
                agent_session_id: None,
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        self.publish_conversation_updated(&updated);
        Ok(updated)
    }

    pub async fn list_messages(
        &self,
        conversation_id: String,
        before_sequence: Option<i64>,
        page: PageRequest,
    ) -> Result<Page<ConversationMessage>> {
        self.get_conversation(conversation_id.clone()).await?;
        ConversationMessageRepo::list_by_conversation(
            &*self.db,
            ConversationMessageListQuery {
                conversation_id,
                before_sequence,
                page,
            },
        )
        .await
        .map_err(ServiceError::from)
    }

    pub async fn list_log_entries(&self, conversation_id: String) -> Result<Vec<LogEntry>> {
        self.get_conversation(conversation_id.clone()).await?;
        let messages = ConversationMessageRepo::list_by_conversation(
            &*self.db,
            ConversationMessageListQuery {
                conversation_id: conversation_id.clone(),
                before_sequence: None,
                page: PageRequest {
                    cursor: None,
                    limit: 1_000,
                    include_total: false,
                    sort_by: db::SortBy::CreatedAt,
                    sort_order: db::SortOrder::Desc,
                },
            },
        )
        .await?
        .items;

        let mut entries = Vec::new();
        let mut sequence = 0_u64;
        let mut preceding_user_prompt: Option<String> = None;
        for message in messages {
            match message.role {
                ConversationMessageRole::User => {
                    preceding_user_prompt = Some(message.content.clone());
                    entries.push(synthetic_log_entry(
                        sequence,
                        &message.id,
                        &message.created_at,
                        LogKind::User,
                        json!({ "text": message.content }),
                    ));
                    sequence += 1;
                }
                ConversationMessageRole::Assistant => {
                    let path = conversation_logs_path(&conversation_id, &message.id);
                    let log_entries = read_conversation_log_file(&path).await;
                    let has_assistant_output = log_entries.iter().any(|entry| {
                        matches!(entry.kind, LogKind::Assistant | LogKind::AssistantDelta)
                    });
                    if !log_entries.is_empty() {
                        for mut entry in log_entries.into_iter().filter(|entry| {
                            !is_duplicate_prompt_user_log(entry, preceding_user_prompt.as_deref())
                        }) {
                            entry.sequence = sequence;
                            sequence += 1;
                            entries.push(entry);
                        }
                    }
                    if !has_assistant_output
                        && (!message.content.is_empty() || message.error.is_some())
                    {
                        let text = if message.content.is_empty() {
                            message.error.clone().unwrap_or_default()
                        } else {
                            message.content.clone()
                        };
                        let mut payload = json!({ "text": text.clone() });
                        if let Some(error) = &message.error {
                            payload = json!({ "text": text, "error": error });
                        }
                        entries.push(synthetic_log_entry(
                            sequence,
                            &message.id,
                            &message.updated_at,
                            LogKind::Assistant,
                            payload,
                        ));
                        sequence += 1;
                    }
                    preceding_user_prompt = None;
                }
                ConversationMessageRole::System => {
                    preceding_user_prompt = None;
                    entries.push(synthetic_log_entry(
                        sequence,
                        &message.id,
                        &message.created_at,
                        LogKind::System,
                        serde_json::from_str(&message.content)
                            .unwrap_or_else(|_| json!({ "text": message.content })),
                    ));
                    sequence += 1;
                }
            }
        }

        Ok(entries)
    }

    pub async fn send_message(
        &self,
        conversation_id: String,
        content: String,
        overrides: Option<ExecutionOverrides>,
        executor: Arc<dyn TaskExecutor>,
    ) -> Result<(ConversationMessage, ConversationMessage)> {
        let mut conversation = self.get_conversation(conversation_id.clone()).await?;
        if conversation.status == ConversationStatus::Archived {
            return Err(ServiceError::conflict(
                "conversation is archived and cannot accept new messages",
            ));
        }
        let content = content.trim().to_owned();
        if content.is_empty() {
            return Err(ServiceError::invalid_operation(
                "message content cannot be empty",
            ));
        }

        if let Some(active) =
            ConversationMessageRepo::get_active_streaming_message(&*self.db, &conversation_id)
                .await?
        {
            let _ = self
                .cancel_response(conversation_id.clone(), Arc::clone(&executor))
                .await;
            let _ = ConversationMessageRepo::update(
                &*self.db,
                UpdateConversationMessage {
                    id: active.id,
                    content: None,
                    status: Some(ConversationMessageStatus::Cancelled),
                    model: None,
                    token_usage_json: None,
                    duration_ms: None,
                    error: None,
                    updated_at: now_rfc3339(),
                },
            )
            .await?;
        }

        let now = now_rfc3339();
        let user_sequence =
            ConversationMessageRepo::next_sequence(&*self.db, &conversation_id).await?;
        let user_message = ConversationMessageRepo::create(
            &*self.db,
            CreateConversationMessage {
                id: new_uuid_v4(),
                conversation_id: conversation_id.clone(),
                role: ConversationMessageRole::User,
                content: content.clone(),
                status: ConversationMessageStatus::Complete,
                model: None,
                token_usage_json: None,
                duration_ms: None,
                error: None,
                sequence: user_sequence,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await?;

        let assistant_message = ConversationMessageRepo::create(
            &*self.db,
            CreateConversationMessage {
                id: new_uuid_v4(),
                conversation_id: conversation_id.clone(),
                role: ConversationMessageRole::Assistant,
                content: String::new(),
                status: ConversationMessageStatus::Streaming,
                model: None,
                token_usage_json: None,
                duration_ms: None,
                error: None,
                sequence: user_sequence + 1,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await?;

        conversation = ConversationRepo::update(
            &*self.db,
            UpdateConversation {
                id: conversation.id.clone(),
                expected_version: conversation.version,
                agent_id: None,
                title: if conversation.message_count == 0
                    && conversation.title == "New Conversation"
                {
                    Some(content.chars().take(80).collect())
                } else {
                    None
                },
                status: None,
                system_prompt: None,
                message_count: Some(conversation.message_count + 2),
                last_message_at: Some(Some(now.clone())),
                agent_session_id: None,
                updated_at: now.clone(),
            },
        )
        .await?;

        self.publish_message_created(&conversation.project_id, &conversation.id, &user_message);
        self.publish_message_created(
            &conversation.project_id,
            &conversation.id,
            &assistant_message,
        );
        self.publish_conversation_updated(&conversation);

        let service = self.clone();
        let user_message_for_task = user_message.clone();
        let assistant_message_for_task = assistant_message.clone();
        tokio::spawn(async move {
            service
                .run_assistant_turn(
                    conversation,
                    user_message_for_task,
                    assistant_message_for_task,
                    overrides,
                    executor,
                )
                .await;
        });

        Ok((user_message, assistant_message))
    }

    pub async fn cancel_response(
        &self,
        conversation_id: String,
        executor: Arc<dyn TaskExecutor>,
    ) -> Result<()> {
        let execution_id = {
            let inflight = self.inflight.lock().await;
            inflight.get(&conversation_id).cloned()
        };
        let Some(execution_id) = execution_id else {
            return Err(ServiceError::conflict("no active response"));
        };
        executor.cancel(&execution_id).await?;
        {
            let mut inflight = self.inflight.lock().await;
            inflight.remove(&conversation_id);
        }
        if let Some(active) =
            ConversationMessageRepo::get_active_streaming_message(&*self.db, &conversation_id)
                .await?
        {
            let updated = ConversationMessageRepo::update(
                &*self.db,
                UpdateConversationMessage {
                    id: active.id,
                    content: None,
                    status: Some(ConversationMessageStatus::Cancelled),
                    model: None,
                    token_usage_json: None,
                    duration_ms: None,
                    error: None,
                    updated_at: now_rfc3339(),
                },
            )
            .await?;
            let conversation = self.get_conversation(conversation_id.clone()).await?;
            self.publish_message_completed(
                &conversation.project_id,
                &conversation_id,
                &updated,
                None,
            );
        }
        Ok(())
    }

    async fn run_assistant_turn(
        &self,
        conversation: Conversation,
        user_message: ConversationMessage,
        assistant_message: ConversationMessage,
        overrides: Option<ExecutionOverrides>,
        executor: Arc<dyn TaskExecutor>,
    ) {
        {
            let mut inflight = self.inflight.lock().await;
            inflight.insert(conversation.id.clone(), assistant_message.id.clone());
        }

        let start = Instant::now();
        let result = self
            .run_assistant_turn_inner(
                &conversation,
                &user_message,
                &assistant_message,
                overrides,
                executor,
            )
            .await;

        if let Err(error) = result {
            let updated = ConversationMessageRepo::update(
                &*self.db,
                UpdateConversationMessage {
                    id: assistant_message.id.clone(),
                    content: None,
                    status: Some(ConversationMessageStatus::Failed),
                    model: None,
                    token_usage_json: None,
                    duration_ms: Some(Some(start.elapsed().as_millis() as i64)),
                    error: Some(Some(error.to_string())),
                    updated_at: now_rfc3339(),
                },
            )
            .await;
            if error.to_string().contains("thread not found") {
                let latest = self.get_conversation(conversation.id.clone()).await;
                if let Ok(latest) = latest {
                    let _ = ConversationRepo::update(
                        &*self.db,
                        UpdateConversation {
                            id: latest.id.clone(),
                            expected_version: latest.version,
                            agent_id: None,
                            title: None,
                            status: None,
                            system_prompt: None,
                            message_count: None,
                            last_message_at: Some(Some(now_rfc3339())),
                            agent_session_id: Some(None),
                            updated_at: now_rfc3339(),
                        },
                    )
                    .await;
                }
            }
            self.publish_message_completed(
                &conversation.project_id,
                &conversation.id,
                updated.as_ref().unwrap_or(&assistant_message),
                Some(error.to_string()),
            );
        }

        let mut inflight = self.inflight.lock().await;
        inflight.remove(&conversation.id);
    }

    async fn run_assistant_turn_inner(
        &self,
        conversation: &Conversation,
        user_message: &ConversationMessage,
        assistant_message: &ConversationMessage,
        overrides: Option<ExecutionOverrides>,
        executor: Arc<dyn TaskExecutor>,
    ) -> Result<()> {
        let agent_id = conversation
            .agent_id
            .clone()
            .ok_or_else(|| ServiceError::invalid_operation("conversation has no assigned agent"))?;
        let agent = AgentRepo::get_by_id(&*self.db, &agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
        let kind = agent
            .executor_type
            .parse::<ExecutorKind>()
            .map_err(ServiceError::invalid_operation)?;
        if kind == ExecutorKind::Shell {
            return Err(ServiceError::invalid_operation(
                "Shell executor does not support conversation mode (Plan policy is a no-op for shell)",
            ));
        }

        let active_count =
            count_running_executions_excluding_conversation(&self.db, &agent.id, &conversation.id)
                .await?;
        if active_count >= agent.max_concurrent_tasks {
            return Err(ServiceError::conflict(
                "Agent at capacity — try again when the agent is free",
            ));
        }

        let history = ConversationMessageRepo::list_by_conversation(
            &*self.db,
            ConversationMessageListQuery {
                conversation_id: conversation.id.clone(),
                before_sequence: Some(user_message.sequence),
                page: PageRequest {
                    cursor: None,
                    limit: 50,
                    include_total: false,
                    sort_by: db::SortBy::CreatedAt,
                    sort_order: db::SortOrder::Desc,
                },
            },
        )
        .await?
        .items;
        let project = ProjectRepo::get_by_id(&*self.db, &conversation.project_id).await?;
        let full_prompt = build_prompt(
            conversation,
            project.as_ref(),
            &agent.prompt_template,
            &history,
            &user_message.content,
        );
        let prompt = if conversation.agent_session_id.is_some() {
            user_message.content.clone()
        } else {
            full_prompt.clone()
        };
        let config = build_executor_config(
            &agent.config_json,
            &agent.executor_type,
            agent.model.as_deref(),
            agent.reasoning_effort.as_deref(),
            conversation
                .system_prompt
                .as_deref()
                .or(agent.prompt_template.as_deref()),
            conversation.agent_session_id.as_deref(),
            Some(full_prompt.as_str()),
            overrides.as_ref(),
        )?;
        let logs_path = conversation_logs_path(&conversation.id, &assistant_message.id);
        if let Some(parent) = logs_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let (log_tx, mut log_rx) = mpsc::unbounded_channel::<LogEntry>();
        let service = self.clone();
        let conversation_id = conversation.id.clone();
        let project_id = conversation.project_id.clone();
        let message_id = assistant_message.id.clone();
        let user_prompt = user_message.content.clone();
        tokio::spawn(async move {
            while let Some(entry) = log_rx.recv().await {
                if let Some(delta) = extract_delta_text(&entry) {
                    let _ = service
                        .append_assistant_delta(&project_id, &conversation_id, &message_id, &delta)
                        .await;
                }
                if !is_duplicate_prompt_user_log(&entry, Some(&user_prompt)) {
                    service.publish_conversation_log(&project_id, &conversation_id, &entry);
                }
            }
        });

        let start = Instant::now();
        let worktree_path = self.conversation_worktree_path(conversation).await?;
        let result = executor
            .execute(ExecutionContext {
                task_id: conversation.id.clone(),
                execution_id: assistant_message.id.clone(),
                worktree_path,
                description: prompt,
                agent_config: config,
                logs_path: logs_path.to_string_lossy().into_owned(),
                heartbeat_interval_seconds: 30,
                max_turns: None,
                log_sender: Some(log_tx),
            })
            .await?;

        let status = match result.status {
            ExecutionOutcome::Completed => ConversationMessageStatus::Complete,
            ExecutionOutcome::Cancelled => ConversationMessageStatus::Cancelled,
            ExecutionOutcome::Failed => ConversationMessageStatus::Failed,
        };
        let updated = ConversationMessageRepo::update(
            &*self.db,
            UpdateConversationMessage {
                id: assistant_message.id.clone(),
                content: result.summary.clone(),
                status: Some(status),
                model: Some(result.usage.as_ref().and_then(|usage| usage.model.clone())),
                token_usage_json: Some(result.usage.as_ref().map(|usage| {
                    json!({
                        "input": usage.input_tokens,
                        "output": usage.output_tokens,
                        "cache_read": usage.cache_read_tokens,
                        "cache_write": usage.cache_write_tokens,
                        "cost_usd": usage.cost_usd
                    })
                    .to_string()
                })),
                duration_ms: Some(Some(start.elapsed().as_millis() as i64)),
                error: Some(result.error.clone()),
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        if let Some(session_id) = result.agent_session_id {
            let latest = self.get_conversation(conversation.id.clone()).await?;
            let _ = ConversationRepo::update(
                &*self.db,
                UpdateConversation {
                    id: latest.id.clone(),
                    expected_version: latest.version,
                    agent_id: None,
                    title: None,
                    status: None,
                    system_prompt: None,
                    message_count: None,
                    last_message_at: Some(Some(now_rfc3339())),
                    agent_session_id: Some(Some(session_id)),
                    updated_at: now_rfc3339(),
                },
            )
            .await?;
        }
        self.publish_message_completed(
            &conversation.project_id,
            &conversation.id,
            &updated,
            updated.error.clone(),
        );
        Ok(())
    }

    async fn conversation_worktree_path(&self, conversation: &Conversation) -> Result<String> {
        let fallback = || {
            std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .to_string_lossy()
                .into_owned()
        };
        let Some(project) = ProjectRepo::get_by_id(&*self.db, &conversation.project_id).await?
        else {
            return Ok(fallback());
        };
        let Some(repo_id) = project.primary_repo_id else {
            return Ok(fallback());
        };
        let Some(repo) = RepoRepo::get_by_id(&*self.db, &repo_id).await? else {
            return Ok(fallback());
        };
        let Some(local_path) = repo.local_path else {
            return Ok(fallback());
        };
        if std::path::Path::new(&local_path).exists() {
            Ok(local_path)
        } else {
            Ok(fallback())
        }
    }

    async fn append_assistant_delta(
        &self,
        project_id: &str,
        conversation_id: &str,
        message_id: &str,
        delta: &str,
    ) -> Result<()> {
        let Some(message) = ConversationMessageRepo::get_by_id(&*self.db, message_id).await? else {
            return Ok(());
        };
        let mut content = message.content;
        content.push_str(delta);
        let updated = ConversationMessageRepo::update(
            &*self.db,
            UpdateConversationMessage {
                id: message_id.to_owned(),
                content: Some(content),
                status: None,
                model: None,
                token_usage_json: None,
                duration_ms: None,
                error: None,
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        self.event_bus.publish(ForgeEvent {
            event_type: "conversation.message_delta".to_owned(),
            entity_id: updated.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ConversationMessageDelta {
                project_id: project_id.to_owned(),
                conversation_id: conversation_id.to_owned(),
                message_id: updated.id,
                delta: delta.to_owned(),
            },
        });
        Ok(())
    }

    fn publish_message_created(
        &self,
        project_id: &str,
        conversation_id: &str,
        message: &ConversationMessage,
    ) {
        self.event_bus.publish(ForgeEvent {
            event_type: "conversation.message_created".to_owned(),
            entity_id: message.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ConversationMessageCreated {
                project_id: project_id.to_owned(),
                conversation_id: conversation_id.to_owned(),
                message_id: message.id.clone(),
                role: message.role.to_string(),
                status: message.status.to_string(),
            },
        });
    }

    fn publish_message_completed(
        &self,
        project_id: &str,
        conversation_id: &str,
        message: &ConversationMessage,
        error: Option<String>,
    ) {
        self.event_bus.publish(ForgeEvent {
            event_type: "conversation.message_completed".to_owned(),
            entity_id: message.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ConversationMessageCompleted {
                project_id: project_id.to_owned(),
                conversation_id: conversation_id.to_owned(),
                message_id: message.id.clone(),
                status: message.status.to_string(),
                error,
            },
        });
    }

    fn publish_conversation_updated(&self, conversation: &Conversation) {
        self.event_bus.publish(ForgeEvent {
            event_type: "conversation.updated".to_owned(),
            entity_id: conversation.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ConversationUpdated {
                project_id: conversation.project_id.clone(),
                conversation_id: conversation.id.clone(),
                status: conversation.status.to_string(),
            },
        });
    }

    fn publish_conversation_log(&self, project_id: &str, conversation_id: &str, entry: &LogEntry) {
        self.event_bus.publish(ForgeEvent {
            event_type: "conversation.log".to_owned(),
            entity_id: conversation_id.to_owned(),
            timestamp: event_timestamp(),
            context: EventContext::ConversationLog {
                project_id: project_id.to_owned(),
                conversation_id: conversation_id.to_owned(),
                log: serde_json::to_value(entry).unwrap_or_default(),
            },
        });
    }
}

fn conversation_logs_path(conversation_id: &str, message_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("forge")
        .join("conversations")
        .join(conversation_id)
        .join(format!("{message_id}.jsonl"))
}

async fn read_conversation_log_file(path: &std::path::Path) -> Vec<LogEntry> {
    let mut entries = Vec::new();
    let mut from_sequence = 0_u64;
    loop {
        let Ok(result) = LogReader::read(path, from_sequence, 500).await else {
            return entries;
        };
        if result.entries.is_empty() {
            return entries;
        }
        entries.extend(result.entries);
        let Some(next_sequence) = result.next_sequence else {
            return entries;
        };
        if !result.has_more {
            return entries;
        }
        from_sequence = next_sequence;
    }
}

fn synthetic_log_entry(
    sequence: u64,
    execution_id: &str,
    timestamp: &str,
    kind: LogKind,
    payload: Value,
) -> LogEntry {
    LogEntry {
        schema_version: 1,
        sequence,
        timestamp: timestamp.to_owned(),
        execution_id: execution_id.to_owned(),
        kind,
        stream: executors::LogStream::Main,
        payload,
        truncated: false,
    }
}

fn build_prompt(
    conversation: &Conversation,
    project: Option<&Project>,
    agent_prompt_template: &Option<String>,
    history: &[ConversationMessage],
    user_message: &str,
) -> String {
    let mut sections = vec![
        conversation
            .system_prompt
            .clone()
            .or_else(|| agent_prompt_template.clone())
            .unwrap_or_else(|| "You are a helpful project assistant.".to_owned()),
        project_context_section(conversation, project),
        "Available Forge MCP tools: forge_create_task, forge_list_tasks, forge_get_task, forge_update_task, forge_transition_task, forge_cancel_task, forge_list_projects, forge_get_project, forge_create_project, forge_update_project, forge_update_project_lifecycle_hooks, forge_list_agents, forge_list_executions.\nConversation mode may inspect project files with read-only filesystem and shell access. Do not edit files, stage changes, or commit. Use Forge MCP tools for project and task management when appropriate.".to_owned(),
        "Conversation history:".to_owned(),
    ];
    for message in history.iter().rev().take(50) {
        sections.push(format!("{}: {}", message.role, message.content));
    }
    sections.push(format!("user: {user_message}"));
    sections.join("\n\n")
}

fn project_context_section(conversation: &Conversation, project: Option<&Project>) -> String {
    let project_name = project
        .map(|project| project.name.as_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Unknown");
    format!(
        "Current Forge project:\n- id: {}\n- name: {}\nUse this project id for Forge MCP tool calls when the user refers to this project, the current project, or this workspace.",
        conversation.project_id, project_name
    )
}

#[allow(clippy::too_many_arguments)]
fn build_executor_config(
    raw_config_json: &str,
    executor_type: &str,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    prompt_template: Option<&str>,
    resume_session_id: Option<&str>,
    resume_fallback_prompt: Option<&str>,
    overrides: Option<&ExecutionOverrides>,
) -> Result<Value> {
    let mut config = serde_json::from_str::<Value>(raw_config_json).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid agent config_json: {error}"))
    })?;
    let Some(map) = config.as_object_mut() else {
        return Err(ServiceError::invalid_operation(
            "agent config_json must be a JSON object",
        ));
    };
    if let Some(model) = model {
        map.insert("model".to_owned(), Value::String(model.to_owned()));
    }
    if let Some(reasoning_effort) = reasoning_effort {
        map.insert(
            "model_reasoning_effort".to_owned(),
            Value::String(reasoning_effort.to_owned()),
        );
        map.insert(
            "effort".to_owned(),
            Value::String(reasoning_effort.to_owned()),
        );
    }
    if let Some(prompt_template) = prompt_template {
        map.insert(
            "prompt_template".to_owned(),
            Value::String(prompt_template.to_owned()),
        );
    }
    map.insert(
        "permission_policy".to_owned(),
        Value::String(conversation_permission_policy(executor_type).to_owned()),
    );
    map.insert(
        "ask_for_approval".to_owned(),
        Value::String("never".to_owned()),
    );
    map.insert("sandbox".to_owned(), Value::String("read-only".to_owned()));
    map.insert("auto_commit".to_owned(), Value::Bool(false));
    if let Some(session_id) = resume_session_id {
        if executor_type == "codex" {
            map.insert(
                "resume_thread_id".to_owned(),
                Value::String(session_id.to_owned()),
            );
            map.insert("resume_thread_in_place".to_owned(), Value::Bool(true));
            if let Some(prompt) = resume_fallback_prompt {
                map.insert(
                    "resume_fallback_prompt".to_owned(),
                    Value::String(prompt.to_owned()),
                );
            } else {
                map.remove("resume_fallback_prompt");
            }
        } else if executor_type == "claude_code" {
            map.insert(
                "resume_session_id".to_owned(),
                Value::String(session_id.to_owned()),
            );
        }
    }
    if let Some(overrides) = overrides {
        merge_overrides(&mut config, overrides)?;
        if let Some(map) = config.as_object_mut() {
            map.insert(
                "permission_policy".to_owned(),
                Value::String(conversation_permission_policy(executor_type).to_owned()),
            );
        }
    }
    let kind = executor_type
        .parse::<ExecutorKind>()
        .map_err(ServiceError::invalid_operation)?;
    let resolved = resolve_config_value(kind, &config, &ExecutionOverrides::default())?;
    Ok(json!({
        "executor_type": executor_type,
        "config": resolved,
    }))
}

fn conversation_permission_policy(executor_type: &str) -> &'static str {
    if executor_type == "claude_code" {
        "supervised"
    } else {
        "plan"
    }
}

fn extract_delta_text(entry: &LogEntry) -> Option<String> {
    match entry.kind {
        LogKind::AssistantDelta => payload_text(&entry.payload),
        LogKind::Assistant => payload_text(&entry.payload),
        _ => None,
    }
}

fn is_duplicate_prompt_user_log(entry: &LogEntry, prompt: Option<&str>) -> bool {
    if entry.kind != LogKind::User {
        return false;
    }
    if entry
        .payload
        .get("source")
        .and_then(Value::as_str)
        .is_some_and(|source| source == "forge_prompt")
    {
        return true;
    }
    let Some(prompt) = prompt.map(str::trim).filter(|prompt| !prompt.is_empty()) else {
        return false;
    };
    payload_text(&entry.payload)
        .map(|text| text.trim() == prompt)
        .unwrap_or(false)
}

fn payload_text(payload: &Value) -> Option<String> {
    if let Some(text) = payload.as_str() {
        if !text.is_empty() {
            return Some(text.to_owned());
        }
    }
    for key in ["delta", "text", "message", "content"] {
        if let Some(text) = payload.get(key).and_then(Value::as_str) {
            if !text.is_empty() {
                return Some(text.to_owned());
            }
        }
    }
    payload
        .get("params")
        .and_then(|params| params.get("delta").and_then(Value::as_str))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    use async_trait::async_trait;
    use db::{
        create_sqlite_pool, run_migrations, AgentRepo, AgentStatus, ConversationMessageRepo,
        ConversationMessageStatus, CreateAgent, CreateProject, CreateRepo, PageRequest,
        ProjectRepo, RepoRepo, SortBy, SortOrder, SqliteDb, UpdateProject, WorkMode,
    };
    use events::EventBus;
    use executors::{
        ExecutionContext, ExecutionOutcome, ExecutionResult, ExecutorError, LogStream, LogWriter,
        TaskExecutor,
    };

    use super::*;

    struct CountingExecutor {
        execute_calls: Arc<AtomicUsize>,
        worktree_paths: Arc<Mutex<Vec<String>>>,
        descriptions: Arc<Mutex<Vec<String>>>,
    }

    struct FailingExecutor {
        error: &'static str,
    }

    struct ToolLoggingExecutor;

    #[async_trait]
    impl TaskExecutor for CountingExecutor {
        async fn execute(
            &self,
            ctx: ExecutionContext,
        ) -> std::result::Result<ExecutionResult, ExecutorError> {
            self.execute_calls.fetch_add(1, Ordering::SeqCst);
            self.worktree_paths
                .lock()
                .expect("worktree paths lock")
                .push(ctx.worktree_path.clone());
            self.descriptions
                .lock()
                .expect("descriptions lock")
                .push(ctx.description);
            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                after_sha: None,
                agent_session_id: Some("session-1".to_owned()),
                summary: Some("Acknowledged".to_owned()),
                error: None,
                usage: None,
            })
        }

        async fn cancel(&self, _execution_id: &str) -> std::result::Result<(), ExecutorError> {
            Ok(())
        }
    }

    #[async_trait]
    impl TaskExecutor for FailingExecutor {
        async fn execute(
            &self,
            _ctx: ExecutionContext,
        ) -> std::result::Result<ExecutionResult, ExecutorError> {
            Err(ExecutorError::Other(self.error.to_owned()))
        }

        async fn cancel(&self, _execution_id: &str) -> std::result::Result<(), ExecutorError> {
            Ok(())
        }
    }

    #[async_trait]
    impl TaskExecutor for ToolLoggingExecutor {
        async fn execute(
            &self,
            ctx: ExecutionContext,
        ) -> std::result::Result<ExecutionResult, ExecutorError> {
            let mut writer = LogWriter::new(&ctx.logs_path, ctx.execution_id.clone(), 1024 * 1024);
            if let Some(sender) = ctx.log_sender {
                writer.set_log_sender(sender);
            }
            writer
                .write(
                    LogKind::User,
                    LogStream::Main,
                    json!({ "text": ctx.description, "source": "forge_prompt" }),
                )
                .await?;
            writer
                .write(
                    LogKind::ToolCall,
                    LogStream::Main,
                    json!({ "tool": "forge_list_tasks", "call_id": "call-1", "params": {} }),
                )
                .await?;
            writer
                .write(
                    LogKind::ToolResult,
                    LogStream::Main,
                    json!({ "call_id": "call-1", "success": true, "content": [] }),
                )
                .await?;
            writer
                .write(
                    LogKind::Assistant,
                    LogStream::Main,
                    json!({ "text": "Tool completed" }),
                )
                .await?;

            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                after_sha: None,
                agent_session_id: Some("session-1".to_owned()),
                summary: Some("Tool completed".to_owned()),
                error: None,
                usage: None,
            })
        }

        async fn cancel(&self, _execution_id: &str) -> std::result::Result<(), ExecutorError> {
            Ok(())
        }
    }

    async fn make_service() -> ConversationService {
        let pool = create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        run_migrations(&pool).await.expect("migrations run");
        let db = Arc::new(SqliteDb::new(pool));
        let event_bus = Arc::new(EventBus::new(32));

        let now = now_rfc3339();
        let project_id = "project-1".to_owned();
        ProjectRepo::create(
            &*db,
            CreateProject {
                id: project_id,
                name: "Project".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("project creates");

        AgentRepo::create(
            &*db,
            CreateAgent {
                id: "agent-codex".to_owned(),
                name: "Codex".to_owned(),
                description: None,
                executor_type: "codex".to_owned(),
                model: Some("gpt-5".to_owned()),
                reasoning_effort: Some("medium".to_owned()),
                permission_policy: None,
                prompt_template: Some("You are helpful".to_owned()),
                capabilities_json: "[]".to_owned(),
                config_json: "{}".to_owned(),
                daemon_id: None,
                max_concurrent_tasks: 1,
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status: AgentStatus::Idle,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: None,
                visibility: "global".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("codex agent creates");

        AgentRepo::create(
            &*db,
            CreateAgent {
                id: "agent-shell".to_owned(),
                name: "Shell".to_owned(),
                description: None,
                executor_type: "shell".to_owned(),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: "[]".to_owned(),
                config_json: "{}".to_owned(),
                daemon_id: None,
                max_concurrent_tasks: 3,
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status: AgentStatus::Idle,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: None,
                visibility: "global".to_owned(),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("shell agent creates");

        ConversationService::new(db, event_bus)
    }

    #[tokio::test]
    async fn update_conversation_agent_inserts_system_message_and_clears_session() {
        let service = make_service().await;

        let conversation = service
            .create_conversation(
                "project-1".to_owned(),
                "agent-codex".to_owned(),
                Some("Planning".to_owned()),
                None,
            )
            .await
            .expect("conversation creates");

        let updated = service
            .update_conversation(
                conversation.id.clone(),
                conversation.version,
                None,
                Some("agent-shell".to_owned()),
                None,
                None,
            )
            .await
            .expect("conversation updates");

        assert_eq!(updated.agent_id.as_deref(), Some("agent-shell"));
        assert_eq!(updated.agent_session_id, None);

        let messages = service
            .list_messages(
                updated.id.clone(),
                None,
                PageRequest {
                    cursor: None,
                    limit: 20,
                    include_total: false,
                    sort_by: SortBy::CreatedAt,
                    sort_order: SortOrder::Desc,
                },
            )
            .await
            .expect("messages list");

        assert_eq!(messages.items.len(), 1);
        let system = &messages.items[0];
        assert_eq!(system.role, db::ConversationMessageRole::System);
        assert!(system.content.contains("agent_changed"));
        assert!(system.content.contains("agent-codex"));
        assert!(system.content.contains("agent-shell"));
    }

    #[tokio::test]
    async fn send_message_uses_executor_and_completes_assistant_message() {
        let service = make_service().await;
        let execute_calls = Arc::new(AtomicUsize::new(0));
        let executor: Arc<dyn TaskExecutor> = Arc::new(CountingExecutor {
            execute_calls: Arc::clone(&execute_calls),
            worktree_paths: Arc::new(Mutex::new(Vec::new())),
            descriptions: Arc::new(Mutex::new(Vec::new())),
        });

        let conversation = service
            .create_conversation("project-1".to_owned(), "agent-codex".to_owned(), None, None)
            .await
            .expect("conversation creates");

        let (_user, assistant_placeholder) = service
            .send_message(
                conversation.id.clone(),
                "Summarize our current risks".to_owned(),
                None,
                Arc::clone(&executor),
            )
            .await
            .expect("send message succeeds");

        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        assert_eq!(execute_calls.load(Ordering::SeqCst), 1);

        let assistant = ConversationMessageRepo::get_by_id(&*service.db, &assistant_placeholder.id)
            .await
            .expect("assistant load succeeds")
            .expect("assistant exists");
        assert_eq!(assistant.status, ConversationMessageStatus::Complete);
        assert_eq!(assistant.content, "Acknowledged");

        let refreshed = service
            .get_conversation(conversation.id)
            .await
            .expect("conversation reloads");
        assert_eq!(refreshed.agent_session_id.as_deref(), Some("session-1"));
    }

    #[tokio::test]
    async fn send_message_follow_up_uses_resume_session_prompt_only() {
        let service = make_service().await;
        let execute_calls = Arc::new(AtomicUsize::new(0));
        let descriptions = Arc::new(Mutex::new(Vec::new()));
        let executor: Arc<dyn TaskExecutor> = Arc::new(CountingExecutor {
            execute_calls: Arc::clone(&execute_calls),
            worktree_paths: Arc::new(Mutex::new(Vec::new())),
            descriptions: Arc::clone(&descriptions),
        });

        let conversation = service
            .create_conversation("project-1".to_owned(), "agent-codex".to_owned(), None, None)
            .await
            .expect("conversation creates");

        service
            .send_message(
                conversation.id.clone(),
                "First turn".to_owned(),
                None,
                Arc::clone(&executor),
            )
            .await
            .expect("first send succeeds");
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        service
            .send_message(
                conversation.id,
                "Follow up only".to_owned(),
                None,
                Arc::clone(&executor),
            )
            .await
            .expect("follow-up send succeeds");
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        assert_eq!(execute_calls.load(Ordering::SeqCst), 2);
        let descriptions = descriptions.lock().expect("descriptions lock");
        assert_eq!(descriptions.len(), 2);
        assert!(descriptions[0].contains("Current Forge project:"));
        assert!(descriptions[0].contains("First turn"));
        assert_eq!(descriptions[1], "Follow up only");
    }

    #[tokio::test]
    async fn send_message_failure_exposes_error_in_logs_and_clears_missing_thread_session() {
        let service = make_service().await;
        let conversation = service
            .create_conversation("project-1".to_owned(), "agent-codex".to_owned(), None, None)
            .await
            .expect("conversation creates");

        service
            .send_message(
                conversation.id.clone(),
                "First turn".to_owned(),
                None,
                Arc::new(CountingExecutor {
                    execute_calls: Arc::new(AtomicUsize::new(0)),
                    worktree_paths: Arc::new(Mutex::new(Vec::new())),
                    descriptions: Arc::new(Mutex::new(Vec::new())),
                }),
            )
            .await
            .expect("first send succeeds");
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        let with_session = service
            .get_conversation(conversation.id.clone())
            .await
            .expect("conversation reloads");
        assert_eq!(with_session.agent_session_id.as_deref(), Some("session-1"));

        let (_user, assistant_placeholder) = service
            .send_message(
                conversation.id.clone(),
                "Follow up".to_owned(),
                None,
                Arc::new(FailingExecutor {
                    error: "turn/start failed: thread not found: session-1 (-32600)",
                }),
            )
            .await
            .expect("send creates placeholder");
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        let assistant = ConversationMessageRepo::get_by_id(&*service.db, &assistant_placeholder.id)
            .await
            .expect("assistant loads")
            .expect("assistant exists");
        assert_eq!(assistant.status, ConversationMessageStatus::Failed);
        assert!(assistant
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("thread not found"));

        let refreshed = service
            .get_conversation(conversation.id.clone())
            .await
            .expect("conversation reloads");
        assert_eq!(refreshed.agent_session_id, None);

        let logs = service
            .list_log_entries(conversation.id)
            .await
            .expect("logs load");
        assert!(logs.iter().any(|entry| {
            entry.kind == LogKind::Assistant
                && payload_text(&entry.payload)
                    .unwrap_or_default()
                    .contains("thread not found")
        }));
    }

    #[tokio::test]
    async fn send_message_logs_preserve_tool_calls_without_duplicate_prompt_user() {
        let service = make_service().await;
        let conversation = service
            .create_conversation("project-1".to_owned(), "agent-codex".to_owned(), None, None)
            .await
            .expect("conversation creates");

        service
            .send_message(
                conversation.id.clone(),
                "Use a tool".to_owned(),
                None,
                Arc::new(ToolLoggingExecutor),
            )
            .await
            .expect("send succeeds");
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        let logs = service
            .list_log_entries(conversation.id)
            .await
            .expect("logs load");
        let prompt_user_count = logs
            .iter()
            .filter(|entry| {
                entry.kind == LogKind::User
                    && payload_text(&entry.payload).as_deref() == Some("Use a tool")
            })
            .count();

        assert_eq!(prompt_user_count, 1);
        assert!(logs.iter().any(|entry| entry.kind == LogKind::ToolCall));
        assert!(logs.iter().any(|entry| entry.kind == LogKind::ToolResult));
    }

    #[tokio::test]
    async fn list_log_entries_omits_empty_streaming_assistant_placeholder() {
        let service = make_service().await;
        let conversation = service
            .create_conversation("project-1".to_owned(), "agent-codex".to_owned(), None, None)
            .await
            .expect("conversation creates");
        let now = now_rfc3339();
        ConversationMessageRepo::create(
            &*service.db,
            CreateConversationMessage {
                id: "user-follow-up".to_owned(),
                conversation_id: conversation.id.clone(),
                role: ConversationMessageRole::User,
                content: "Follow up".to_owned(),
                status: ConversationMessageStatus::Complete,
                model: None,
                token_usage_json: None,
                duration_ms: None,
                error: None,
                sequence: 1,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("user creates");
        ConversationMessageRepo::create(
            &*service.db,
            CreateConversationMessage {
                id: "assistant-placeholder".to_owned(),
                conversation_id: conversation.id.clone(),
                role: ConversationMessageRole::Assistant,
                content: String::new(),
                status: ConversationMessageStatus::Streaming,
                model: None,
                token_usage_json: None,
                duration_ms: None,
                error: None,
                sequence: 2,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("assistant creates");

        let logs = service
            .list_log_entries(conversation.id)
            .await
            .expect("logs load");

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].kind, LogKind::User);
        assert_eq!(payload_text(&logs[0].payload).as_deref(), Some("Follow up"));
    }

    #[tokio::test]
    async fn send_message_executes_in_project_primary_repo_when_available() {
        let service = make_service().await;
        let repo_dir = tempfile::tempdir().expect("repo dir creates");
        let repo_path = repo_dir.path().to_string_lossy().into_owned();
        let now = now_rfc3339();
        let repo = RepoRepo::create(
            &*service.db,
            CreateRepo {
                id: new_uuid_v4(),
                project_id: "project-1".to_owned(),
                name: "repo".to_owned(),
                remote_url: repo_path.clone(),
                local_path: Some(repo_path.clone()),
                work_mode: WorkMode::DirectMerge,
                default_branch: "main".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("repo creates");
        ProjectRepo::update(
            &*service.db,
            UpdateProject {
                id: "project-1".to_owned(),
                name: None,
                settings: None,
                primary_repo_id: Some(Some(repo.id)),
                paused_at: None,
                updated_at: now,
            },
        )
        .await
        .expect("project primary repo updates");

        let execute_calls = Arc::new(AtomicUsize::new(0));
        let worktree_paths = Arc::new(Mutex::new(Vec::new()));
        let executor: Arc<dyn TaskExecutor> = Arc::new(CountingExecutor {
            execute_calls: Arc::clone(&execute_calls),
            worktree_paths: Arc::clone(&worktree_paths),
            descriptions: Arc::new(Mutex::new(Vec::new())),
        });
        let conversation = service
            .create_conversation("project-1".to_owned(), "agent-codex".to_owned(), None, None)
            .await
            .expect("conversation creates");

        service
            .send_message(
                conversation.id,
                "Is this a React app?".to_owned(),
                None,
                Arc::clone(&executor),
            )
            .await
            .expect("send message succeeds");

        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        assert_eq!(execute_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            worktree_paths
                .lock()
                .expect("worktree paths lock")
                .as_slice(),
            &[repo_path]
        );
    }

    #[tokio::test]
    async fn send_message_with_shell_agent_marks_assistant_as_failed() {
        let service = make_service().await;
        let execute_calls = Arc::new(AtomicUsize::new(0));
        let executor: Arc<dyn TaskExecutor> = Arc::new(CountingExecutor {
            execute_calls: Arc::clone(&execute_calls),
            worktree_paths: Arc::new(Mutex::new(Vec::new())),
            descriptions: Arc::new(Mutex::new(Vec::new())),
        });

        let conversation = service
            .create_conversation("project-1".to_owned(), "agent-shell".to_owned(), None, None)
            .await
            .expect("conversation creates");

        let (_user, assistant_placeholder) = service
            .send_message(
                conversation.id.clone(),
                "What changed?".to_owned(),
                None,
                Arc::clone(&executor),
            )
            .await
            .expect("send message succeeds");

        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        assert_eq!(execute_calls.load(Ordering::SeqCst), 0);

        let assistant = ConversationMessageRepo::get_by_id(&*service.db, &assistant_placeholder.id)
            .await
            .expect("assistant load succeeds")
            .expect("assistant exists");
        assert_eq!(assistant.status, ConversationMessageStatus::Failed);
        assert!(assistant
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("Shell executor does not support conversation mode"));
    }

    #[test]
    fn conversation_prompt_allows_read_only_repo_inspection() {
        let conversation = Conversation {
            id: "conversation-1".to_owned(),
            project_id: "project-1".to_owned(),
            agent_id: Some("agent-codex".to_owned()),
            title: "Chat".to_owned(),
            status: ConversationStatus::Active,
            system_prompt: None,
            message_count: 0,
            last_message_at: None,
            agent_session_id: None,
            version: 1,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        };

        let prompt = build_prompt(&conversation, None, &None, &[], "Is this React?");

        assert!(prompt.contains("Current Forge project:"));
        assert!(prompt.contains("- id: project-1"));
        assert!(!prompt.contains("primary_repo_id"));
        assert!(prompt.contains("may inspect project files with read-only filesystem"));
        assert!(!prompt.contains("Do not run shell commands"));
        assert!(prompt.contains("Do not edit files, stage changes, or commit"));
    }

    #[test]
    fn conversation_executor_config_disables_task_commit_behaviour() {
        let config = build_executor_config(
            "{}",
            "codex",
            Some("gpt-5.3-codex"),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("config builds");

        assert_eq!(config["config"]["permission_policy"], "plan");
        assert_eq!(config["config"]["ask_for_approval"], "never");
        assert_eq!(config["config"]["sandbox"], "read-only");
        assert_eq!(config["config"]["auto_commit"], false);
    }

    #[test]
    fn conversation_executor_config_uses_resume_fallback_prompt_only_when_provided() {
        let config = build_executor_config(
            r#"{"resume_fallback_prompt":"Full reconstructed prompt"}"#,
            "codex",
            Some("gpt-5.3-codex"),
            None,
            None,
            Some("thread-1"),
            Some("Reconstructed history prompt"),
            None,
        )
        .expect("config builds");

        assert_eq!(config["config"]["resume_thread_id"], "thread-1");
        assert_eq!(config["config"]["resume_thread_in_place"], true);
        assert_eq!(
            config["config"]["resume_fallback_prompt"],
            "Reconstructed history prompt"
        );
    }

    #[test]
    fn conversation_executor_config_uses_supervised_claude_permissions() {
        let config = build_executor_config(
            "{}",
            "claude_code",
            Some("claude-sonnet-4-5"),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("config builds");

        assert_eq!(config["config"]["permission_policy"], "supervised");
        assert!(config["config"].get("auto_commit").is_none());
    }
}
