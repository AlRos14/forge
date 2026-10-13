//! Public contracts for shared Project media assets and audited mutations.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::AuthorizationProvenance;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct MutationEnvelope {
    #[ts(type = "number")]
    pub expected_version: i64,
    #[serde(default)]
    pub expected_digest: Option<String>,
    pub idempotency_key: String,
    #[serde(default)]
    pub deduplication_key: Option<String>,
    pub authorization: AuthorizationProvenance,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EvidenceAvailability {
    Available,
    Quarantined,
    Redacted,
    Purged,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct MediaAsset {
    pub id: String,
    pub project_id: String,
    pub original_filename: String,
    pub content_type: String,
    #[ts(type = "number")]
    pub byte_size: u64,
    pub checksum: String,
    pub availability: EvidenceAvailability,
    #[serde(default)]
    pub task_media_ids: Vec<String>,
    #[serde(default)]
    pub stable_project_url: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct ProjectMediaListResponse {
    pub items: Vec<MediaAsset>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Multipart upload metadata. The binary part is named `file`; clients send
/// this value in the `mutation` part with the caller's exact authorization.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProjectMediaUploadRequest {
    pub mutation: MutationEnvelope,
}

/// User-authorized disposition of a Project media asset. Storage keys and
/// bytes remain internal; callers address the asset through its stable URL.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProjectMediaTombstoneRequest {
    pub mutation: MutationEnvelope,
    pub reason: String,
}
