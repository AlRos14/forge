//! Redaction-safe linkage from an Agent Runtime turn to Forge provenance.
//!
//! Agent Runtime owns context ordering, sizing, serialization, and LCM
//! compaction.  Forge only persists the immutable metadata needed to link a
//! domain context manifest to that final decision.  These types intentionally
//! contain identifiers, hashes, counts, and revisions only; no context or
//! summary body is representable here.

use agent_runtime::core::store::{SessionSnapshot, TurnManifest};
use serde::{Deserialize, Serialize};

/// The final context decision recorded by Agent Runtime for one turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeContextManifestLink {
    /// The runtime turn this manifest describes.
    pub turn_id: String,
    /// Runtime manifest vocabulary revision.
    pub schema_version: u32,
    /// Fingerprint of the assembled context plan.
    pub context_fingerprint: String,
    /// Fingerprint of the provider cache plan.
    pub cache_plan_fingerprint: String,
    /// Full redaction-safe RunManifest fingerprint, distinct from the
    /// assembled-context fingerprint above.
    pub runtime_manifest_fingerprint: String,
    /// Final ordered segments.  Segment bodies are never copied.
    pub segments: Vec<RuntimeContextSegmentLink>,
    /// Runtime compaction coverage, preserving source-to-summary linkage.
    pub summaries: Vec<RuntimeSummaryCoverageLink>,
    /// Redaction-safe LCM node provenance observed by this turn.
    pub lossless_summaries: Vec<RuntimeLosslessSummaryLink>,
    /// Opaque Forge timeline identity, when the host supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lcm_timeline_id: Option<String>,
    /// Authorization/configuration revision for the timeline binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lcm_binding_revision: Option<String>,
    /// Backing Forge LCM adapter revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lcm_store_revision: Option<String>,
}

impl RuntimeContextManifestLink {
    /// Projects the latest runtime manifest from a session snapshot without
    /// copying context bodies or protected runtime state.
    pub fn from_snapshot(snapshot: &SessionSnapshot) -> Option<Self> {
        snapshot.manifests.last().map(Self::from_turn_manifest)
    }

    /// Projects one runtime turn manifest into Forge's redaction-safe link.
    pub fn from_turn_manifest(turn: &TurnManifest) -> Self {
        let manifest = &turn.manifest;
        Self {
            turn_id: turn.turn.as_str().to_owned(),
            schema_version: manifest.schema_version,
            context_fingerprint: manifest.context_fingerprint.as_str().to_owned(),
            cache_plan_fingerprint: manifest.cache_plan_fingerprint.as_str().to_owned(),
            runtime_manifest_fingerprint: manifest.fingerprint().as_str().to_owned(),
            segments: manifest
                .segments
                .iter()
                .map(|segment| RuntimeContextSegmentLink {
                    id: segment.id.as_str().to_owned(),
                    kind: segment.kind.as_str().to_owned(),
                    sensitivity: segment.sensitivity.as_str().to_owned(),
                    content_hash: segment.content_hash.as_str().to_owned(),
                    tokens: segment.tokens,
                })
                .collect(),
            summaries: manifest
                .summaries
                .iter()
                .map(|summary| RuntimeSummaryCoverageLink {
                    summary: summary.summary.as_str().to_owned(),
                    covered: summary
                        .covered
                        .iter()
                        .map(|segment| segment.as_str().to_owned())
                        .collect(),
                })
                .collect(),
            lossless_summaries: manifest
                .lossless_summaries
                .iter()
                .map(RuntimeLosslessSummaryLink::from_runtime)
                .collect(),
            lcm_timeline_id: None,
            lcm_binding_revision: None,
            lcm_store_revision: None,
        }
    }

    /// Attaches only opaque LCM binding metadata supplied by the host.
    pub fn with_lcm_binding(
        mut self,
        timeline_id: impl Into<String>,
        binding_revision: impl Into<String>,
        store_revision: impl Into<String>,
    ) -> Self {
        self.lcm_timeline_id = Some(timeline_id.into());
        self.lcm_binding_revision = Some(binding_revision.into());
        self.lcm_store_revision = Some(store_revision.into());
        self
    }
}

/// A redaction-safe final context segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeContextSegmentLink {
    pub id: String,
    pub kind: String,
    pub sensitivity: String,
    pub content_hash: String,
    pub tokens: u32,
}

/// One final runtime summary and the segment identities it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSummaryCoverageLink {
    pub summary: String,
    pub covered: Vec<String>,
}

/// Stable classification/provenance for a lossless summary node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeClassificationLink {
    pub sensitivity: String,
    pub trust: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guard_revisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transformation_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transformation_revisions: Vec<String>,
}

/// Redaction-safe metadata for one LCM summary node observed in a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeLosslessSummaryLink {
    pub summary: String,
    pub covered: Vec<String>,
    pub timeline_id: String,
    pub node_id: String,
    pub dag_revision: u64,
    pub node_revision: u64,
    pub authorization_revision: String,
    pub store_revision: String,
    pub store_view_revision: String,
    pub source_range_start: u64,
    pub source_range_end: u64,
    pub covered_count: u64,
    pub source_tokens: u64,
    pub token_count: u64,
    pub source_fingerprint: String,
    pub policy_revision: String,
    pub algorithm_revision: String,
    pub sizer_revision: String,
    pub summary_revision: String,
    pub classification: RuntimeClassificationLink,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_fingerprint: Option<String>,
}

impl RuntimeLosslessSummaryLink {
    fn from_runtime(record: &agent_runtime::core::manifest::LosslessSummaryRecord) -> Self {
        let classification = &record.classification;
        Self {
            summary: record.summary.as_str().to_owned(),
            covered: record
                .covered
                .iter()
                .map(|segment| segment.as_str().to_owned())
                .collect(),
            timeline_id: record.timeline_id.clone(),
            node_id: record.node_id.clone(),
            dag_revision: record.dag_revision,
            node_revision: record.node_revision,
            authorization_revision: record.authorization_revision.as_str().to_owned(),
            store_revision: record.store_revision.as_str().to_owned(),
            store_view_revision: record.store_view_revision.as_str().to_owned(),
            source_range_start: record.source_range_start,
            source_range_end: record.source_range_end,
            covered_count: record.covered_count,
            source_tokens: record.source_tokens,
            token_count: record.token_count,
            source_fingerprint: record.source_fingerprint.as_str().to_owned(),
            policy_revision: record.policy_revision.as_str().to_owned(),
            algorithm_revision: record.algorithm_revision.as_str().to_owned(),
            sizer_revision: record.sizer_revision.as_str().to_owned(),
            summary_revision: record.summary_revision.as_str().to_owned(),
            classification: RuntimeClassificationLink {
                sensitivity: classification.sensitivity.as_str().to_owned(),
                trust: classification.trust.as_str().to_owned(),
                guard_revision: classification.guard_revision.clone(),
                guard_revisions: classification.guard_revisions.iter().cloned().collect(),
                transformation_revision: classification
                    .transformation_revision
                    .as_ref()
                    .map(|revision| revision.as_str().to_owned()),
                transformation_revisions: classification
                    .transformation_revisions
                    .iter()
                    .cloned()
                    .collect(),
            },
            operation_id: record.operation_id.clone(),
            operation_fingerprint: record
                .operation_fingerprint
                .as_ref()
                .map(|fingerprint| fingerprint.as_str().to_owned()),
        }
    }
}
