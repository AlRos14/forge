use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// How the current Forge integration implements one harness operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CapabilitySupport {
    Native,
    Emulated,
    Unsupported,
    Unknown,
}

impl CapabilitySupport {
    pub const fn is_available(self) -> bool {
        matches!(self, Self::Native | Self::Emulated)
    }

    pub const fn is_native(self) -> bool {
        matches!(self, Self::Native)
    }
}

/// Effective, non-secret capability evidence for one normalized harness
/// configuration. Unknown is always fail-closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct HarnessCapabilities {
    pub resume: CapabilitySupport,
    pub cancel: CapabilitySupport,
    pub structured_events: CapabilitySupport,
    pub usage_reporting: CapabilitySupport,
    pub account_usage_observation: CapabilitySupport,
    pub model_selection: CapabilitySupport,
    pub reasoning_controls: CapabilitySupport,
    pub approval_policy: CapabilitySupport,
    pub sandbox_controls: CapabilitySupport,
    pub planning: CapabilitySupport,
    pub review_mode: CapabilitySupport,
    pub fork: CapabilitySupport,
    pub steer: CapabilitySupport,
    pub pause_resume: CapabilitySupport,
    pub compaction: CapabilitySupport,
    pub subagents: CapabilitySupport,
}

impl Default for HarnessCapabilities {
    fn default() -> Self {
        Self::unknown()
    }
}

impl HarnessCapabilities {
    pub const fn unknown() -> Self {
        Self {
            resume: CapabilitySupport::Unknown,
            cancel: CapabilitySupport::Unknown,
            structured_events: CapabilitySupport::Unknown,
            usage_reporting: CapabilitySupport::Unknown,
            account_usage_observation: CapabilitySupport::Unknown,
            model_selection: CapabilitySupport::Unknown,
            reasoning_controls: CapabilitySupport::Unknown,
            approval_policy: CapabilitySupport::Unknown,
            sandbox_controls: CapabilitySupport::Unknown,
            planning: CapabilitySupport::Unknown,
            review_mode: CapabilitySupport::Unknown,
            fork: CapabilitySupport::Unknown,
            steer: CapabilitySupport::Unknown,
            pause_resume: CapabilitySupport::Unknown,
            compaction: CapabilitySupport::Unknown,
            subagents: CapabilitySupport::Unknown,
        }
    }

    pub const fn unsupported() -> Self {
        Self {
            resume: CapabilitySupport::Unsupported,
            cancel: CapabilitySupport::Unsupported,
            structured_events: CapabilitySupport::Unsupported,
            usage_reporting: CapabilitySupport::Unsupported,
            account_usage_observation: CapabilitySupport::Unsupported,
            model_selection: CapabilitySupport::Unsupported,
            reasoning_controls: CapabilitySupport::Unsupported,
            approval_policy: CapabilitySupport::Unsupported,
            sandbox_controls: CapabilitySupport::Unsupported,
            planning: CapabilitySupport::Unsupported,
            review_mode: CapabilitySupport::Unsupported,
            fork: CapabilitySupport::Unsupported,
            steer: CapabilitySupport::Unsupported,
            pause_resume: CapabilitySupport::Unsupported,
            compaction: CapabilitySupport::Unsupported,
            subagents: CapabilitySupport::Unsupported,
        }
    }
}

/// Runtime-only request to begin a harness run or continue one exact session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum HarnessInvocation {
    #[default]
    Start,
    Resume { external_session_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorTypeDescriptor {
    #[serde(rename = "type")]
    pub type_name: String,
    pub display_name: String,
    pub config_schema: Value,
    pub default_config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailabilityResponse {
    pub status: String,
    pub authenticated_at: Option<String>,
    pub config_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredOptionsResponse {
    pub models: Vec<String>,
    pub permission_policies: Vec<String>,
    pub cli_specific: Value,
    pub harness_capabilities: HarnessCapabilities,
    #[serde(default)]
    pub available_daemons: Vec<DiscoveredDaemonResponse>,
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredDaemonResponse {
    pub id: String,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAvailabilityResponse {
    pub available: bool,
    pub effective_status: String,
    pub resolved_daemon_id: Option<String>,
    pub active_task_count: i64,
    pub max_concurrent_tasks: i64,
    pub reason: Option<String>,
}

#[cfg(test)]
mod harness_capability_tests {
    use super::{CapabilitySupport as S, HarnessCapabilities};

    #[test]
    fn support_levels_remain_distinct_and_unknown_fails_closed() {
        assert!(S::Native.is_available());
        assert!(S::Native.is_native());
        assert!(S::Emulated.is_available());
        assert!(!S::Emulated.is_native());
        assert!(!S::Unsupported.is_available());
        assert!(!S::Unknown.is_available());

        assert_eq!(
            serde_json::to_value(S::Emulated).expect("support serializes"),
            serde_json::json!("emulated")
        );
        assert_eq!(HarnessCapabilities::default().resume, S::Unknown);
    }

    #[test]
    fn typed_snapshot_requires_all_dimensions_and_rejects_legacy_extra_fields() {
        let incomplete = serde_json::json!({"resume":"native"});
        assert!(serde_json::from_value::<HarnessCapabilities>(incomplete).is_err());

        let mut complete = serde_json::to_value(HarnessCapabilities::unknown()).unwrap();
        complete["legacy_tag"] = serde_json::json!("planning");
        assert!(serde_json::from_value::<HarnessCapabilities>(complete).is_err());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceResponse {
    pub id: String,
    pub task_id: String,
    pub repo_id: String,
    pub worktree_path: String,
    pub branch: String,
    pub status: String,
    pub before_sha: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub description: Option<String>,
    pub executor_type: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
    pub prompt_template: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub config_json: Option<Value>,
    pub daemon_id: Option<String>,
    pub max_concurrent_tasks: Option<i64>,
    pub heartbeat_interval_seconds: Option<i64>,
    pub max_missed_heartbeats: Option<i64>,
    pub is_default: Option<bool>,
    /// Optional provider entry powering this harness agent. When set, Forge
    /// injects the credential at dispatch (`auth_source: forge_provider`);
    /// when absent the harness uses its own CLI-managed login.
    #[serde(default)]
    pub credential_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAgentRequest {
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub model: Option<Option<String>>,
    #[serde(default)]
    pub reasoning_effort: Option<Option<String>>,
    #[serde(default)]
    pub permission_policy: Option<Option<String>>,
    #[serde(default)]
    pub prompt_template: Option<Option<String>>,
    pub capabilities: Option<Vec<String>>,
    pub config_json: Option<Value>,
    #[serde(default)]
    pub daemon_id: Option<Option<String>>,
    pub max_concurrent_tasks: Option<i64>,
    pub is_default: Option<bool>,
    pub paused: Option<bool>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateAgentRequest {
    pub name: String,
}

/// Create a direct (embedded-runtime) agent referencing an existing provider
/// entry. Credentials are never part of this request.
#[derive(Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateEmbeddedAgentRequest {
    pub name: String,
    pub description: Option<String>,
    pub credential_id: String,
    pub model: String,
    pub system_prompt: Option<String>,
    #[ts(type = "Record<string, unknown> | null")]
    pub account_permission_ceiling: Option<Value>,
    #[ts(type = "Record<string, unknown> | null")]
    pub tool_policy: Option<Value>,
    pub context_tokens: Option<u32>,
    pub max_input_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

/// Publish a replacement profile for an existing embedded identity,
/// referencing an existing provider entry.
#[derive(Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConnectEmbeddedProfileRequest {
    #[ts(type = "number")]
    pub version: i64,
    pub credential_id: String,
    pub model: String,
    pub system_prompt: Option<String>,
    pub permission_policy: Option<String>,
    #[ts(type = "Record<string, unknown> | null")]
    pub tool_policy: Option<Value>,
    pub context_tokens: Option<u32>,
    pub max_input_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum CanonicalScopeRequest {
    Account,
    Project { project_id: String },
    AgentChat { chat_id: String },
    Task { task_id: String, role: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateAgentSessionRequest {
    pub profile_id: Option<String>,
    pub scope: CanonicalScopeRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionVersionRequest {
    #[ts(type = "number")]
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SteerAgentSessionRequest {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CredentialHandleResponse {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub credential_method: String,
    pub status: String,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProviderRevocationStatus {
    NotSupported,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct DisconnectCredentialResponse {
    pub id: String,
    pub status: String,
    pub provider_revocation: ProviderRevocationStatus,
    /// Agents that referenced the removed entry and are now visibly
    /// unhealthy. They are never silently rebound or deleted.
    pub affected_agents: Vec<crate::ProviderEntryAgentRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentProfileResponse {
    pub id: String,
    pub identity_id: String,
    pub backend_kind: String,
    pub executor_type: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
    pub system_prompt: Option<String>,
    #[ts(type = "Record<string, unknown>")]
    pub capabilities: Value,
    #[ts(type = "Record<string, unknown>")]
    pub tool_policy: Value,
    #[ts(type = "Record<string, unknown>")]
    pub config: Value,
    pub credential_handle_id: Option<String>,
    pub version: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentConnectionHealthResponse {
    pub profile_id: String,
    pub status: String,
    #[ts(type = "Record<string, unknown>")]
    pub capabilities: Value,
    pub checked_at: Option<String>,
    pub error_code: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSessionResponse {
    pub id: String,
    pub identity_id: String,
    pub profile_id: String,
    pub context_scope_id: String,
    pub backend_kind: String,
    pub status: String,
    #[ts(type = "Record<string, unknown>")]
    pub capabilities: Value,
    pub connection_status: String,
    pub predecessor_session_id: Option<String>,
    pub replaced_by_session_id: Option<String>,
    pub last_activity_at: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Redaction-safe metadata for a pending native runtime interaction.  The
/// questionnaire and any answer remain encrypted in the protected runtime
/// store; this type intentionally contains no question/answer bodies.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProtectedInteractionSummaryResponse {
    pub id: String,
    pub session_id: String,
    pub interaction_kind: String,
    pub prompt_redacted: String,
    pub status: String,
    pub expires_at: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProtectedInteractionAnswerRequest {
    pub expected_version: i64,
    pub values: Vec<ProtectedInteractionAnswerValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum ProtectedInteractionAnswerValue {
    Choice {
        question_id: String,
        choice_id: String,
    },
    FreeForm {
        question_id: String,
        value: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProtectedInteractionCancelRequest {
    pub expected_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConnectedEmbeddedAgentResponse {
    pub agent: crate::AgentResponse,
    pub credential_handle: CredentialHandleResponse,
    pub profile: AgentProfileResponse,
    pub health: AgentConnectionHealthResponse,
    pub session: AgentSessionResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConnectedEmbeddedProfileResponse {
    pub agent: crate::AgentResponse,
    pub profile: AgentProfileResponse,
    pub credential_handle: CredentialHandleResponse,
    pub health: AgentConnectionHealthResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EffectivePermissionsResponse {
    pub allowed: Vec<String>,
    pub denied: Vec<String>,
    pub requires_approval: Vec<String>,
}
