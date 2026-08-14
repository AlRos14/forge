use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemorySearchQuery {
    pub query: String,
    pub layer: Option<u8>,
    pub token_budget: Option<u32>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemoryGetQuery {
    pub layer: Option<u8>,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemorySearchResultDto {
    pub id: String,
    pub layer: u8,
    pub content: String,
    pub score: f32,
    pub source_type: String,
    pub source_id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub created_at: String,
    pub creator: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemorySearchResponse {
    pub items: Vec<MemorySearchResultDto>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemoryBackfillTypeResponse {
    pub source_type: String,
    pub indexed: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemoryBackfillResponse {
    pub items: Vec<MemoryBackfillTypeResponse>,
    pub indexed: u64,
    pub skipped: u64,
}

/// Explicitly widens one immutable memory assertion into another canonical
/// scope.  The API never returns the submitted evidence; it is retained only
/// for audit and is represented by lifecycle metadata in responses.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemoryPublicationRequest {
    pub source_scope_type: String,
    pub source_scope_id: String,
    pub target_scope_type: String,
    pub target_scope_id: String,
    pub target_project_id: Option<String>,
    pub target_task_id: Option<String>,
    pub target_chat_id: Option<String>,
    pub target_visibility: String,
    pub target_authority: String,
    pub actor_identity_id: String,
    pub reason: String,
    pub evidence_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemoryLifecycleRequest {
    pub scope_type: String,
    pub scope_id: String,
    pub assertion_type: String,
    pub related_memory_id: Option<String>,
    pub reason: Option<String>,
    pub evidence_json: String,
    pub actor_identity_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemoryLifecycleResponse {
    pub id: String,
    pub memory_item_id: String,
    pub assertion_type: String,
    pub related_memory_id: Option<String>,
    pub reason: Option<String>,
    pub evidence_present: bool,
    pub asserted_by_type: String,
    pub asserted_by_id: Option<String>,
    pub source_event_id: Option<String>,
    pub created_at: String,
}

/// Metadata-only memory provenance.  Deliberately omits title, summary, body,
/// evidence and any other content-bearing field.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemoryProvenanceResponse {
    pub id: String,
    pub scope_type: String,
    pub scope_id: String,
    pub visibility: String,
    pub owner_identity_id: Option<String>,
    pub authority: String,
    pub sensitivity: String,
    pub retention_priority: i64,
    pub source_type: String,
    pub source_ref: Option<String>,
    pub source_event_id: Option<String>,
    pub source_scope_type: Option<String>,
    pub source_scope_id: Option<String>,
    pub source_revision: Option<String>,
    pub source_chat_sequence: Option<i64>,
    pub publication_source_id: Option<String>,
    pub supersedes_id: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub created_by_type: Option<String>,
    pub created_by_id: Option<String>,
    pub created_at: String,
    pub lifecycle: Vec<MemoryLifecycleResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ContextManifestQuery {
    pub identity_id: String,
    pub context_scope_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ContextManifestListQuery {
    pub context_scope_id: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct MemoryProvenanceQuery {
    pub scope_type: String,
    pub scope_id: String,
    pub identity_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ContextManifestSourceResponse {
    pub ordinal: i64,
    pub source_id: String,
    pub source_type: String,
    pub source_revision: String,
    pub selection_reason: String,
    pub disposition: String,
    /// True when this immutable source revision no longer matches the
    /// Project's current canonical pointer. The historical selection decision
    /// and manifest fingerprint remain unchanged.
    pub is_stale: bool,
    /// The current canonical revision when this source type is pointer-backed.
    /// `None` means either the source is not pointer-backed or its canonical
    /// artifact is no longer present in the current Project projection.
    pub current_revision: Option<String>,
    pub retention_priority: i64,
    pub fragment_fingerprint: String,
}

/// Immutable context-manifest metadata and source decisions.  Source bodies
/// are never exposed by this inspector.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ContextManifestResponse {
    pub id: String,
    pub identity_id: String,
    pub agent_session_id: Option<String>,
    pub context_scope_id: String,
    pub scope_type: String,
    pub scope_id: String,
    pub policy_revision: String,
    pub domain_revision: String,
    pub lcm_binding_revision: Option<String>,
    pub runtime_manifest_id: Option<String>,
    pub runtime_manifest_fingerprint: Option<String>,
    pub combined_fingerprint: String,
    pub request_fingerprint: String,
    pub created_at: String,
    pub sources: Vec<ContextManifestSourceResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ContextManifestListResponse {
    pub items: Vec<ContextManifestResponse>,
    pub has_more: bool,
}
