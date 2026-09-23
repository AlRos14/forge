use crate::{
    config::merge_overrides, ExecutionContext, ExecutionResult, ExecutorError,
    ResolvedExecutorCandidate, TaskExecutor,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Registry of known CLI executor families.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorKind {
    /// Forge-hosted Agent Runtime profile.  This is intentionally not a CLI
    /// adapter; services route it to the Forge-owned native task backend.
    Embedded,
    Shell,
    Codex,
    ClaudeCode,
    Cursor,
    Opencode,
    Gemini,
    Smith,
    Null,
}

impl std::fmt::Display for ExecutorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Embedded => write!(f, "embedded"),
            Self::Shell => write!(f, "shell"),
            Self::Codex => write!(f, "codex"),
            Self::ClaudeCode => write!(f, "claude_code"),
            Self::Cursor => write!(f, "cursor"),
            Self::Opencode => write!(f, "opencode"),
            Self::Gemini => write!(f, "gemini"),
            Self::Smith => write!(f, "smith"),
            Self::Null => write!(f, "null"),
        }
    }
}

impl std::str::FromStr for ExecutorKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "embedded" => Ok(Self::Embedded),
            "shell" => Ok(Self::Shell),
            "codex" => Ok(Self::Codex),
            "claude_code" => Ok(Self::ClaudeCode),
            "cursor" => Ok(Self::Cursor),
            "opencode" => Ok(Self::Opencode),
            "gemini" => Ok(Self::Gemini),
            "smith" => Ok(Self::Smith),
            "null" => Ok(Self::Null),
            other => Err(format!("unknown executor kind: {other}")),
        }
    }
}

/// Availability state reported by an adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityStatus {
    Authenticated,
    Installed,
    NotFound,
}

/// Availability info returned by an adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailabilityInfo {
    pub status: AvailabilityStatus,
    pub authenticated_at: Option<String>,
    pub config_path: Option<String>,
}

/// Context for adapter discovery.
#[derive(Debug, Clone)]
pub struct DiscoverContext {
    pub project_path: Option<String>,
}

/// Options discovered by an adapter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscoveredOptions {
    pub models: Vec<String>,
    pub permission_policies: Vec<String>,
    pub cli_specific: serde_json::Value,
}

/// Generic policy posture after one adapter interprets its concrete config.
/// Deterministic risk classification and workspace authority remain in Forge
/// core and are not adapter-controlled fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessPolicyInterpretation {
    pub permission_policy: String,
    pub isolation_posture: String,
}

/// Per-execution overrides applied on top of profile config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionOverrides {
    pub model_id: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
}

/// Harness protocol and capability authority for one concrete integration.
#[async_trait]
pub trait HarnessAdapter: Send + Sync {
    fn kind(&self) -> ExecutorKind;

    /// Legacy family-level detector used by adapters whose availability does
    /// not depend on one candidate's normalized configuration.
    fn check_availability(&self) -> AvailabilityInfo {
        AvailabilityInfo {
            status: AvailabilityStatus::NotFound,
            authenticated_at: None,
            config_path: None,
        }
    }

    /// Detect availability without starting a model turn. Detection remains
    /// advisory; typed runtime availability failures may still advance Start
    /// fallback routes.
    fn detect(&self, _config: &serde_json::Value) -> AvailabilityInfo {
        self.check_availability()
    }

    /// Return evidence for the exact normalized candidate configuration.
    /// The default is Unknown for every dimension and therefore fails closed.
    fn capabilities(&self, _config: &serde_json::Value) -> api_types::HarnessCapabilities {
        api_types::HarnessCapabilities::unknown()
    }

    /// Normalize one concrete harness configuration after generic profile and
    /// execution override layers have been merged. Concrete adapters override
    /// this with their own typed config.
    fn normalize_config(
        &self,
        config: &serde_json::Value,
        overrides: &ExecutionOverrides,
    ) -> Result<serde_json::Value, ExecutorError> {
        let mut normalized = config.clone();
        merge_overrides(&mut normalized, overrides)?;
        Ok(normalized)
    }

    async fn discover_options(
        &self,
        ctx: DiscoverContext,
    ) -> Result<DiscoveredOptions, ExecutorError>;

    /// Run the normalized invocation carried by the generic context. Concrete
    /// protocol translation stays inside the adapter implementation.
    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError>;

    async fn start(&self, mut ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        ctx.invocation = api_types::HarnessInvocation::Start;
        self.execute(ctx).await
    }

    async fn resume(
        &self,
        mut ctx: ExecutionContext,
        external_session_id: &str,
    ) -> Result<ExecutionResult, ExecutorError> {
        let support = self.capabilities(&ctx.agent_config).resume;
        if !support.is_available() {
            return Err(ExecutorError::UnsupportedCapability {
                capability: "resume".to_owned(),
                support,
            });
        }
        ctx.invocation = api_types::HarnessInvocation::Resume {
            external_session_id: external_session_id.to_owned(),
        };
        self.execute(ctx).await
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError>;

    /// Optional account/quota observation, separate from per-execution token
    /// usage. It must not start a model turn.
    async fn observe_usage(
        &self,
        _config: &serde_json::Value,
        _cancel: CancellationToken,
    ) -> Result<Option<UsageObservation>, ExecutorError> {
        Ok(None)
    }

    fn executable_name(&self) -> Option<String> {
        None
    }

    fn interpret_execution_policy(
        &self,
        config: &serde_json::Value,
    ) -> HarnessPolicyInterpretation {
        let config = config.get("config").unwrap_or(config);
        HarnessPolicyInterpretation {
            permission_policy: config
                .get("permission_policy")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            isolation_posture: "not_applicable".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UsageObservation {
    pub value: serde_json::Value,
    pub source: Option<String>,
}

/// Registry mapping the current ExecutorKind compatibility identifier to the
/// sole HarnessAdapter authority for that candidate.
pub struct HarnessAdapterRegistry {
    adapters: HashMap<ExecutorKind, Box<dyn HarnessAdapter>>,
}

impl HarnessAdapterRegistry {
    pub fn new() -> Self {
        Self {
            adapters: HashMap::new(),
        }
    }

    pub fn register(&mut self, adapter: Box<dyn HarnessAdapter>) {
        let kind = adapter.kind();
        self.adapters.insert(kind, adapter);
    }

    pub fn get(&self, kind: &ExecutorKind) -> Option<&dyn HarnessAdapter> {
        self.adapters.get(kind).map(|a| a.as_ref())
    }

    pub fn kinds(&self) -> Vec<ExecutorKind> {
        self.adapters.keys().cloned().collect()
    }
}

impl Default for HarnessAdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Supervisor-facing executor that dispatches to a typed CLI adapter.
pub struct AdapterExecutor {
    registry: Arc<HarnessAdapterRegistry>,
}

impl AdapterExecutor {
    pub fn new(registry: Arc<HarnessAdapterRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl TaskExecutor for AdapterExecutor {
    async fn execute(&self, mut ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        let (kind, raw_config) = executor_config_pair(&ctx.agent_config)?;
        let adapter = self.registry.get(&kind).ok_or_else(|| {
            ExecutorError::Other(format!("No adapter registered for executor type: {kind}"))
        })?;
        let config = adapter.normalize_config(&raw_config, &ExecutionOverrides::default())?;
        let invocation_config =
            crate::config::with_runtime_environment(&config, &ctx.agent_config)?;
        let capabilities = adapter.capabilities(&config);
        let policy_interpretation = adapter.interpret_execution_policy(&config);
        let effective_policy = crate::effective_policy::from_harness_interpretation(
            &kind,
            &policy_interpretation.permission_policy,
            &policy_interpretation.isolation_posture,
            Some(&ctx.worktree_path),
            None,
            &config,
        );
        let candidate_key = crate::config::candidate_key(&kind, &config);
        let execution_config = config.clone();
        ctx.agent_config = invocation_config;
        let invocation = ctx.invocation.clone();
        let mut result = match invocation {
            api_types::HarnessInvocation::Start => adapter.start(ctx).await?,
            api_types::HarnessInvocation::Resume {
                external_session_id,
            } => adapter.resume(ctx, &external_session_id).await?,
        };
        result.resolved_candidate = Some(ResolvedExecutorCandidate {
            candidate_key,
            executor_type: kind,
            config: execution_config,
            harness_capabilities: capabilities,
            effective_policy,
        });
        Ok(result)
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        for kind in self.registry.kinds() {
            if let Some(adapter) = self.registry.get(&kind) {
                adapter.cancel(execution_id).await?;
            }
        }
        Ok(())
    }

    async fn observe_usage(
        &self,
        kind: ExecutorKind,
        config: &serde_json::Value,
        cancel: CancellationToken,
    ) -> Result<Option<UsageObservation>, ExecutorError> {
        observe_usage_with_registry(&self.registry, kind, config, cancel).await
    }
}

/// Cooldown applied to an exhausted account when the provider gives no
/// retry-after hint.
pub const DEFAULT_ACCOUNT_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Supervisor-facing executor that walks an ordered candidate route,
/// advancing only on availability failures. Snapshots without a `routing`
/// block behave exactly like `AdapterExecutor`.
pub struct FallbackExecutor {
    registry: Arc<HarnessAdapterRegistry>,
    cooldowns: std::sync::Mutex<HashMap<String, std::time::Instant>>,
    cancellations: std::sync::Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
}

struct RouteCandidate {
    kind: ExecutorKind,
    config: serde_json::Value,
    candidate_key: String,
    account_key: String,
    harness_capabilities: api_types::HarnessCapabilities,
    effective_policy: api_types::EffectiveExecutionPolicy,
}

impl FallbackExecutor {
    pub fn new(registry: Arc<HarnessAdapterRegistry>) -> Self {
        Self {
            registry,
            cooldowns: std::sync::Mutex::new(HashMap::new()),
            cancellations: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The preferred candidate is the snapshot's top-level pair (the launch
    /// path points it at the sticky winner); remaining route candidates
    /// follow in configured order.
    fn route(
        &self,
        ctx: &ExecutionContext,
        include_fallbacks: bool,
    ) -> Result<Vec<RouteCandidate>, ExecutorError> {
        let (preferred_kind, raw_preferred_config) = executor_config_pair(&ctx.agent_config)?;
        let preferred_adapter = self.registry.get(&preferred_kind).ok_or_else(|| {
            ExecutorError::Other(format!("No adapter registered for executor type: {preferred_kind}"))
        })?;
        let preferred_config = preferred_adapter
            .normalize_config(&raw_preferred_config, &ExecutionOverrides::default())?;
        let preferred = RouteCandidate {
            candidate_key: crate::config::candidate_key(&preferred_kind, &preferred_config),
            account_key: crate::config::account_key(&preferred_kind, &preferred_config),
            harness_capabilities: preferred_adapter.capabilities(&preferred_config),
            effective_policy: {
                let interpretation = preferred_adapter.interpret_execution_policy(&preferred_config);
                crate::effective_policy::from_harness_interpretation(
                    &preferred_kind,
                    &interpretation.permission_policy,
                    &interpretation.isolation_posture,
                    Some(&ctx.worktree_path),
                    None,
                    &preferred_config,
                )
            },
            kind: preferred_kind,
            config: preferred_config,
        };

        let mut candidates = vec![preferred];
        if !include_fallbacks {
            return Ok(candidates);
        }
        let routing = ctx.agent_config.get(crate::config::ROUTING_SNAPSHOT_KEY);
        if let Some(routing) = routing {
            let routing: crate::config::ExecutorRouting = serde_json::from_value(routing.clone())
                .map_err(|error| {
                ExecutorError::Other(format!("invalid routing block in snapshot: {error}"))
            })?;
            if routing.policy != crate::config::ROUTING_POLICY_ORDERED_FALLBACK_V1 {
                return Err(ExecutorError::Other(format!(
                    "unknown routing policy: {}",
                    routing.policy
                )));
            }
            for candidate in routing.candidates {
                let adapter = self.registry.get(&candidate.executor_type).ok_or_else(|| {
                    ExecutorError::Other(format!(
                        "No adapter registered for executor type: {}",
                        candidate.executor_type
                    ))
                })?;
                let config = adapter
                    .normalize_config(&candidate.config, &ExecutionOverrides::default())?;
                crate::config::validate_same_agent_candidate(
                    &candidates[0].kind,
                    &candidates[0].config,
                    &candidate.executor_type,
                    &config,
                )?;
                let key = crate::config::candidate_key(&candidate.executor_type, &config);
                if candidates.iter().any(|c| c.candidate_key == key) {
                    continue;
                }
                candidates.push(RouteCandidate {
                    account_key: crate::config::account_key(&candidate.executor_type, &config),
                    candidate_key: key,
                    harness_capabilities: adapter.capabilities(&config),
                    effective_policy: {
                        let interpretation = adapter.interpret_execution_policy(&config);
                        crate::effective_policy::from_harness_interpretation(
                            &candidate.executor_type,
                            &interpretation.permission_policy,
                            &interpretation.isolation_posture,
                            Some(&ctx.worktree_path),
                            None,
                            &config,
                        )
                    },
                    kind: candidate.executor_type,
                    config,
                });
            }
        }
        Ok(candidates)
    }

    fn cooldown_remaining(&self, account_key: &str) -> Option<std::time::Duration> {
        let mut cooldowns = self.cooldowns.lock().expect("cooldown lock poisoned");
        let now = std::time::Instant::now();
        match cooldowns.get(account_key) {
            Some(expiry) if *expiry > now => Some(*expiry - now),
            Some(_) => {
                cooldowns.remove(account_key);
                None
            }
            None => None,
        }
    }

    fn note_exhausted(&self, account_key: &str, retry_after: Option<std::time::Duration>) {
        let cooldown = retry_after.unwrap_or(DEFAULT_ACCOUNT_COOLDOWN);
        self.cooldowns
            .lock()
            .expect("cooldown lock poisoned")
            .insert(account_key.to_owned(), std::time::Instant::now() + cooldown);
    }

    fn cancellation_flag(&self, execution_id: &str) -> Arc<std::sync::atomic::AtomicBool> {
        self.cancellations
            .lock()
            .expect("cancellation lock poisoned")
            .entry(execution_id.to_owned())
            .or_default()
            .clone()
    }

    fn clear_cancellation(&self, execution_id: &str) {
        self.cancellations
            .lock()
            .expect("cancellation lock poisoned")
            .remove(execution_id);
    }

    async fn log_hop(
        writer: &mut crate::LogWriter,
        event: &str,
        candidate: &RouteCandidate,
        detail: serde_json::Value,
    ) {
        let mut payload = serde_json::json!({
            "source": "executor_fallback",
            "event": event,
            "candidate_key": candidate.candidate_key,
            "executor_type": candidate.kind.to_string(),
        });
        if let (Some(object), Some(extra)) = (payload.as_object_mut(), detail.as_object()) {
            for (key, value) in extra {
                object.insert(key.clone(), value.clone());
            }
        }
        // Hop logging is best-effort; a full log must not mask the run itself.
        let _ = writer
            .write(crate::LogKind::System, crate::LogStream::Main, payload)
            .await;
    }

    fn unavailable_result(
        attempts: Vec<crate::config::RouteAttempt>,
        usage: Option<crate::TokenUsage>,
        retry_after: Option<std::time::Duration>,
        summary: String,
    ) -> ExecutionResult {
        ExecutionResult {
            status: crate::ExecutionOutcome::Failed,
            error: Some(summary),
            usage,
            failure_class: Some(crate::ExecutionFailureClass::ExecutorUnavailable),
            retry_after,
            route_attempts: attempts,
            ..Default::default()
        }
    }
}

#[async_trait]
impl TaskExecutor for FallbackExecutor {
    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        let is_resume = matches!(&ctx.invocation, api_types::HarnessInvocation::Resume { .. });
        let candidates = self.route(&ctx, !is_resume)?;
        let cancelled = self.cancellation_flag(&ctx.execution_id);
        let single_candidate = candidates.len() == 1;

        let mut writer = crate::LogWriter::new(
            std::path::Path::new(&ctx.logs_path),
            ctx.execution_id.clone(),
            crate::log_writer::DEFAULT_MAX_OUTPUT_BYTES,
        );
        if let Some(sender) = ctx.log_sender.clone() {
            writer.set_log_sender(sender);
        }

        let mut attempts: Vec<crate::config::RouteAttempt> = Vec::new();
        let mut aggregated_usage: Option<crate::TokenUsage> = None;
        let mut earliest_retry: Option<std::time::Duration> = None;
        let mut skip_reasons: Vec<String> = Vec::new();

        let absorb = |aggregated: &mut Option<crate::TokenUsage>,
                      usage: &Option<crate::TokenUsage>| {
            if let Some(usage) = usage {
                aggregated
                    .get_or_insert_with(crate::TokenUsage::default)
                    .absorb(usage);
            }
        };

        let outcome = 'chain: {
            for candidate in &candidates {
                if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                    break 'chain Some(ExecutionResult {
                        status: crate::ExecutionOutcome::Cancelled,
                        usage: aggregated_usage.take(),
                        route_attempts: std::mem::take(&mut attempts),
                        ..Default::default()
                    });
                }

                if let Some(remaining) = self.cooldown_remaining(&candidate.account_key) {
                    attempts.push(crate::config::RouteAttempt {
                        candidate_key: candidate.candidate_key.clone(),
                        outcome: crate::config::RouteAttemptOutcome::SkippedCooldown,
                    });
                    earliest_retry = Some(earliest_retry.map_or(remaining, |e| e.min(remaining)));
                    skip_reasons.push(format!(
                        "{} (cooldown {}s)",
                        candidate.candidate_key,
                        remaining.as_secs()
                    ));
                    Self::log_hop(
                        &mut writer,
                        "candidate_skipped_cooldown",
                        candidate,
                        serde_json::json!({"cooldown_remaining_seconds": remaining.as_secs()}),
                    )
                    .await;
                    continue;
                }

                let Some(adapter) = self.registry.get(&candidate.kind) else {
                    break 'chain None; // fall through to the registry error below
                };

                let invocation_config = crate::config::with_runtime_environment(
                    &candidate.config,
                    &ctx.agent_config,
                )?;
                if matches!(
                    adapter.detect(&invocation_config).status,
                    AvailabilityStatus::NotFound
                ) {
                    attempts.push(crate::config::RouteAttempt {
                        candidate_key: candidate.candidate_key.clone(),
                        outcome: crate::config::RouteAttemptOutcome::Unavailable,
                    });
                    skip_reasons.push(format!("{} (not installed)", candidate.candidate_key));
                    Self::log_hop(
                        &mut writer,
                        "candidate_unavailable",
                        candidate,
                        serde_json::json!({}),
                    )
                    .await;
                    continue;
                }

                if !single_candidate {
                    Self::log_hop(
                        &mut writer,
                        "candidate_selected",
                        candidate,
                        serde_json::json!({}),
                    )
                    .await;
                }

                let mut candidate_ctx = ctx.clone();
                candidate_ctx.agent_config = invocation_config;
                let attempt = match candidate_ctx.invocation.clone() {
                    api_types::HarnessInvocation::Start => adapter.start(candidate_ctx).await,
                    api_types::HarnessInvocation::Resume { external_session_id } => {
                        adapter.resume(candidate_ctx, &external_session_id).await
                    }
                };
                writer = crate::LogWriter::new(
                    std::path::Path::new(&ctx.logs_path),
                    ctx.execution_id.clone(),
                    crate::log_writer::DEFAULT_MAX_OUTPUT_BYTES,
                );
                if let Some(sender) = ctx.log_sender.clone() {
                    writer.set_log_sender(sender);
                }

                match attempt {
                    Ok(mut result) => {
                        absorb(&mut aggregated_usage, &result.usage);
                        let was_cancelled = cancelled.load(std::sync::atomic::Ordering::SeqCst)
                            || result.status == crate::ExecutionOutcome::Cancelled;
                        attempts.push(crate::config::RouteAttempt {
                            candidate_key: candidate.candidate_key.clone(),
                            outcome: if was_cancelled {
                                crate::config::RouteAttemptOutcome::Cancelled
                            } else if result.status == crate::ExecutionOutcome::Completed {
                                crate::config::RouteAttemptOutcome::Completed
                            } else {
                                crate::config::RouteAttemptOutcome::Failed
                            },
                        });
                        if was_cancelled {
                            result.status = crate::ExecutionOutcome::Cancelled;
                        } else if result.status == crate::ExecutionOutcome::Failed {
                            result.failure_class = Some(crate::ExecutionFailureClass::TaskFailed);
                        }
                        result.usage = aggregated_usage.take();
                        result.resolved_candidate = Some(crate::ResolvedExecutorCandidate {
                            candidate_key: candidate.candidate_key.clone(),
                            executor_type: candidate.kind.clone(),
                            config: candidate.config.clone(),
                            harness_capabilities: candidate.harness_capabilities.clone(),
                            effective_policy: candidate.effective_policy.clone(),
                        });
                        result.route_attempts = std::mem::take(&mut attempts);
                        break 'chain Some(result);
                    }
                    Err(error) if error.is_availability() => {
                        let (outcome, retry_after, reason) = match &error {
                            ExecutorError::UsageExhausted { retry_after, usage } => {
                                self.note_exhausted(&candidate.account_key, *retry_after);
                                absorb(&mut aggregated_usage, usage);
                                (
                                    crate::config::RouteAttemptOutcome::UsageExhausted,
                                    retry_after.unwrap_or(DEFAULT_ACCOUNT_COOLDOWN),
                                    "usage exhausted".to_owned(),
                                )
                            }
                            ExecutorError::Unavailable(reason) => (
                                crate::config::RouteAttemptOutcome::Unavailable,
                                DEFAULT_ACCOUNT_COOLDOWN,
                                reason.clone(),
                            ),
                            _ => unreachable!("is_availability covers both variants"),
                        };
                        attempts.push(crate::config::RouteAttempt {
                            candidate_key: candidate.candidate_key.clone(),
                            outcome,
                        });
                        earliest_retry =
                            Some(earliest_retry.map_or(retry_after, |e| e.min(retry_after)));
                        skip_reasons.push(format!("{} ({reason})", candidate.candidate_key));
                        Self::log_hop(
                            &mut writer,
                            "candidate_exhausted",
                            candidate,
                            serde_json::json!({
                                "reason": reason,
                                "retry_after_seconds": retry_after.as_secs(),
                            }),
                        )
                        .await;
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            None
        };

        self.clear_cancellation(&ctx.execution_id);

        if let Some(result) = outcome {
            return Ok(result);
        }

        // Registry gap is an infra error, not an availability disposition.
        if let Some(candidate) = candidates
            .iter()
            .find(|c| self.registry.get(&c.kind).is_none())
        {
            return Err(ExecutorError::Other(format!(
                "No adapter registered for executor type: {}",
                candidate.kind
            )));
        }

        let summary = if is_resume {
            format!("exact harness session candidate unavailable; resume was not rerouted: {}", skip_reasons.join(", "))
        } else {
            format!("no executor candidate available: {}", skip_reasons.join(", "))
        };
        Ok(Self::unavailable_result(
            attempts,
            aggregated_usage,
            earliest_retry,
            summary,
        ))
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        if let Some(flag) = self
            .cancellations
            .lock()
            .expect("cancellation lock poisoned")
            .get(execution_id)
        {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        for kind in self.registry.kinds() {
            if let Some(adapter) = self.registry.get(&kind) {
                adapter.cancel(execution_id).await?;
            }
        }
        Ok(())
    }

    async fn observe_usage(
        &self,
        kind: ExecutorKind,
        config: &serde_json::Value,
        cancel: CancellationToken,
    ) -> Result<Option<UsageObservation>, ExecutorError> {
        observe_usage_with_registry(&self.registry, kind, config, cancel).await
    }
}

async fn observe_usage_with_registry(
    registry: &HarnessAdapterRegistry,
    kind: ExecutorKind,
    config: &serde_json::Value,
    cancel: CancellationToken,
) -> Result<Option<UsageObservation>, ExecutorError> {
    let adapter = registry.get(&kind).ok_or_else(|| {
        ExecutorError::Other(format!("No adapter registered for executor type: {kind}"))
    })?;
    let normalized = adapter.normalize_config(config, &ExecutionOverrides::default())?;
    let capabilities = adapter.capabilities(&normalized);
    if !capabilities.account_usage_observation.is_available() {
        return Err(ExecutorError::UnsupportedCapability {
            capability: "account_usage_observation".to_owned(),
            support: capabilities.account_usage_observation,
        });
    }
    adapter.observe_usage(&normalized, cancel).await
}

fn executor_config_pair(
    agent_config: &serde_json::Value,
) -> Result<(ExecutorKind, serde_json::Value), ExecutorError> {
    let object = agent_config.as_object().ok_or_else(|| {
        ExecutorError::Other("executor config snapshot must be a JSON object".to_owned())
    })?;
    let executor_type = object
        .get("executor_type")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            ExecutorError::Other("executor config snapshot missing executor_type".to_owned())
        })?;
    let kind = executor_type.parse::<ExecutorKind>().map_err(|_| {
        ExecutorError::Other(format!(
            "No adapter registered for executor type: {executor_type}"
        ))
    })?;
    let config = object.get("config").unwrap_or(agent_config).clone();
    Ok((kind, config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_shell_command_plan, ExecutionOutcome, ExecutionResult, ShellConfig};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn resolve_context_config(
        snapshot: &serde_json::Value,
    ) -> Result<(ExecutorKind, serde_json::Value), ExecutorError> {
        let (kind, raw_config) = executor_config_pair(snapshot)?;
        let config = crate::config::resolve_config_value(
            kind.clone(),
            &raw_config,
            &ExecutionOverrides::default(),
        )?;
        Ok((kind, config))
    }

    struct CapturingAdapter;

    #[async_trait]
    impl HarnessAdapter for CapturingAdapter {
        fn kind(&self) -> ExecutorKind {
            ExecutorKind::Codex
        }

        fn check_availability(&self) -> AvailabilityInfo {
            AvailabilityInfo {
                status: AvailabilityStatus::Authenticated,
                authenticated_at: None,
                config_path: None,
            }
        }

        async fn discover_options(
            &self,
            _ctx: DiscoverContext,
        ) -> Result<DiscoveredOptions, ExecutorError> {
            Ok(DiscoveredOptions::default())
        }

        async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
            assert_eq!(ctx.agent_config["model"], "gpt-5-codex");
            assert_eq!(ctx.agent_config["model_reasoning_effort"], "high");
            assert_eq!(ctx.agent_config["permission_policy"], "auto");
            assert_eq!(ctx.agent_config["sandbox"], "danger-full-access");
            assert!(ctx.agent_config.get("effort").is_none());
            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                after_sha: None,
                agent_session_id: None,
                summary: None,
                error: None,
                usage: None,
                ..Default::default()
            })
        }

        async fn cancel(&self, _execution_id: &str) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    struct CancelTrackingAdapter {
        cancel_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl HarnessAdapter for CancelTrackingAdapter {
        fn kind(&self) -> ExecutorKind {
            ExecutorKind::Shell
        }

        fn check_availability(&self) -> AvailabilityInfo {
            AvailabilityInfo {
                status: AvailabilityStatus::Authenticated,
                authenticated_at: None,
                config_path: None,
            }
        }

        async fn discover_options(
            &self,
            _ctx: DiscoverContext,
        ) -> Result<DiscoveredOptions, ExecutorError> {
            Ok(DiscoveredOptions::default())
        }

        async fn execute(&self, _ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                after_sha: None,
                agent_session_id: None,
                summary: None,
                error: None,
                usage: None,
                ..Default::default()
            })
        }

        async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
            assert_eq!(execution_id, "execution-to-cancel");
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn adapter_executor_dispatches_using_snapshot_config() {
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(CapturingAdapter));
        let executor = AdapterExecutor::new(Arc::new(registry));

        let result = executor
            .execute(ExecutionContext {
                    invocation: crate::HarnessInvocation::Start,
                task_id: "task".to_owned(),
                execution_id: "execution".to_owned(),
                role: "coder".to_owned(),
                worktree_path: ".".to_owned(),
                description: "do it".to_owned(),
                agent_config: serde_json::json!({
                    "executor_type": "codex",
                    "config": {
                        "model": "gpt-5-codex",
                        "model_reasoning_effort": "high",
                        "effort": "high",
                        "permission_policy": "auto",
                        "sandbox": "danger-full-access"
                    }
                }),
                logs_path: "logs.jsonl".to_owned(),
                heartbeat_interval_seconds: 1,
                max_turns: None,
                log_sender: None,
            })
            .await
            .expect("dispatch succeeds");

        assert_eq!(result.status, ExecutionOutcome::Completed);
    }

    #[test]
    fn resolve_context_config_builds_shell_command_plan() {
        let snapshot = serde_json::json!({
            "executor_type": "shell",
            "config": {
                "command": "bash",
                "args": ["-lc", "make test"],
                "permission_policy": "supervised",
                "additional_params": ["--verbose"],
                "env": { "CI": "1" }
            }
        });

        let (kind, config) = resolve_context_config(&snapshot).expect("snapshot resolves");
        assert_eq!(kind, ExecutorKind::Shell);

        let shell_config: ShellConfig = serde_json::from_value(config).expect("shell config");
        let plan =
            build_shell_command_plan("ignored", "/tmp/worktree", Some(2), Some(&shell_config));

        assert_eq!(plan.program, "bash");
        assert_eq!(plan.args, vec!["-lc", "make test", "--verbose"]);
        assert_eq!(plan.cwd.to_string_lossy(), "/tmp/worktree");
        assert_eq!(plan.env_set.get("CI").map(String::as_str), Some("1"));
        assert_eq!(
            plan.env_set.get("FORGE_MAX_TURNS").map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn resolve_context_config_resolves_codex_snapshot() {
        let snapshot = serde_json::json!({
            "executor_type": "codex",
            "config": {
                "model": "gpt-5-codex",
                "sandbox": "danger-full-access",
                "model_reasoning_effort": "high",
                "permission_policy": "auto",
                "additional_params": ["--verbose"],
                "env": { "CUSTOM": "1" }
            }
        });

        let (kind, config) = resolve_context_config(&snapshot).expect("snapshot resolves");
        assert_eq!(kind, ExecutorKind::Codex);

        let codex_config: crate::CodexConfig =
            serde_json::from_value(config).expect("codex config");
        assert_eq!(codex_config.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(
            codex_config.command_overrides.additional_params.as_deref(),
            Some(["--verbose".to_owned()].as_slice())
        );
    }

    #[derive(Clone)]
    enum ScriptedBehavior {
        Complete {
            usage_output_tokens: i64,
        },
        FailTask,
        Exhausted {
            retry_after_ms: u64,
            usage_output_tokens: i64,
        },
        AwaitCancel,
        UnavailableAtDetect,
    }

    struct ScriptedAdapter {
        kind: ExecutorKind,
        behaviors: std::collections::HashMap<String, ScriptedBehavior>,
        calls: Arc<std::sync::Mutex<Vec<String>>>,
        observed_configs: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        invocations: Arc<std::sync::Mutex<Vec<api_types::HarnessInvocation>>>,
        started: Arc<tokio::sync::Notify>,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    }

    impl ScriptedAdapter {
        fn new(kind: ExecutorKind, behaviors: &[(&str, ScriptedBehavior)]) -> Self {
            Self {
                kind,
                behaviors: behaviors
                    .iter()
                    .map(|(profile, behavior)| ((*profile).to_owned(), behavior.clone()))
                    .collect(),
                calls: Arc::default(),
                observed_configs: Arc::default(),
                invocations: Arc::default(),
                started: Arc::new(tokio::sync::Notify::new()),
                cancelled: Arc::default(),
            }
        }
    }

    #[async_trait]
    impl HarnessAdapter for ScriptedAdapter {
        fn kind(&self) -> ExecutorKind {
            self.kind.clone()
        }

        fn check_availability(&self) -> AvailabilityInfo {
            AvailabilityInfo {
                status: AvailabilityStatus::Authenticated,
                authenticated_at: None,
                config_path: None,
            }
        }

        fn detect(&self, config: &serde_json::Value) -> AvailabilityInfo {
            let key = config
                .get("model")
                .and_then(serde_json::Value::as_str)
                .or_else(|| config.get("profile").and_then(serde_json::Value::as_str));
            let unavailable = key.is_some_and(|key| {
                matches!(
                    self.behaviors.get(key),
                    Some(ScriptedBehavior::UnavailableAtDetect)
                )
            });
            AvailabilityInfo {
                status: if unavailable {
                    AvailabilityStatus::NotFound
                } else {
                    AvailabilityStatus::Authenticated
                },
                authenticated_at: None,
                config_path: None,
            }
        }

        fn capabilities(&self, config: &serde_json::Value) -> api_types::HarnessCapabilities {
            let model = config.get("model").and_then(serde_json::Value::as_str);
            api_types::HarnessCapabilities {
                resume: api_types::CapabilitySupport::Native,
                account_usage_observation: api_types::CapabilitySupport::Native,
                structured_events: if model == Some("model-a") {
                    api_types::CapabilitySupport::Native
                } else {
                    api_types::CapabilitySupport::Emulated
                },
                ..api_types::HarnessCapabilities::unknown()
            }
        }

        async fn discover_options(
            &self,
            _ctx: DiscoverContext,
        ) -> Result<DiscoveredOptions, ExecutorError> {
            Ok(DiscoveredOptions::default())
        }

        async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
            self.invocations
                .lock()
                .unwrap()
                .push(ctx.invocation.clone());
            self.observed_configs
                .lock()
                .unwrap()
                .push(ctx.agent_config.clone());
            let key = ctx.agent_config["model"]
                .as_str()
                .or_else(|| ctx.agent_config["profile"].as_str())
                .expect("scripted config has model or profile")
                .to_owned();
            self.calls.lock().unwrap().push(key.clone());
            let usage = |output_tokens: i64| crate::TokenUsage {
                output_tokens,
                ..Default::default()
            };
            match self.behaviors.get(&key).expect("scripted behavior") {
                ScriptedBehavior::Complete {
                    usage_output_tokens,
                } => Ok(ExecutionResult {
                    status: ExecutionOutcome::Completed,
                    usage: Some(usage(*usage_output_tokens)),
                    ..Default::default()
                }),
                ScriptedBehavior::FailTask => Ok(ExecutionResult {
                    status: ExecutionOutcome::Failed,
                    error: Some("tests failed".to_owned()),
                    ..Default::default()
                }),
                ScriptedBehavior::Exhausted {
                    retry_after_ms,
                    usage_output_tokens,
                } => Err(ExecutorError::UsageExhausted {
                    retry_after: Some(std::time::Duration::from_millis(*retry_after_ms)),
                    usage: Some(usage(*usage_output_tokens)),
                }),
                ScriptedBehavior::AwaitCancel => {
                    self.started.notify_one();
                    while !self.cancelled.load(Ordering::SeqCst) {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                    Ok(ExecutionResult {
                        status: ExecutionOutcome::Cancelled,
                        ..Default::default()
                    })
                }
                ScriptedBehavior::UnavailableAtDetect => Err(ExecutorError::Unavailable(
                    "scripted candidate should have been filtered by detect".to_owned(),
                )),
            }
        }

        async fn cancel(&self, _execution_id: &str) -> Result<(), ExecutorError> {
            self.cancelled.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn observe_usage(
            &self,
            config: &serde_json::Value,
            _cancel: CancellationToken,
        ) -> Result<Option<UsageObservation>, ExecutorError> {
            Ok(Some(UsageObservation {
                value: serde_json::json!({"profile": config["profile"]}),
                source: Some("scripted_adapter_usage".to_owned()),
            }))
        }
    }

    fn routed_ctx(execution_id: &str, logs_dir: &std::path::Path) -> ExecutionContext {
        ExecutionContext {
            invocation: crate::HarnessInvocation::Start,
            task_id: "task".to_owned(),
            execution_id: execution_id.to_owned(),
            role: "coder".to_owned(),
            worktree_path: ".".to_owned(),
            description: "do it".to_owned(),
            agent_config: serde_json::json!({
                "executor_type": "smith",
                "config": {"profile": "acct-1", "model": "model-a"},
                "routing": {
                    "policy": "ordered_fallback_v1",
                    "candidates": [
                        {"executor_type": "smith", "config": {"profile": "acct-1", "model": "model-a"}},
                        {"executor_type": "smith", "config": {"profile": "acct-1", "model": "model-b"}}
                    ]
                }
            }),
            logs_path: logs_dir
                .join(format!("{execution_id}.jsonl"))
                .to_string_lossy()
                .into_owned(),
            heartbeat_interval_seconds: 1,
            max_turns: None,
            log_sender: None,
        }
    }

    fn fallback_executor(
        behaviors: &[(&str, ScriptedBehavior)],
    ) -> (FallbackExecutor, Arc<std::sync::Mutex<Vec<String>>>) {
        let adapter = ScriptedAdapter::new(ExecutorKind::Smith, behaviors);
        let calls = Arc::clone(&adapter.calls);
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(adapter));
        (FallbackExecutor::new(Arc::new(registry)), calls)
    }

    #[tokio::test]
    async fn usage_exhaustion_does_not_fall_back_across_agent_identity() {
        let dir = tempfile::tempdir().unwrap();
        let (executor, calls) = fallback_executor(&[
            (
                "model-a",
                ScriptedBehavior::Exhausted {
                    retry_after_ms: 60_000,
                    usage_output_tokens: 10,
                },
            ),
            (
                "model-b",
                ScriptedBehavior::Complete {
                    usage_output_tokens: 5,
                },
            ),
        ]);

        let result = executor
            .execute(routed_ctx("exec-1", dir.path()))
            .await
            .expect("exhaustion is returned as an execution result");

        assert_eq!(result.status, ExecutionOutcome::Failed);
        assert_eq!(*calls.lock().unwrap(), vec!["model-a"]);
        assert_eq!(result.usage.expect("usage is preserved").output_tokens, 10);
        assert!(result.resolved_candidate.is_none());
        let outcomes: Vec<_> = result.route_attempts.iter().map(|a| a.outcome).collect();
        assert_eq!(
            outcomes,
            vec![
                crate::config::RouteAttemptOutcome::UsageExhausted,
                crate::config::RouteAttemptOutcome::SkippedCooldown
            ]
        );
    }

    #[tokio::test]
    async fn task_failure_stops_chain_without_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let (executor, calls) = fallback_executor(&[
            ("model-a", ScriptedBehavior::FailTask),
            (
                "model-b",
                ScriptedBehavior::Complete {
                    usage_output_tokens: 5,
                },
            ),
        ]);

        let result = executor
            .execute(routed_ctx("exec-1", dir.path()))
            .await
            .expect("terminal result");

        assert_eq!(result.status, ExecutionOutcome::Failed);
        assert_eq!(
            result.failure_class,
            Some(crate::ExecutionFailureClass::TaskFailed)
        );
        assert_eq!(*calls.lock().unwrap(), vec!["model-a"]);
    }

    #[tokio::test]
    async fn same_agent_candidates_share_account_cooldown() {
        let dir = tempfile::tempdir().unwrap();
        let (executor, calls) = fallback_executor(&[
            (
                "model-a",
                ScriptedBehavior::Exhausted {
                    retry_after_ms: 60_000,
                    usage_output_tokens: 0,
                },
            ),
            (
                "model-b",
                ScriptedBehavior::Exhausted {
                    retry_after_ms: 30_000,
                    usage_output_tokens: 0,
                },
            ),
        ]);

        let first = executor
            .execute(routed_ctx("exec-1", dir.path()))
            .await
            .expect("terminal result");
        assert_eq!(
            first.failure_class,
            Some(crate::ExecutionFailureClass::ExecutorUnavailable)
        );
        let retry = first.retry_after.expect("earliest retry propagated");
        assert!(retry <= std::time::Duration::from_millis(30_000));
        assert_eq!(calls.lock().unwrap().len(), 1);

        // A second execution sees both accounts cooling: fail fast, no spawns.
        let second = executor
            .execute(routed_ctx("exec-2", dir.path()))
            .await
            .expect("terminal result");
        assert_eq!(
            second.failure_class,
            Some(crate::ExecutionFailureClass::ExecutorUnavailable)
        );
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "no CLI spawned while cooling"
        );
        let outcomes: Vec<_> = second.route_attempts.iter().map(|a| a.outcome).collect();
        assert_eq!(
            outcomes,
            vec![
                crate::config::RouteAttemptOutcome::SkippedCooldown,
                crate::config::RouteAttemptOutcome::SkippedCooldown
            ]
        );
    }

    #[tokio::test]
    async fn cancel_between_hops_spawns_no_further_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = ScriptedAdapter::new(ExecutorKind::Smith, &[
            ("model-a", ScriptedBehavior::AwaitCancel),
            (
                "model-b",
                ScriptedBehavior::Complete {
                    usage_output_tokens: 5,
                },
            ),
        ]);
        let calls = Arc::clone(&adapter.calls);
        let started = Arc::clone(&adapter.started);
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(adapter));
        let executor = Arc::new(FallbackExecutor::new(Arc::new(registry)));

        let run = tokio::spawn({
            let executor = Arc::clone(&executor);
            let ctx = routed_ctx("exec-cancel", dir.path());
            async move { executor.execute(ctx).await }
        });

        started.notified().await;
        executor
            .cancel("exec-cancel")
            .await
            .expect("cancel succeeds");

        let result = run.await.expect("join").expect("terminal result");
        assert_eq!(result.status, ExecutionOutcome::Cancelled);
        assert_eq!(*calls.lock().unwrap(), vec!["model-a"]);
    }

    #[tokio::test]
    async fn snapshot_without_routing_dispatches_single_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let (executor, calls) = fallback_executor(&[(
                "acct-1",
            ScriptedBehavior::Complete {
                usage_output_tokens: 1,
            },
        )]);

        let mut ctx = routed_ctx("exec-1", dir.path());
        ctx.agent_config = serde_json::json!({
            "executor_type": "smith",
            "config": {"profile": "acct-1"}
        });

        let result = executor.execute(ctx).await.expect("dispatch succeeds");
        assert_eq!(result.status, ExecutionOutcome::Completed);
        assert_eq!(*calls.lock().unwrap(), vec!["acct-1"]);
        assert!(result
            .resolved_candidate
            .expect("winner recorded")
            .candidate_key
            .contains("profile=acct-1"));
    }

    #[tokio::test]
    async fn resume_intent_reaches_adapter_and_does_not_advance_to_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = ScriptedAdapter::new(
            ExecutorKind::Smith,
            &[
                ("model-a", ScriptedBehavior::Complete { usage_output_tokens: 1 }),
                ("model-b", ScriptedBehavior::Complete { usage_output_tokens: 2 }),
            ],
        );
        let calls = Arc::clone(&adapter.calls);
        let invocations = Arc::clone(&adapter.invocations);
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(adapter));
        let executor = FallbackExecutor::new(Arc::new(registry));

        let mut ctx = routed_ctx("resume-exec", dir.path());
        ctx.invocation = api_types::HarnessInvocation::Resume {
            external_session_id: "external-session-abc".to_owned(),
        };
        let result = executor.execute(ctx).await.expect("resume completes");

        assert_eq!(result.status, ExecutionOutcome::Completed);
        assert_eq!(*calls.lock().unwrap(), vec!["model-a"]);
        assert_eq!(
            *invocations.lock().unwrap(),
            vec![api_types::HarnessInvocation::Resume {
                external_session_id: "external-session-abc".to_owned()
            }]
        );
    }

    #[tokio::test]
    async fn resume_does_not_use_an_available_alternate_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let (executor, calls) = fallback_executor(&[
            ("model-a", ScriptedBehavior::UnavailableAtDetect),
            (
                "model-b",
                ScriptedBehavior::Complete {
                    usage_output_tokens: 2,
                },
            ),
        ]);
        let mut ctx = routed_ctx("resume-exec", dir.path());
        ctx.invocation = api_types::HarnessInvocation::Resume {
            external_session_id: "session-from-acct-1".to_owned(),
        };

        let result = executor.execute(ctx).await.expect("unavailable is a result");

        assert_eq!(
            result.failure_class,
            Some(crate::ExecutionFailureClass::ExecutorUnavailable)
        );
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(result.route_attempts.len(), 1);
        assert!(result.resolved_candidate.is_none());
    }

    #[tokio::test]
    async fn fallback_cannot_switch_harness_identity() {
        let dir = tempfile::tempdir().unwrap();
        let codex = ScriptedAdapter::new(
            ExecutorKind::Codex,
            &[("acct-a", ScriptedBehavior::UnavailableAtDetect)],
        );
        let cursor = ScriptedAdapter::new(
            ExecutorKind::Cursor,
            &[("acct-b", ScriptedBehavior::Complete { usage_output_tokens: 3 })],
        );
        let cursor_calls = Arc::clone(&cursor.calls);
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(codex));
        registry.register(Box::new(cursor));
        let executor = FallbackExecutor::new(Arc::new(registry));

        let mut ctx = routed_ctx("cross-harness-exec", dir.path());
        ctx.agent_config = serde_json::json!({
            "executor_type": "codex",
            "config": {"profile": "acct-a"},
            "routing": {
                "policy": "ordered_fallback_v1",
                "candidates": [
                    {"executor_type": "cursor", "config": {"profile": "acct-b"}}
                ]
            }
        });

        let result = executor.execute(ctx).await;
        assert!(result.is_err(), "cross-harness fallback must be rejected");
        assert!(cursor_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn same_agent_start_fallback_records_winning_model_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = ScriptedAdapter::new(
            ExecutorKind::Smith,
            &[
                ("model-a", ScriptedBehavior::UnavailableAtDetect),
                ("model-b", ScriptedBehavior::Complete { usage_output_tokens: 3 }),
            ],
        );
        let calls = Arc::clone(&adapter.calls);
        let observed_configs = Arc::clone(&adapter.observed_configs);
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(adapter));
        let executor = FallbackExecutor::new(Arc::new(registry));

        let mut ctx = routed_ctx("same-harness-fallback", dir.path());
        ctx.agent_config = serde_json::json!({
            "executor_type": "smith",
            "config": {"profile":"acct-1", "model":"model-a"},
            "runtime_env": {"SMITH_API_KEY":"runtime-secret"},
            "routing": {
                "policy": "ordered_fallback_v1",
                "candidates": [
                    {"executor_type":"smith","config":{"profile":"acct-1", "model":"model-b"}}
                ]
            }
        });

        let result = executor.execute(ctx).await.expect("same-kind fallback completes");
        let winner = result.resolved_candidate.expect("winner recorded");

        assert_eq!(winner.executor_type, ExecutorKind::Smith);
        assert_eq!(winner.config["profile"], "acct-1");
        assert_eq!(winner.config["model"], "model-b");
        assert_eq!(
            winner.harness_capabilities.structured_events,
            api_types::CapabilitySupport::Emulated
        );
        assert_eq!(
            winner.candidate_key,
            crate::config::candidate_key(
                &ExecutorKind::Smith,
                &serde_json::json!({"profile":"acct-1", "model":"model-b"})
            )
        );
        assert!(winner.config.get("env").is_none());
        let invoked_config = observed_configs.lock().unwrap()[0].clone();
        assert_eq!(invoked_config["model"], "model-b");
        assert_eq!(invoked_config["env"]["SMITH_API_KEY"], "runtime-secret");
        assert_eq!(*calls.lock().unwrap(), vec!["model-b"]);
    }

    #[tokio::test]
    async fn unknown_resume_is_an_explicit_unsupported_capability() {
        let adapter = CapturingAdapter;
        let ctx = routed_ctx("unknown-resume", std::path::Path::new("/tmp"));

        let error = adapter
            .resume(ctx, "session-unknown")
            .await
            .expect_err("unknown resume must fail closed");

        assert!(matches!(
            error,
            ExecutorError::UnsupportedCapability {
                capability,
                support: api_types::CapabilitySupport::Unknown,
            } if capability == "resume"
        ));
    }

    #[tokio::test]
    async fn generic_usage_observation_routes_through_registered_adapter() {
        let adapter = ScriptedAdapter::new(
            ExecutorKind::Smith,
            &[("acct-usage", ScriptedBehavior::Complete { usage_output_tokens: 0 })],
        );
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(adapter));
        let executor = AdapterExecutor::new(Arc::new(registry));

        let observation = executor
            .observe_usage(
                ExecutorKind::Smith,
                &serde_json::json!({"profile":"acct-usage"}),
                CancellationToken::new(),
            )
            .await
            .expect("adapter observation succeeds")
            .expect("adapter returns an observation");

        assert_eq!(observation.value, serde_json::json!({"profile":"acct-usage"}));
        assert_eq!(observation.source.as_deref(), Some("scripted_adapter_usage"));
    }

    #[tokio::test]
    async fn adapter_executor_cancel_passthrough_reaches_registered_adapter() {
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let mut registry = HarnessAdapterRegistry::new();
        registry.register(Box::new(CancelTrackingAdapter {
            cancel_calls: Arc::clone(&cancel_calls),
        }));
        let executor = AdapterExecutor::new(Arc::new(registry));

        executor
            .cancel("execution-to-cancel")
            .await
            .expect("cancel succeeds");

        assert_eq!(cancel_calls.load(Ordering::SeqCst), 1);
    }
}
