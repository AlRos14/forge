//! Deterministic execution-baseline rendering and persistence-shape helpers.
//!
//! The database keeps the baseline columns normalized for gate queries.  This
//! module is the typed boundary that derives those columns from one closed API
//! payload and computes the exact digests shown to the approving user.

use api_types::{
    canonical_digest_with_schema, canonical_render_digest, ExecutionBaselineContent,
    ExecutionBaselineReleasePolicy,
};
use serde_json::json;
use std::collections::HashSet;

pub const EXECUTION_BASELINE_SCHEMA_VERSION: &str = "forge.execution-baseline/v1";
pub const EXECUTION_BASELINE_RENDER_VERSION: &str = "forge.execution-baseline-render/v1";
pub const EXECUTION_BASELINE_RELEASE_POLICY_SCHEMA: &str =
    "forge.execution-baseline-release-policy/v1";

/// Compute the authority digest for the complete, closed release policy.
/// Callers must never accept a client-supplied opaque policy digest without
/// comparing it with this value.
pub fn release_policy_digest(
    policy: &ExecutionBaselineReleasePolicy,
) -> Result<String, serde_json::Error> {
    canonical_digest_with_schema(EXECUTION_BASELINE_RELEASE_POLICY_SCHEMA, policy)
}

/// Validate the frozen policy carried by a baseline before it is persisted or
/// proposed by a Project Agent.  This is intentionally shared by the HTTP
/// user path and the server-owned typed action materializer so neither path
/// can accept an opaque revision/digest pair.
pub fn validate_execution_baseline_policy(
    content: &ExecutionBaselineContent,
) -> Result<(), String> {
    if content.release_policy.schema_version != EXECUTION_BASELINE_RELEASE_POLICY_SCHEMA
        || content.release_policy.revision != content.release_policy_revision
    {
        return Err(
            "the baseline release policy must use the declared Forge schema and revision"
                .to_owned(),
        );
    }
    let computed_policy_digest = release_policy_digest(&content.release_policy)
        .map_err(|error| format!("invalid release policy: {error}"))?;
    if content.release_policy_digest != computed_policy_digest {
        return Err(
            "the release policy digest does not match the complete frozen policy payload"
                .to_owned(),
        );
    }
    if content.release_policy.revision.trim().is_empty() {
        return Err("the frozen release policy revision cannot be empty".to_owned());
    }

    validate_identifier_rules(
        "required_check_definition_revisions",
        &content.release_policy.required_check_definition_revisions,
        true,
    )?;
    validate_literal_rules(
        "reviewer_independence_rules",
        &content.release_policy.reviewer_independence_rules,
        &["independent-reviewer"],
        true,
    )?;
    validate_literal_rules(
        "manual_attestation_rules",
        &content.release_policy.manual_attestation_rules,
        &["manual-attestation"],
        false,
    )?;
    validate_literal_rules(
        "waiver_rules",
        &content.release_policy.waiver_rules,
        &["user-waiver"],
        false,
    )?;
    validate_literal_rules(
        "evidence_kinds",
        &content.release_policy.evidence_kinds,
        &[
            "artifact",
            "ci-log",
            "media",
            "review-report",
            "test-report",
        ],
        true,
    )?;
    validate_literal_rules(
        "evidence_contexts",
        &content.release_policy.evidence_contexts,
        &[
            "commit",
            "external",
            "milestone",
            "project",
            "repository",
            "task",
        ],
        true,
    )?;
    validate_literal_rules(
        "evidence_freshness_rules",
        &content.release_policy.evidence_freshness_rules,
        &[
            "current-baseline",
            "current-charter",
            "current-commit",
            "current-milestone",
        ],
        true,
    )?;
    validate_literal_rules(
        "dependency_rules",
        &content.release_policy.dependency_rules,
        &[
            "dependencies-green",
            "dependencies-reviewed",
            "no-blocked-dependencies",
        ],
        true,
    )?;
    validate_literal_rules(
        "stale_input_rules",
        &content.release_policy.stale_input_rules,
        &["stale-baseline-blocks", "stale-evidence-blocks"],
        true,
    )?;
    validate_literal_rules(
        "forbidden_side_effects",
        &content.release_policy.forbidden_side_effects,
        &[
            "credential-access",
            "cross-project-write",
            "force-push",
            "merge",
            "publish",
            "release",
        ],
        true,
    )?;
    validate_literal_rules(
        "known_issue_rules",
        &content.release_policy.known_issue_rules,
        &[
            "known-issue-blocks",
            "known-issue-waiver",
            "record-known-issue",
        ],
        true,
    )?;
    validate_literal_rules(
        "correction_rules",
        &content.release_policy.correction_rules,
        &[
            "correct-before-release",
            "correction-required",
            "rerun-failed-checks",
        ],
        true,
    )?;
    validate_literal_rules(
        "purge_rules",
        &content.release_policy.purge_rules,
        &[
            "purge-invalid-evidence",
            "purge-revoked-evidence",
            "purge-stale-evidence",
        ],
        true,
    )?;
    Ok(())
}

fn validate_identifier_rules(field: &str, values: &[String], required: bool) -> Result<(), String> {
    if required && values.is_empty() {
        return Err(format!("release policy field '{field}' must not be empty"));
    }
    let mut seen = HashSet::new();
    let mut previous: Option<&str> = None;
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed != value {
            return Err(format!(
                "release policy field '{field}' contains an invalid identifier"
            ));
        }
        if !trimmed.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        }) {
            return Err(format!(
                "release policy field '{field}' contains an invalid identifier"
            ));
        }
        if !seen.insert(trimmed) {
            return Err(format!(
                "release policy field '{field}' contains a duplicate rule"
            ));
        }
        if previous.is_some_and(|previous| previous >= trimmed) {
            return Err(format!(
                "release policy field '{field}' must use canonical lexicographic order"
            ));
        }
        previous = Some(trimmed);
    }
    Ok(())
}

fn validate_literal_rules(
    field: &str,
    values: &[String],
    supported: &[&str],
    required: bool,
) -> Result<(), String> {
    if required && values.is_empty() {
        return Err(format!("release policy field '{field}' must not be empty"));
    }
    let mut seen = HashSet::new();
    let mut previous: Option<&str> = None;
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed != value || !supported.contains(&trimmed) {
            return Err(format!(
                "release policy field '{field}' contains unsupported rule '{value}'"
            ));
        }
        if !seen.insert(trimmed) {
            return Err(format!(
                "release policy field '{field}' contains a duplicate rule"
            ));
        }
        if previous.is_some_and(|previous| previous >= trimmed) {
            return Err(format!(
                "release policy field '{field}' must use canonical lexicographic order"
            ));
        }
        previous = Some(trimmed);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionBaselineRender {
    pub rendered_view: String,
    pub content_digest: String,
    pub render_digest: String,
}

/// Render the exact baseline review target.  JSON is deliberately used as the
/// review view here: it is bounded, deterministic, and preserves every
/// approval-bearing field without a second hand-written Markdown authority.
pub fn render_execution_baseline(
    content: &ExecutionBaselineContent,
) -> Result<ExecutionBaselineRender, serde_json::Error> {
    let rendered_view = serde_json::to_string_pretty(content)?;
    let content_digest = canonical_digest_with_schema(EXECUTION_BASELINE_SCHEMA_VERSION, content)?;
    let render_digest = canonical_render_digest(EXECUTION_BASELINE_RENDER_VERSION, &rendered_view)?;
    Ok(ExecutionBaselineRender {
        rendered_view,
        content_digest,
        render_digest,
    })
}

/// Encode the typed content into the existing V076 normalized columns.  The
/// source payload is still retained in `source_refs_json` by the API adapter;
/// these projections exist so task admission can query without decoding the
/// entire canonical bundle.
pub fn baseline_column_json(
    content: &ExecutionBaselineContent,
) -> Result<BaselineColumnJson, serde_json::Error> {
    Ok(BaselineColumnJson {
        document_revisions_json: serde_json::to_string(&content.document_revisions)?,
        plan_items_json: serde_json::to_string(&content.plan_item_ids)?,
        release_policy_json: serde_json::to_string(&json!({
            "revision": content.release_policy_revision,
            "digest": content.release_policy_digest,
            "policy": content.release_policy,
            "reviewer_independence_rules": content.reviewer_independence_rules,
        }))?,
        acceptance_matrix_json: serde_json::to_string(&content.acceptance_evidence_matrix)?,
        capability_classes_json: serde_json::to_string(&content.capability_classes)?,
        risk_classes_json: serde_json::to_string(&content.risk_classes)?,
        adaptive_envelope_json: serde_json::to_string(&content.adaptive_envelope)?,
        elevated_operations_json: serde_json::to_string(&content.elevated_operations)?,
        exclusions_json: serde_json::to_string(&content.exclusions)?,
        rollback_recovery_json: serde_json::to_string(&content.rollback_and_recovery)?,
        // Keep this as an ordered projection.  The definition at index `i`
        // governs the milestone at index `i`; treating these as independent
        // sets would allow a valid definition to be silently paired with the
        // wrong milestone during activation or Task admission.
        milestone_definition_revision_ids_json: serde_json::to_string(
            &content.milestone_definition_revision_ids,
        )?,
        milestone_id: content
            .primary_milestone_id
            .clone()
            .or_else(|| content.milestone_ids.first().cloned()),
        primary_milestone_id: content.primary_milestone_id.clone(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineColumnJson {
    pub document_revisions_json: String,
    pub plan_items_json: String,
    pub milestone_id: Option<String>,
    pub primary_milestone_id: Option<String>,
    pub release_policy_json: String,
    pub acceptance_matrix_json: String,
    pub capability_classes_json: String,
    pub risk_classes_json: String,
    pub adaptive_envelope_json: String,
    pub elevated_operations_json: String,
    pub exclusions_json: String,
    pub rollback_recovery_json: String,
    pub milestone_definition_revision_ids_json: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_types::{
        AdaptiveEnvelope, ArtifactRef, ExecutionBaselineContent, ExecutionBaselineReleasePolicy,
    };

    fn content() -> ExecutionBaselineContent {
        let release_policy = ExecutionBaselineReleasePolicy {
            schema_version: EXECUTION_BASELINE_RELEASE_POLICY_SCHEMA.to_owned(),
            revision: "policy-r1".to_owned(),
            required_check_definition_revisions: vec!["check-r1".to_owned()],
            reviewer_independence_rules: vec!["independent-reviewer".to_owned()],
            manual_attestation_rules: vec!["manual-attestation".to_owned()],
            waiver_rules: vec!["user-waiver".to_owned()],
            evidence_kinds: vec!["test-report".to_owned()],
            evidence_contexts: vec!["repository".to_owned()],
            evidence_freshness_rules: vec!["current-commit".to_owned()],
            dependency_rules: vec!["dependencies-green".to_owned()],
            stale_input_rules: vec!["stale-baseline-blocks".to_owned()],
            forbidden_side_effects: vec!["publish".to_owned()],
            known_issue_rules: vec!["record-known-issue".to_owned()],
            correction_rules: vec!["correct-before-release".to_owned()],
            purge_rules: vec!["purge-invalid-evidence".to_owned()],
        };
        ExecutionBaselineContent {
            charter_revision: ArtifactRef {
                artifact_id: "charter".to_owned(),
                revision_id: "charter-r1".to_owned(),
                content_digest: "charter-digest".to_owned(),
                render_version: None,
                render_digest: None,
            },
            document_revisions: Vec::new(),
            plan_item_ids: vec!["plan-1".to_owned()],
            milestone_ids: vec!["milestone-1".to_owned()],
            milestone_definition_revision_ids: vec!["milestone-definition-1".to_owned()],
            primary_milestone_id: Some("milestone-1".to_owned()),
            release_policy_revision: "policy-r1".to_owned(),
            release_policy_digest: release_policy_digest(&release_policy).expect("policy digest"),
            release_policy,
            acceptance_evidence_matrix: Vec::new(),
            capability_classes: vec!["repository_write".to_owned()],
            risk_classes: vec!["low".to_owned()],
            reviewer_independence_rules: Vec::new(),
            elevated_operations: Vec::new(),
            adaptive_envelope: AdaptiveEnvelope {
                allowed_task_operations: vec!["split".to_owned()],
                fixed_outcomes: Vec::new(),
                fixed_acceptance: Vec::new(),
                fixed_risk_classes: vec!["low".to_owned()],
                forbidden_side_effects: Vec::new(),
                elevated_operations: Vec::new(),
            },
            rollback_and_recovery: Vec::new(),
            exclusions: Vec::new(),
        }
    }

    #[test]
    fn render_and_columns_are_stable() {
        let rendered = render_execution_baseline(&content()).expect("render baseline");
        assert!(!rendered.content_digest.is_empty());
        assert!(!rendered.render_digest.is_empty());
        assert!(rendered.rendered_view.contains("repository_write"));
        let columns = baseline_column_json(&content()).expect("columns");
        assert_eq!(columns.milestone_id.as_deref(), Some("milestone-1"));
        assert!(columns.plan_items_json.contains("plan-1"));
        assert_eq!(
            columns.milestone_definition_revision_ids_json,
            r#"["milestone-definition-1"]"#
        );
    }

    #[test]
    fn release_policy_rejects_unknown_and_duplicate_rules() {
        let mut unknown = content();
        unknown.release_policy.evidence_kinds = vec!["arbitrary-rule".to_owned()];
        unknown.release_policy_digest =
            release_policy_digest(&unknown.release_policy).expect("policy digest");
        assert!(validate_execution_baseline_policy(&unknown)
            .expect_err("unknown rule must fail closed")
            .contains("unsupported"));

        let mut duplicate = content();
        duplicate.release_policy.evidence_contexts =
            vec!["repository".to_owned(), "repository".to_owned()];
        duplicate.release_policy_digest =
            release_policy_digest(&duplicate.release_policy).expect("policy digest");
        assert!(validate_execution_baseline_policy(&duplicate)
            .expect_err("duplicate rule must fail closed")
            .contains("duplicate"));
    }
}
