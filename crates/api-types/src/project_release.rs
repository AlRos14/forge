//! Read-only wire contracts for immutable historical Project Releases.
//!
//! Project Charter, document, baseline, milestone, readiness, and decision
//! runtime DTOs were retired with the Project OS storage in PR13. The Release
//! snapshot remains a public historical contract and is reconstructed only
//! from its frozen records.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use ts_rs::TS;

use crate::EvidenceAvailability;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub artifact_id: String,
    pub revision_id: String,
    pub content_digest: String,
    #[serde(default)]
    pub render_version: Option<String>,
    #[serde(default)]
    pub render_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PrincipalKind {
    User,
    Agent,
    Worker,
    Reviewer,
    Service,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRef {
    pub kind: PrincipalKind,
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationProvenance {
    pub principal: PrincipalRef,
    pub authorization_basis: String,
    pub action: String,
    pub event_id: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReleaseEvidenceAvailability {
    Available,
    Quarantined,
    Redacted,
    Purged,
    EvidenceUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct EvidencePin {
    pub id: String,
    pub release_id: String,
    pub attachment_id: String,
    pub asset_id: String,
    pub attachment_digest: String,
    pub asset_checksum: String,
    pub availability: EvidenceAvailability,
    /// Read-time overlay derived from an immutable, audited media tombstone.
    /// The historical `availability` above is never mutated after pinning.
    pub availability_projection: ReleaseEvidenceAvailability,
    #[serde(default)]
    pub task_media_id: Option<String>,
    #[serde(default)]
    pub stable_project_url: Option<String>,
    pub pinned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTaskReference {
    pub task_id: String,
    #[ts(type = "number")]
    pub task_version: i64,
    pub task_type: String,
    pub task_state: String,
    #[serde(default)]
    pub acceptance_check_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DecisionRecordState {
    Active,
    Superseded,
    Invalidated,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct ReleaseDecisionReference {
    pub decision_id: String,
    pub state: DecisionRecordState,
    pub digest: String,
    pub rationale: String,
    pub authorization: AuthorizationProvenance,
    pub affected_milestone_id: Option<String>,
    pub affected_check_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AcceptanceCheckResultStatus {
    Pass,
    Fail,
    Pending,
    Blocked,
    Stale,
    Unavailable,
    Waived,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct ReleaseValidationReference {
    pub validation_id: String,
    pub result_digest: String,
    pub evaluated_at: String,
    pub principal: PrincipalRef,
    pub authorization: AuthorizationProvenance,
    pub status: AcceptanceCheckResultStatus,
    pub result: String,
    pub input_digest: String,
    pub governing_revision_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSnapshot {
    pub schema_version: String,
    pub project_id: String,
    pub milestone_id: String,
    pub milestone_canonical_id: String,
    #[ts(type = "number")]
    pub release_revision: i64,
    pub release_identity: String,
    pub milestone_definition_revision_id: String,
    pub milestone_definition_digest: String,
    #[ts(type = "number")]
    pub expected_milestone_version: i64,
    #[serde(default)]
    pub display_label: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub changelog: Vec<String>,
    #[serde(default)]
    pub known_issues: Vec<String>,
    pub readiness_snapshot_id: String,
    pub readiness_digest: String,
    pub source_event_watermark: String,
    pub baseline_id: String,
    pub baseline_revision_id: String,
    pub baseline_digest: String,
    pub charter_revision: ArtifactRef,
    #[serde(default)]
    pub document_revisions: Vec<ArtifactRef>,
    #[serde(default)]
    pub included_decisions: Vec<ReleaseDecisionReference>,
    #[serde(default)]
    pub included_tasks: Vec<ReleaseTaskReference>,
    #[serde(default)]
    pub validation_results: Vec<ReleaseValidationReference>,
    #[serde(default)]
    pub repository_references: Vec<String>,
    pub evidence_pins: Vec<EvidencePin>,
    pub waived_check_ids: Vec<String>,
    pub release_policy_revision: String,
    pub release_policy_digest: String,
    pub released_by: PrincipalRef,
    pub authorization: AuthorizationProvenance,
    pub released_at: String,
    pub idempotency_key: String,
    pub snapshot_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct ProjectRelease {
    pub id: String,
    pub project_id: String,
    pub milestone_id: String,
    #[ts(type = "number")]
    pub release_sequence: i64,
    pub release_identity: String,
    pub snapshot: ReleaseSnapshot,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct ProjectReleaseListResponse {
    pub items: Vec<ProjectRelease>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Digest a value using recursively sorted object keys and a schema envelope.
/// This remains stable so the historical Release snapshot digest is checked
/// against the exact PR12 representation.
pub fn canonical_digest_with_schema<T: Serialize>(
    schema_version: &str,
    value: &T,
) -> Result<String, serde_json::Error> {
    let value = serde_json::to_value(value)?;
    let mut envelope = Map::new();
    envelope.insert(
        "schema_version".to_owned(),
        Value::String(schema_version.to_owned()),
    );
    envelope.insert("value".to_owned(), canonicalize_json(&value));
    let canonical = serde_json::to_string(&canonicalize_json(&Value::Object(envelope)))?;
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(hex_lower(&digest))
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted: BTreeMap<&str, Value> = object
                .iter()
                .map(|(key, value)| (key.as_str(), canonicalize_json(value)))
                .collect();
            let mut canonical = Map::new();
            for (key, value) in sorted {
                canonical.insert(key.to_owned(), value);
            }
            Value::Object(canonical)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        scalar => scalar.clone(),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::canonical_digest_with_schema;
    use serde_json::json;

    #[test]
    fn release_digest_sorts_nested_objects_without_reordering_arrays() {
        let left = json!({"z": {"b": 2, "a": 1}, "items": ["first", "second"]});
        let right = json!({"items": ["first", "second"], "z": {"a": 1, "b": 2}});
        assert_eq!(
            canonical_digest_with_schema("schema/v1", &left).unwrap(),
            canonical_digest_with_schema("schema/v1", &right).unwrap()
        );
        let reordered = json!({"items": ["second", "first"], "z": {"a": 1, "b": 2}});
        assert_ne!(
            canonical_digest_with_schema("schema/v1", &left).unwrap(),
            canonical_digest_with_schema("schema/v1", &reordered).unwrap()
        );
    }

    #[test]
    fn release_digest_includes_schema_version() {
        let value = json!({"release": "immutable"});
        assert_ne!(
            canonical_digest_with_schema("schema/a", &value).unwrap(),
            canonical_digest_with_schema("schema/b", &value).unwrap()
        );
    }
}
