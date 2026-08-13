//! Durable execution loop for Main and Project Agent Chat turns.
//!
//! Chat jobs intentionally do not share the Task worker's workspace contract.
//! The worker claims one FIFO job per responder/scope, renews an expiring
//! lease while the backend is running, and commits the response through the
//! atomic Agent Chat service composite.  A failed adapter call is persisted on
//! the job with a bounded error and a finite retry budget.

use std::{collections::BTreeSet, fmt, path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use db::{
    now_rfc3339, Agent, AgentChatMessage, AgentChatMessageAuthorType, AgentChatMessageListQuery,
    AgentChatMessageRepo, AgentChatMessageStatus, AgentChatTurnJob, AgentChatTurnJobRepo,
    AgentProfile, AgentProfileRepo, AgentRepo, AgentSession, PageRequest, SqliteDb,
};
use executors::{ExecutionContext, ExecutionOutcome, ExecutionResult, ExecutorKind, TaskExecutor};
use forge_agent_host::RuntimeContextManifestLink;
use forge_agent_host::{
    AgentSessionBackend, AgentTurnRequest, BackendCapabilities, CanonicalScope, CanonicalScopeType,
    Message, NativeProviderConfig, Role, TurnEventSink, WorkspaceAccess,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    agent_chat_policy::guard_agent_chat_content,
    agent_chat_service::{
        AgentChatService, AppendAgentChatSuccessInput, CommittedAgentChatResponse,
    },
    agent_chat_turn_policy::failure_after_claim,
    context_manifest::{ContextManifestInput, ContextManifestService, ContextSourceInput},
    embedded_agent_service::{CreateScopedSession, RequestedCanonicalScope},
    EmbeddedAgentService, Result, ServiceError,
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(30);
const TURN_LEASE_SECONDS: i64 = 120;
const MAX_ACTIVE_TURNS: usize = 32;
const MAX_HISTORY: i64 = 100;
const MAX_ERROR_CHARS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedAgentChatTurn {
    pub identity_id: String,
    pub profile_id: String,
    pub session_id: String,
    pub model: Option<String>,
    pub content: String,
    pub token_usage_json: Option<String>,
    pub duration_ms: i64,
    pub context_manifest_id: Option<String>,
}

struct LoadedAgentChatTurn {
    agent: Agent,
    profile: AgentProfile,
    session: AgentSession,
    input: AgentChatMessage,
    history: Vec<AgentChatMessage>,
    genesis_instruction: Option<String>,
}

#[async_trait]
pub trait AgentChatTurnRunner: Send + Sync {
    async fn run_turn(
        &self,
        job: &AgentChatTurnJob,
        cancellation: CancellationToken,
    ) -> Result<CompletedAgentChatTurn>;
}

/// Narrow legacy CLI adapter for migrated Agent Chats. It deliberately uses a
/// disposable empty directory and advertises denied workspace authority; a
/// Task execution path is not routed through this type.
#[derive(Clone)]
pub struct CliAgentChatSessionBackend {
    executor: Arc<dyn TaskExecutor>,
}

impl fmt::Debug for CliAgentChatSessionBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CliAgentChatSessionBackend")
            .finish_non_exhaustive()
    }
}

impl CliAgentChatSessionBackend {
    pub fn new(executor: Arc<dyn TaskExecutor>) -> Self {
        Self { executor }
    }

    pub fn capabilities() -> BackendCapabilities {
        BackendCapabilities {
            native_runtime: false,
            persistent_session: false,
            protected_checkpoints: false,
            lcm: false,
            cancel: true,
            steer: false,
            workspace: WorkspaceAccess::Deny,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run_turn(
        &self,
        scope: &CanonicalScope,
        job_id: &str,
        chat_id: &str,
        executor_type: &str,
        agent_config: Value,
        prompt: String,
        cancellation: CancellationToken,
    ) -> Result<(ExecutionResult, i64)> {
        if scope.scope_type != CanonicalScopeType::AgentChat
            || scope.scope_id != chat_id
            || scope.workspace_access != WorkspaceAccess::Deny
        {
            return Err(ServiceError::invalid_operation(
                "CLI Agent Chat backend requires a denied-filesystem Agent Chat scope",
            ));
        }
        let kind = executor_type
            .parse::<ExecutorKind>()
            .map_err(ServiceError::invalid_operation)?;
        let executor_type = kind.to_string();
        if matches!(kind, ExecutorKind::Shell | ExecutorKind::Embedded) {
            return Err(ServiceError::invalid_operation(
                "selected executor cannot run a legacy CLI Agent Chat turn",
            ));
        }
        let executor_snapshot = cli_executor_snapshot(&executor_type, agent_config);

        let sandbox = chat_sandbox_path(job_id);
        let logs_path = chat_log_path(job_id);
        if sandbox.exists() {
            std::fs::remove_dir_all(&sandbox).map_err(|_| {
                ServiceError::invalid_operation("stale Agent Chat sandbox could not be removed")
            })?;
        }
        std::fs::create_dir_all(&sandbox).map_err(|_| {
            ServiceError::invalid_operation("Agent Chat sandbox could not be created")
        })?;
        if let Some(parent) = logs_path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| {
                ServiceError::invalid_operation("Agent Chat log directory could not be created")
            })?;
        }

        let started = std::time::Instant::now();
        let execution = self.executor.execute(ExecutionContext {
            task_id: chat_id.to_owned(),
            execution_id: job_id.to_owned(),
            worktree_path: sandbox.to_string_lossy().into_owned(),
            description: prompt,
            agent_config: executor_snapshot,
            logs_path: logs_path.to_string_lossy().into_owned(),
            heartbeat_interval_seconds: 30,
            max_turns: None,
            log_sender: None,
        });
        tokio::pin!(execution);
        let result = tokio::select! {
            result = &mut execution => result,
            _ = cancellation.cancelled() => {
                let _ = self.executor.cancel(job_id).await;
                let _ = std::fs::remove_dir_all(&sandbox);
                return Err(ServiceError::invalid_operation("Agent Chat CLI turn was cancelled"));
            }
        }?;
        let _ = std::fs::remove_dir_all(&sandbox);
        Ok((result, started.elapsed().as_millis() as i64))
    }
}

#[derive(Clone)]
pub struct FederatedAgentChatTurnRunner {
    db: Arc<SqliteDb>,
    embedded_agents: Arc<EmbeddedAgentService>,
    cli_backend: CliAgentChatSessionBackend,
}

impl fmt::Debug for FederatedAgentChatTurnRunner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedAgentChatTurnRunner")
            .finish_non_exhaustive()
    }
}

impl FederatedAgentChatTurnRunner {
    pub fn new(
        db: Arc<SqliteDb>,
        embedded_agents: Arc<EmbeddedAgentService>,
        cli_executor: Arc<dyn TaskExecutor>,
    ) -> Self {
        Self {
            db,
            embedded_agents,
            cli_backend: CliAgentChatSessionBackend::new(cli_executor),
        }
    }

    async fn load_turn(&self, job: &AgentChatTurnJob) -> Result<LoadedAgentChatTurn> {
        let input =
            AgentChatMessageRepo::get_agent_chat_message(&*self.db, &job.triggering_message_id)
                .await?
                .filter(|message| message.chat_id == job.chat_id)
                .ok_or_else(|| {
                    ServiceError::not_found("agent_chat_message", job.triggering_message_id.clone())
                })?;
        let identity_id = job
            .responder_identity_id
            .as_deref()
            .ok_or_else(|| ServiceError::invalid_operation("Agent Chat job has no responder"))?;
        let agent = AgentRepo::get_by_id(&*self.db, identity_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent_identity", identity_id.to_owned()))?;
        let profile_id = job
            .profile_id
            .as_deref()
            .ok_or_else(|| ServiceError::invalid_operation("Agent Chat job has no profile"))?;
        let profile = AgentProfileRepo::get_profile(&*self.db, profile_id)
            .await?
            .filter(|profile| profile.identity_id == agent.id)
            .ok_or_else(|| ServiceError::not_found("agent_profile", profile_id.to_owned()))?;
        let owner_user_id = agent
            .owner_id
            .clone()
            .ok_or_else(|| ServiceError::invalid_operation("Agent identity has no owner"))?;
        let session = self
            .embedded_agents
            .create_or_resume_session(CreateScopedSession {
                actor_user_id: owner_user_id,
                identity_id: agent.id.clone(),
                profile_id: Some(profile.id.clone()),
                scope: RequestedCanonicalScope::AgentChat {
                    chat_id: job.chat_id.clone(),
                },
            })
            .await?;
        let history = AgentChatMessageRepo::list_agent_chat_messages(
            &*self.db,
            AgentChatMessageListQuery {
                chat_id: job.chat_id.clone(),
                before_sequence: Some(input.sequence),
                page: PageRequest {
                    cursor: None,
                    limit: MAX_HISTORY,
                    include_total: false,
                    sort_by: db::SortBy::CreatedAt,
                    sort_order: db::SortOrder::Asc,
                },
            },
        )
        .await?
        .items
        .into_iter()
        .filter(|message| {
            message.sequence < input.sequence && message.status == AgentChatMessageStatus::Complete
        })
        .collect();
        // Genesis instructions are immutable history rows, but only the
        // currently active session is admitted as model context.  Joining the
        // lifecycle table here prevents a cancelled/handed-off protocol from
        // remaining an authority-bearing overlay after restart.
        let active_genesis_instruction = sqlx::query_scalar::<_, String>(
            "SELECT instruction.body
             FROM agent_chat_instruction_revision AS instruction
             JOIN product_genesis_session AS genesis
               ON genesis.id = instruction.source_id
              AND genesis.main_chat_id = instruction.chat_id
             WHERE instruction.chat_id = ?
               AND instruction.source_type = 'native'
               AND genesis.lifecycle IN ('discovering', 'ready_for_project')
             ORDER BY instruction.revision DESC, instruction.id DESC
             LIMIT 1",
        )
        .bind(&job.chat_id)
        .fetch_optional(self.db.pool())
        .await?;
        Ok(LoadedAgentChatTurn {
            agent,
            profile,
            session,
            input,
            history,
            genesis_instruction: active_genesis_instruction,
        })
    }

    async fn run_native(
        &self,
        job: &AgentChatTurnJob,
        turn: LoadedAgentChatTurn,
        cancellation: CancellationToken,
    ) -> Result<CompletedAgentChatTurn> {
        let LoadedAgentChatTurn {
            agent,
            profile,
            session,
            input,
            history,
            genesis_instruction,
        } = turn;
        let owner_user_id = agent
            .owner_id
            .as_deref()
            .ok_or_else(|| ServiceError::invalid_operation("Agent identity has no owner"))?;
        let credential_ref = profile
            .credential_ref
            .as_deref()
            .ok_or_else(|| ServiceError::invalid_operation("Agent profile has no credential"))?;
        let credential = self
            .embedded_agents
            .protected_store()
            .load_credential(credential_ref, owner_user_id)
            .await
            .map_err(|_| ServiceError::invalid_operation("Agent credential is unavailable"))?;
        let config: NativeProfileConfig = serde_json::from_str(&profile.config_json)
            .map_err(|_| ServiceError::invalid_operation("Agent profile config is invalid"))?;
        let runtime_session_id = session
            .runtime_session_id
            .clone()
            .ok_or_else(|| ServiceError::invalid_operation("Agent session has no runtime id"))?;
        let provider = profile
            .provider
            .clone()
            .ok_or_else(|| ServiceError::invalid_operation("Agent profile has no provider"))?;
        let model = profile
            .model
            .clone()
            .ok_or_else(|| ServiceError::invalid_operation("Agent profile has no model"))?;
        let started = std::time::Instant::now();
        let output = self
            .embedded_agents
            .native_backend()
            .run_turn(
                AgentTurnRequest {
                    forge_session_id: session.id.clone(),
                    runtime_session_id,
                    scope: CanonicalScope {
                        scope_type: CanonicalScopeType::AgentChat,
                        scope_id: job.chat_id.clone(),
                        workspace_access: WorkspaceAccess::Deny,
                    },
                    workspace_path: None,
                    provider: NativeProviderConfig {
                        provider,
                        base_url: config.base_url,
                        model: model.clone(),
                        credential,
                        context_tokens: config.context_tokens,
                        max_input_tokens: config.max_input_tokens,
                        max_output_tokens: config.max_output_tokens,
                    },
                    system_prompt: compose_system_prompt(
                        profile.prompt_template.as_deref(),
                        genesis_instruction.as_deref(),
                    ),
                    history: runtime_history(&history),
                    input: input.content,
                    cancellation,
                },
                Arc::new(NoopTurnEventSink),
            )
            .await
            .map_err(|_| ServiceError::invalid_operation("native Agent Chat turn failed"))?;
        let content = output.text.trim().to_owned();
        guard_agent_chat_content(&content)?;
        let context_manifest_id = if let Some(manifest) = output.context_manifest.as_ref() {
            Some(
                self.persist_runtime_context_manifest(job, &agent, &session, manifest)
                    .await?,
            )
        } else {
            None
        };
        Ok(CompletedAgentChatTurn {
            identity_id: agent.id,
            profile_id: profile.id,
            session_id: session.id,
            model: Some(model),
            content,
            token_usage_json: Some(
                serde_json::json!({
                    "input": output.input_tokens,
                    "output": output.output_tokens,
                })
                .to_string(),
            ),
            duration_ms: started.elapsed().as_millis() as i64,
            context_manifest_id,
        })
    }

    /// Persist only the final runtime manifest's redaction-safe linkage. The
    /// runtime remains the owner of context ordering and bodies; Forge stores
    /// identifiers, revisions, counts and fingerprints and links the result
    /// to the canonical Agent Chat session before the response is admitted.
    async fn persist_runtime_context_manifest(
        &self,
        job: &AgentChatTurnJob,
        agent: &Agent,
        session: &AgentSession,
        runtime_manifest: &RuntimeContextManifestLink,
    ) -> Result<String> {
        let identity_id = uuid::Uuid::parse_str(&agent.id)
            .map_err(|_| ServiceError::invalid_operation("Agent identity id is invalid"))?;
        let context_scope_id = uuid::Uuid::parse_str(&session.context_scope_id).map_err(|_| {
            ServiceError::invalid_operation("Agent Chat context scope id is invalid")
        })?;
        let manifest_id = agent_chat_manifest_id(&agent.id, &session.id, runtime_manifest);
        let request_fingerprint = agent_chat_request_fingerprint(job, session, runtime_manifest);
        let sources = runtime_manifest_sources(runtime_manifest);
        let service = ContextManifestService::new(Arc::clone(&self.db));

        if let Some(existing) = service
            .get_authorized(manifest_id, identity_id, context_scope_id)
            .await?
        {
            if existing.runtime_manifest_fingerprint.as_deref()
                != Some(runtime_manifest.runtime_manifest_fingerprint.as_str())
                || existing.request_fingerprint != request_fingerprint
            {
                return Err(ServiceError::invalid_operation(
                    "Agent Chat runtime context manifest idempotency conflict",
                ));
            }
            let existing_sources = service.sources(manifest_id).await?;
            for source in &sources {
                if existing_sources.iter().any(|stored| {
                    stored.ordinal == source.ordinal
                        && stored.source_id == source.source_id
                        && stored.source_revision == source.source_revision
                }) {
                    continue;
                }
                service.append_source(manifest_id, source.clone()).await?;
            }
            return Ok(manifest_id.to_string());
        }

        let created = service
            .create(
                ContextManifestInput {
                    id: manifest_id,
                    identity_id,
                    agent_session_id: Some(uuid::Uuid::parse_str(&session.id).map_err(|_| {
                        ServiceError::invalid_operation("Agent Chat session id is invalid")
                    })?),
                    context_scope_id,
                    scope_type: "agent_chat".to_owned(),
                    scope_id: job.chat_id.clone(),
                    policy_revision: "forge-agent-chat-context-policy-1".to_owned(),
                    domain_revision: "forge-agent-chat-runtime-link-1".to_owned(),
                    lcm_binding_revision: runtime_manifest.lcm_binding_revision.clone(),
                    runtime_manifest_id: Some(runtime_manifest.turn_id.clone()),
                    runtime_manifest_fingerprint: Some(
                        runtime_manifest.runtime_manifest_fingerprint.clone(),
                    ),
                    request_fingerprint,
                },
                &sources,
            )
            .await?;
        let created_id = uuid::Uuid::parse_str(&created.id).map_err(|_| {
            ServiceError::invalid_operation("persisted context manifest id is invalid")
        })?;
        for source in sources {
            service.append_source(created_id, source).await?;
        }
        Ok(created.id)
    }

    async fn run_cli(
        &self,
        job: &AgentChatTurnJob,
        turn: LoadedAgentChatTurn,
        cancellation: CancellationToken,
    ) -> Result<CompletedAgentChatTurn> {
        let LoadedAgentChatTurn {
            agent,
            profile,
            session,
            input,
            history,
            genesis_instruction,
        } = turn;
        let prompt = build_cli_prompt(
            profile.prompt_template.as_deref(),
            genesis_instruction.as_deref(),
            &history,
            &input.content,
        );
        let config: Value = serde_json::from_str(&profile.config_json)
            .map_err(|_| ServiceError::invalid_operation("Agent profile config is invalid"))?;
        let scope = CanonicalScope {
            scope_type: CanonicalScopeType::AgentChat,
            scope_id: job.chat_id.clone(),
            workspace_access: WorkspaceAccess::Deny,
        };
        let (result, duration_ms) = self
            .cli_backend
            .run_turn(
                &scope,
                &job.id,
                &job.chat_id,
                &profile.executor_type,
                config,
                prompt,
                cancellation,
            )
            .await?;
        let content = cli_result_content(result)?;
        guard_agent_chat_content(&content)?;
        Ok(CompletedAgentChatTurn {
            identity_id: agent.id,
            profile_id: profile.id,
            session_id: session.id,
            model: profile.model,
            content,
            token_usage_json: None,
            duration_ms,
            context_manifest_id: None,
        })
    }
}

#[async_trait]
impl AgentChatTurnRunner for FederatedAgentChatTurnRunner {
    async fn run_turn(
        &self,
        job: &AgentChatTurnJob,
        cancellation: CancellationToken,
    ) -> Result<CompletedAgentChatTurn> {
        let turn = self.load_turn(job).await?;
        let backend_kind = turn.profile.backend_kind.clone();
        match backend_kind.as_str() {
            "native" => self.run_native(job, turn, cancellation).await,
            "cli" => self.run_cli(job, turn, cancellation).await,
            _ => Err(ServiceError::invalid_operation(
                "selected Agent Chat backend is unsupported",
            )),
        }
    }
}

#[derive(Clone)]
pub struct AgentChatTurnWorker {
    db: Arc<SqliteDb>,
    chat_service: Arc<AgentChatService<SqliteDb>>,
    runner: Arc<dyn AgentChatTurnRunner>,
    lease_owner: String,
}

impl fmt::Debug for AgentChatTurnWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentChatTurnWorker")
            .field("lease_owner", &self.lease_owner)
            .finish_non_exhaustive()
    }
}

impl AgentChatTurnWorker {
    pub fn new(
        db: Arc<SqliteDb>,
        embedded_agents: Arc<EmbeddedAgentService>,
        cli_executor: Arc<dyn TaskExecutor>,
    ) -> Self {
        let runner = Arc::new(FederatedAgentChatTurnRunner::new(
            Arc::clone(&db),
            embedded_agents,
            cli_executor,
        ));
        Self::with_runner(db, runner)
    }

    pub fn with_runner(db: Arc<SqliteDb>, runner: Arc<dyn AgentChatTurnRunner>) -> Self {
        Self {
            chat_service: Arc::new(AgentChatService::new(Arc::clone(&db))),
            db,
            runner,
            lease_owner: format!("agent-chat-worker:{}", db::new_uuid_v4()),
        }
    }

    pub fn start(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let cancellation = CancellationToken::new();
            let mut active = tokio::task::JoinSet::new();
            let mut poll = tokio::time::interval(POLL_INTERVAL);
            poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow_and_update() { break; }
                    }
                    _ = poll.tick(), if active.len() < MAX_ACTIVE_TURNS => {
                        match self.claim_available(MAX_ACTIVE_TURNS - active.len()).await {
                            Ok(jobs) => for job in jobs {
                                let worker = Arc::clone(&self);
                                let token = cancellation.child_token();
                                active.spawn(async move { worker.process_claimed(job, token).await; });
                            },
                            Err(error) => tracing::warn!(error = %error, "Agent Chat turn polling failed"),
                        }
                    }
                    Some(result) = active.join_next(), if !active.is_empty() => {
                        if let Err(error) = result {
                            tracing::warn!(error = %error, "Agent Chat worker task stopped unexpectedly");
                        }
                    }
                }
            }
            cancellation.cancel();
            while let Some(result) = active.join_next().await {
                if let Err(error) = result {
                    tracing::warn!(error = %error, "Agent Chat worker task stopped during shutdown");
                }
            }
        })
    }

    pub async fn run_once(&self) -> Result<usize> {
        self.recover_expired().await?;
        let jobs = self.claim_available(1).await?;
        let count = jobs.len();
        for job in jobs {
            self.process_claimed(job, CancellationToken::new()).await;
        }
        Ok(count)
    }

    async fn claim_available(&self, capacity: usize) -> Result<Vec<AgentChatTurnJob>> {
        let mut jobs = Vec::with_capacity(capacity);
        self.recover_expired().await?;
        for _ in 0..capacity {
            let Some(job) = self.claim_one().await? else {
                break;
            };
            jobs.push(job);
        }
        Ok(jobs)
    }

    async fn claim_one(&self) -> Result<Option<AgentChatTurnJob>> {
        let now = now_rfc3339();
        let leased_until = lease_deadline();
        let mut transaction = self.db.pool().begin().await?;
        let id = sqlx::query_scalar::<_, String>(
            "WITH candidate AS (
                 SELECT job.id
                 FROM agent_chat_turn_job AS job
                 WHERE job.status IN ('queued', 'retry_wait')
                   AND job.attempt_count < job.max_attempts
                   AND (job.next_attempt_at IS NULL OR job.next_attempt_at <= ?)
                   AND NOT EXISTS (
                       SELECT 1 FROM agent_chat_turn_job AS prior
                       WHERE prior.id <> job.id
                         AND prior.responder_identity_id = job.responder_identity_id
                         AND prior.canonical_scope_id = job.canonical_scope_id
                         AND prior.status IN ('queued', 'leased', 'retry_wait')
                         AND (prior.created_at < job.created_at
                              OR (prior.created_at = job.created_at AND prior.id < job.id))
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM agent_chat_turn_job AS active
                       WHERE active.chat_id = job.chat_id AND active.status = 'leased'
                   )
                 ORDER BY job.created_at ASC, job.id ASC
                 LIMIT 1
             )
             UPDATE agent_chat_turn_job
             SET status = 'leased', lease_owner = ?, leased_until = ?,
                 attempt_count = attempt_count + 1, next_attempt_at = NULL,
                 version = version + 1, updated_at = ?
             WHERE id = (SELECT id FROM candidate)
               AND status IN ('queued', 'retry_wait')
             RETURNING id",
        )
        .bind(&now)
        .bind(&self.lease_owner)
        .bind(&leased_until)
        .bind(&now)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let Some(id) = id else { return Ok(None) };
        AgentChatTurnJobRepo::get_agent_chat_turn_job(&*self.db, &id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent_chat_turn_job", id))
            .map(Some)
    }

    async fn recover_expired(&self) -> Result<()> {
        let now = now_rfc3339();
        let expired = sqlx::query(
            "SELECT id, attempt_count, max_attempts
             FROM agent_chat_turn_job
             WHERE status = 'leased' AND leased_until IS NOT NULL AND leased_until <= ?
             ORDER BY created_at ASC, id ASC",
        )
        .bind(&now)
        .fetch_all(self.db.pool())
        .await?;
        let decision_time = Utc::now();
        for row in expired {
            let id: String = row.try_get("id")?;
            let attempt_count: i64 = row.try_get("attempt_count")?;
            let max_attempts: i64 = row.try_get("max_attempts")?;
            let decision = failure_after_claim(
                attempt_count,
                max_attempts,
                decision_time,
                "Agent Chat lease expired",
            );
            let status = match decision.status {
                api_types::AgentChatTurnStatus::Failed => "failed",
                _ => "retry_wait",
            };
            sqlx::query(
                "UPDATE agent_chat_turn_job
                 SET status = ?, lease_owner = NULL, leased_until = NULL,
                     next_attempt_at = ?, error_code = 'lease_expired',
                     error_message = ?, version = version + 1, updated_at = ?
                 WHERE id = ? AND status = 'leased' AND leased_until IS NOT NULL
                   AND leased_until <= ?",
            )
            .bind(status)
            .bind(decision.next_attempt_at.map(|value| value.to_rfc3339()))
            .bind(decision.error)
            .bind(&now)
            .bind(id)
            .bind(&now)
            .execute(self.db.pool())
            .await?;
        }
        Ok(())
    }

    async fn process_claimed(&self, job: AgentChatTurnJob, cancellation: CancellationToken) {
        let stop = CancellationToken::new();
        let turn_cancellation = cancellation.child_token();
        let renewal =
            self.spawn_lease_renewal(job.id.clone(), stop.clone(), turn_cancellation.clone());
        let result = self.runner.run_turn(&job, turn_cancellation).await;
        stop.cancel();
        let _ = renewal.await;
        // Renewal is versioned. Re-read after the backend stops so a long
        // native turn commits against the current lease version rather than
        // the snapshot that was originally claimed.
        let commit_job = AgentChatTurnJobRepo::get_agent_chat_turn_job(&*self.db, &job.id)
            .await
            .ok()
            .flatten()
            .unwrap_or(job.clone());
        match result {
            Ok(turn) => {
                if let Err(error) = self.commit_success(&commit_job, turn).await {
                    tracing::warn!(job_id = %commit_job.id, error = %error, "Agent Chat response commit failed");
                    let _ = self
                        .chat_service
                        .append_failure(
                            &commit_job,
                            &self.lease_owner,
                            "response_commit_failed",
                            "Agent Chat response could not be committed",
                        )
                        .await;
                }
            }
            Err(error) => {
                let code = classify_turn_error(&error);
                let message = bounded_error_message(&error.to_string());
                if let Err(commit_error) = self
                    .chat_service
                    .append_failure(&commit_job, &self.lease_owner, code, &message)
                    .await
                {
                    tracing::warn!(job_id = %commit_job.id, error = %commit_error, "Agent Chat failure could not be persisted");
                }
            }
        }
    }

    async fn commit_success(
        &self,
        job: &AgentChatTurnJob,
        turn: CompletedAgentChatTurn,
    ) -> Result<CommittedAgentChatResponse> {
        self.chat_service
            .append_success(
                job,
                &self.lease_owner,
                AppendAgentChatSuccessInput {
                    content: turn.content,
                    model: turn.model,
                    session_id: Some(turn.session_id),
                    context_manifest_id: turn.context_manifest_id,
                    token_usage_json: turn.token_usage_json,
                    duration_ms: Some(turn.duration_ms),
                },
            )
            .await
    }

    fn spawn_lease_renewal(
        &self,
        job_id: String,
        stop: CancellationToken,
        turn_cancellation: CancellationToken,
    ) -> JoinHandle<()> {
        let db = Arc::clone(&self.db);
        let owner = self.lease_owner.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(LEASE_RENEW_INTERVAL);
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = interval.tick() => {
                        let now = now_rfc3339();
                        let result = sqlx::query(
                            "UPDATE agent_chat_turn_job
                             SET leased_until = ?, version = version + 1, updated_at = ?
                             WHERE id = ? AND status = 'leased' AND lease_owner = ?",
                        )
                        .bind(lease_deadline())
                        .bind(&now)
                        .bind(&job_id)
                        .bind(&owner)
                        .execute(db.pool())
                        .await;
                        if result.map(|result| result.rows_affected() == 0).unwrap_or(true) {
                            turn_cancellation.cancel();
                            break;
                        }
                    }
                }
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct NativeProfileConfig {
    base_url: String,
    #[serde(default = "default_context_tokens")]
    context_tokens: u32,
    #[serde(default = "default_max_input_tokens")]
    max_input_tokens: u32,
    #[serde(default = "default_max_output_tokens")]
    max_output_tokens: u32,
}

fn default_context_tokens() -> u32 {
    128_000
}

fn default_max_input_tokens() -> u32 {
    96_000
}

fn default_max_output_tokens() -> u32 {
    16_000
}

fn runtime_history(history: &[AgentChatMessage]) -> Vec<Message> {
    history
        .iter()
        .map(|message| match message.author_type {
            AgentChatMessageAuthorType::User | AgentChatMessageAuthorType::Handoff => {
                Message::user(message.content.clone())
            }
            AgentChatMessageAuthorType::Agent => {
                Message::text(Role::Assistant, message.content.clone())
            }
            AgentChatMessageAuthorType::System => Message::system(message.content.clone()),
        })
        .collect()
}

fn build_cli_prompt(
    profile_prompt: Option<&str>,
    genesis_instruction: Option<&str>,
    history: &[AgentChatMessage],
    input: &str,
) -> String {
    let mut sections = Vec::new();
    if let Some(prompt) = profile_prompt.filter(|value| !value.trim().is_empty()) {
        sections.push(prompt.trim().to_owned());
    }
    if let Some(instruction) = genesis_instruction.filter(|value| !value.trim().is_empty()) {
        sections.push(format!(
            "Active Product Genesis instruction (immutable revision):\n{}",
            instruction.trim()
        ));
    }
    sections.push(
        "This is an Agent Chat turn with no Task Workspace authority. Do not read or modify repositories or files. Planning and scope-authorized typed proposals are not Workspace access: a Project Agent may still propose Tasks for its own Project when that scoped tool is available, while a Main Agent may not. Never claim a mutation occurred unless a tool result confirms it."
            .to_owned(),
    );
    sections.push("Authorized Agent Chat history:".to_owned());
    for message in history {
        let role = match message.author_type {
            AgentChatMessageAuthorType::Agent => "assistant",
            AgentChatMessageAuthorType::System => "system",
            _ => "user",
        };
        sections.push(format!("{role}: {}", message.content));
    }
    sections.push(format!("user: {input}"));
    sections.join("\n\n")
}

fn compose_system_prompt(
    profile_prompt: Option<&str>,
    genesis_instruction: Option<&str>,
) -> Option<String> {
    let mut sections = Vec::new();
    if let Some(prompt) = profile_prompt.filter(|value| !value.trim().is_empty()) {
        sections.push(prompt.trim().to_owned());
    }
    if let Some(instruction) = genesis_instruction.filter(|value| !value.trim().is_empty()) {
        sections.push(format!(
            "Active Product Genesis instruction (immutable revision):\n{}",
            instruction.trim()
        ));
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

fn agent_chat_manifest_id(
    identity_id: &str,
    session_id: &str,
    runtime_manifest: &RuntimeContextManifestLink,
) -> uuid::Uuid {
    let mut digest = Sha256::new();
    digest.update(b"forge-agent-chat-context-manifest-v1\0");
    digest.update(identity_id.as_bytes());
    digest.update([0]);
    digest.update(session_id.as_bytes());
    digest.update([0]);
    digest.update(runtime_manifest.turn_id.as_bytes());
    let bytes = digest.finalize();
    let mut id = [0_u8; 16];
    id.copy_from_slice(&bytes[..16]);
    id[6] = (id[6] & 0x0f) | 0x50;
    id[8] = (id[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(id)
}

fn agent_chat_request_fingerprint(
    job: &AgentChatTurnJob,
    session: &AgentSession,
    runtime_manifest: &RuntimeContextManifestLink,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"forge-agent-chat-runtime-request-v1\0");
    for value in [
        job.chat_id.as_str(),
        job.triggering_message_id.as_str(),
        job.correlation_id.as_str(),
        &job.causation_depth.to_string(),
        session.id.as_str(),
        session.context_scope_id.as_str(),
        runtime_manifest.turn_id.as_str(),
        runtime_manifest.context_fingerprint.as_str(),
        runtime_manifest.cache_plan_fingerprint.as_str(),
        runtime_manifest.runtime_manifest_fingerprint.as_str(),
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

fn runtime_manifest_sources(
    runtime_manifest: &RuntimeContextManifestLink,
) -> Vec<ContextSourceInput> {
    let source_revision = runtime_manifest.context_fingerprint.clone();
    let covered = runtime_manifest
        .summaries
        .iter()
        .flat_map(|summary| summary.covered.iter().cloned())
        .collect::<BTreeSet<_>>();
    let summary_ids = runtime_manifest
        .summaries
        .iter()
        .map(|summary| summary.summary.clone())
        .collect::<BTreeSet<_>>();
    let segment_ids = runtime_manifest
        .segments
        .iter()
        .map(|segment| segment.id.clone())
        .collect::<BTreeSet<_>>();
    let mut sources = Vec::new();
    let mut source_ids = BTreeSet::new();
    let mut ordinal = 0_i64;
    let mut push = |source_id: String,
                    source_type: String,
                    source_revision: String,
                    selection_reason: String,
                    disposition: String,
                    retention_priority: i64,
                    fragment_fingerprint: String,
                    sensitivity: String| {
        if source_ids.insert(source_id.clone()) {
            sources.push(ContextSourceInput {
                ordinal,
                source_id,
                source_type,
                source_revision,
                selection_reason,
                disposition,
                retention_priority,
                fragment_fingerprint,
                sensitivity,
            });
            ordinal = ordinal.saturating_add(1);
        }
    };

    if let Some(timeline_id) = runtime_manifest.lcm_timeline_id.as_deref() {
        push(
            timeline_id.to_owned(),
            "runtime_lcm_timeline".to_owned(),
            runtime_manifest
                .lcm_binding_revision
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
            "agent_runtime_lcm_binding".to_owned(),
            "included".to_owned(),
            100,
            fingerprint_id(timeline_id),
            "internal".to_owned(),
        );
    }
    for segment in &runtime_manifest.segments {
        push(
            segment.id.clone(),
            "runtime_segment".to_owned(),
            source_revision.clone(),
            "agent_runtime_final_segment".to_owned(),
            if covered.contains(&segment.id) && !summary_ids.contains(&segment.id) {
                "summarized".to_owned()
            } else {
                "included".to_owned()
            },
            if summary_ids.contains(&segment.id) {
                100
            } else {
                10
            },
            segment.content_hash.clone(),
            segment.sensitivity.clone(),
        );
    }
    for summary in &runtime_manifest.summaries {
        push(
            summary.summary.clone(),
            "runtime_lcm_summary".to_owned(),
            source_revision.clone(),
            "agent_runtime_summary_coverage".to_owned(),
            "included".to_owned(),
            100,
            fingerprint_id(&summary.summary),
            "sensitive".to_owned(),
        );
        for covered_id in &summary.covered {
            push(
                covered_id.clone(),
                "runtime_lcm_covered".to_owned(),
                source_revision.clone(),
                "agent_runtime_summary_coverage".to_owned(),
                "summarized".to_owned(),
                10,
                fingerprint_id(covered_id),
                "sensitive".to_owned(),
            );
        }
    }
    for summary in &runtime_manifest.lossless_summaries {
        let source_id = summary.node_id.clone();
        push(
            source_id.clone(),
            "runtime_lossless_summary".to_owned(),
            summary.node_revision.to_string(),
            "agent_runtime_lossless_summary".to_owned(),
            "included".to_owned(),
            100,
            summary
                .operation_fingerprint
                .clone()
                .unwrap_or_else(|| summary.source_fingerprint.clone()),
            summary.classification.sensitivity.clone(),
        );
    }
    // Keep the local variable meaningful in the no-summary case and make the
    // dedupe rule explicit for reviewers: source IDs are never repeated.
    let _ = segment_ids;
    sources
}

fn fingerprint_id(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn cli_result_content(result: ExecutionResult) -> Result<String> {
    match result.status {
        ExecutionOutcome::Completed => result
            .summary
            .filter(|summary| !summary.trim().is_empty())
            .ok_or_else(|| ServiceError::invalid_operation("Agent Chat CLI returned no content")),
        ExecutionOutcome::Cancelled => Err(ServiceError::invalid_operation(
            "Agent Chat CLI turn was cancelled",
        )),
        ExecutionOutcome::Failed => Err(ServiceError::invalid_operation(
            "Agent Chat CLI turn failed",
        )),
    }
}

fn cli_executor_snapshot(executor_type: &str, config: Value) -> Value {
    serde_json::json!({
        "executor_type": executor_type,
        "config": config,
    })
}

fn lease_deadline() -> String {
    (Utc::now() + ChronoDuration::seconds(TURN_LEASE_SECONDS)).to_rfc3339()
}

fn chat_sandbox_path(job_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("forge-agent-chat-sandboxes")
        .join(job_id)
}

fn chat_log_path(job_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("forge-agent-chat-logs")
        .join(format!("{job_id}.jsonl"))
}

fn bounded_error_message(value: &str) -> String {
    value.chars().take(MAX_ERROR_CHARS).collect()
}

fn classify_turn_error(error: &ServiceError) -> &'static str {
    let text = error.to_string().to_ascii_lowercase();
    if text.contains("credential") {
        "credential_unavailable"
    } else if text.contains("cancel") {
        "cancelled"
    } else if text.contains("scope") || text.contains("permission") || text.contains("binding") {
        "authority_denied"
    } else if text.contains("profile") || text.contains("config") {
        "configuration_invalid"
    } else {
        "backend_failed"
    }
}

#[derive(Debug)]
struct NoopTurnEventSink;

#[async_trait]
impl TurnEventSink for NoopTurnEventSink {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_sandbox_is_job_scoped() {
        assert!(chat_sandbox_path("job-a") != chat_sandbox_path("job-b"));
        assert!(chat_log_path("job-a") != chat_log_path("job-b"));
    }

    #[test]
    fn cli_chat_wraps_profile_config_in_executor_snapshot() {
        let snapshot = cli_executor_snapshot(
            "smith",
            serde_json::json!({"profile": "luna", "approval": "deny"}),
        );
        assert_eq!(snapshot["executor_type"], "smith");
        assert_eq!(snapshot["config"]["profile"], "luna");
        assert_eq!(snapshot["config"]["approval"], "deny");
    }

    #[test]
    fn errors_are_bounded_and_classified_without_body_leak() {
        let error = ServiceError::invalid_operation("x".repeat(2048));
        assert_eq!(
            bounded_error_message(&error.to_string()).chars().count(),
            MAX_ERROR_CHARS
        );
        assert_eq!(
            classify_turn_error(&ServiceError::invalid_operation("credential unavailable")),
            "credential_unavailable"
        );
    }

    #[test]
    fn active_genesis_instruction_is_added_to_both_backend_contexts() {
        let instruction = "Product Genesis protocol v1\nAsk at most two questions.";
        let system = compose_system_prompt(Some("profile rules"), Some(instruction))
            .expect("an active instruction produces system context");
        assert!(system.contains("profile rules"));
        assert!(system.contains("Active Product Genesis instruction"));
        assert!(system.contains(instruction));

        let prompt = build_cli_prompt(
            Some("profile rules"),
            Some(instruction),
            &[],
            "continue discovery",
        );
        assert!(prompt.contains("Active Product Genesis instruction"));
        assert!(prompt.contains("continue discovery"));
    }

    #[test]
    fn terminal_genesis_has_no_instruction_overlay() {
        assert!(compose_system_prompt(Some("profile rules"), None)
            .expect("profile prompt remains available")
            .contains("profile rules"));
        assert!(compose_system_prompt(None, None).is_none());
        let prompt = build_cli_prompt(None, None, &[], "ordinary Main message");
        assert!(!prompt.contains("Active Product Genesis instruction"));
    }
}
