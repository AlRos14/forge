//! Persistence contracts for Charter-backed Project orchestration.
//!
//! This module deliberately keeps the database crate independent from the API
//! crate.  The API owns its closed wire enums and JSON contracts; the database
//! exposes stable records and mutation inputs containing the exact revision and
//! digest values which are persisted by SQLite.  Services are responsible for
//! converting between the two layers and for policy authorization.

use async_trait::async_trait;

use crate::{CreateProject, Project, Result};

/// A durable Project Charter (the identity record, not a revision).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCharterRecord {
    pub id: String,
    pub account_id: String,
    pub genesis_session_id: Option<String>,
    pub project_id: Option<String>,
    pub current_draft_revision_id: Option<String>,
    pub current_approved_revision_id: Option<String>,
    pub project_mode: String,
    pub maturity: String,
    pub lifecycle: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectCharter {
    pub id: String,
    pub account_id: String,
    pub genesis_session_id: Option<String>,
    pub project_mode: String,
    pub maturity: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCharterRevisionRecord {
    pub id: String,
    pub charter_id: String,
    pub revision: i64,
    pub base_revision: i64,
    pub base_revision_id: Option<String>,
    pub lifecycle: String,
    pub schema_version: String,
    pub render_version: String,
    pub content_json: String,
    pub rendered_view: String,
    pub change_summary: String,
    pub author_type: String,
    pub author_id: Option<String>,
    pub source_message_id: Option<String>,
    pub source_turn_job_id: Option<String>,
    pub source_refs_json: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectCharterRevision {
    pub id: String,
    pub charter_id: String,
    pub expected_charter_version: i64,
    pub project_mode: String,
    pub maturity: String,
    pub base_revision: i64,
    pub base_revision_id: Option<String>,
    pub lifecycle: String,
    pub schema_version: String,
    pub render_version: String,
    pub content_json: String,
    pub rendered_view: String,
    pub change_summary: String,
    pub author_type: String,
    pub author_id: Option<String>,
    pub source_message_id: Option<String>,
    pub source_turn_job_id: Option<String>,
    pub source_refs_json: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub created_at: String,
}

/// Atomically create (or claim) an owned Charter and append its first
/// revision.  The ownership claim and revision pointer must share a
/// transaction: a failed first revision must not leave an empty Charter
/// attached to a Project or Genesis session, and two callers racing for one
/// caller-supplied ID must have one serialized winner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectCharterRevisionAtomically {
    pub project_id: Option<String>,
    pub genesis_session_id: Option<String>,
    pub account_id: String,
    pub charter: CreateProjectCharter,
    pub revision: CreateProjectCharterRevision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCharterApprovalRecord {
    pub id: String,
    pub approval_type: String,
    pub charter_id: String,
    pub revision_id: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub expected_charter_version: i64,
    pub approved_name: Option<String>,
    pub approved_slug: Option<String>,
    pub approved_project_mode: String,
    pub selected_identity_id: Option<String>,
    pub selected_profile_id: Option<String>,
    pub selected_operating_skill_revision_id: Option<String>,
    pub selected_policy_revision: Option<String>,
    pub selected_policy_digest: Option<String>,
    pub approving_principal_type: String,
    pub approving_principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub explicit_event: String,
    pub authorization_occurred_at: String,
    pub source_action: String,
    pub approval_event_id: Option<String>,
    pub lifecycle: String,
    pub idempotency_key: String,
    pub consumed_project_id: Option<String>,
    pub consumed_at: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// An immutable, project-scoped record of two canonical claims which cannot
/// both be authoritative. The referenced records are typed IDs plus exact
/// revisions/digests; their bodies are intentionally not copied here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCanonicalConflictRecord {
    pub id: String,
    pub project_id: String,
    pub domain: String,
    pub governing_record_type: String,
    pub governing_record_id: String,
    pub governing_record_revision: String,
    pub governing_record_digest: String,
    pub conflicting_record_type: String,
    pub conflicting_record_id: String,
    pub conflicting_record_revision: String,
    pub conflicting_record_digest: String,
    pub affected_paths_json: String,
    pub conflict_code: String,
    pub description: String,
    pub detected_by_type: String,
    pub detected_by_id: Option<String>,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub explicit_event: String,
    pub authorization_occurred_at: String,
    pub idempotency_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectCanonicalConflict {
    pub id: String,
    pub project_id: String,
    pub domain: String,
    pub governing_record_type: String,
    pub governing_record_id: String,
    pub governing_record_revision: String,
    pub governing_record_digest: String,
    pub conflicting_record_type: String,
    pub conflicting_record_id: String,
    pub conflicting_record_revision: String,
    pub conflicting_record_digest: String,
    pub affected_paths_json: String,
    pub conflict_code: String,
    pub description: String,
    pub detected_by_type: String,
    pub detected_by_id: Option<String>,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub explicit_event: String,
    pub authorization_occurred_at: String,
    pub idempotency_key: String,
    pub created_at: String,
}

/// A typed reconciliation projection attached to one affected record. Its
/// state is `required` until the explicit resolution operation inserts an
/// immutable resolution event and advances this row to one of the five
/// allowed retained/revised/cancelled/superseded/invalidated outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectReconciliationRecord {
    pub id: String,
    pub project_id: String,
    pub conflict_id: String,
    pub record_type: String,
    pub record_id: String,
    pub record_revision: String,
    pub record_digest: String,
    pub governing_record_type: String,
    pub governing_record_id: String,
    pub governing_record_revision: String,
    pub governing_record_digest: String,
    pub state: String,
    pub current_resolution_id: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectReconciliation {
    pub id: String,
    pub project_id: String,
    pub conflict_id: String,
    pub record_type: String,
    pub record_id: String,
    pub record_revision: String,
    pub record_digest: String,
    pub governing_record_type: String,
    pub governing_record_id: String,
    pub governing_record_revision: String,
    pub governing_record_digest: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveProjectReconciliation {
    pub id: String,
    pub expected_version: i64,
    pub resolution_id: String,
    pub action: String,
    pub principal_type: String,
    pub principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub explicit_event: String,
    pub authorization_occurred_at: String,
    pub reason: String,
    pub occurred_at: String,
    pub idempotency_key: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveProjectCharter {
    pub id: String,
    pub approval_type: String,
    pub charter_id: String,
    pub revision_id: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub expected_charter_version: i64,
    pub approved_name: Option<String>,
    pub approved_slug: Option<String>,
    pub approved_project_mode: String,
    pub selected_identity_id: Option<String>,
    pub selected_profile_id: Option<String>,
    pub selected_operating_skill_revision_id: Option<String>,
    pub selected_policy_revision: Option<String>,
    pub selected_policy_digest: Option<String>,
    pub approving_principal_type: String,
    pub approving_principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub explicit_event: String,
    pub authorization_occurred_at: String,
    pub source_action: String,
    pub idempotency_key: String,
    pub event_id: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocumentRecord {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub title: String,
    pub lifecycle: String,
    pub approval_policy: String,
    pub current_draft_revision_id: Option<String>,
    pub current_approved_revision_id: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectDocument {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub title: String,
    pub approval_policy: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocumentRevisionRecord {
    pub id: String,
    pub document_id: String,
    pub revision: i64,
    pub base_revision: i64,
    pub base_revision_id: Option<String>,
    pub lifecycle: String,
    pub schema_version: String,
    pub render_version: String,
    pub content_json: String,
    pub rendered_view: String,
    pub change_summary: String,
    pub author_type: String,
    pub author_id: Option<String>,
    pub source_refs_json: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectDocumentRevision {
    pub id: String,
    pub document_id: String,
    pub expected_document_version: i64,
    pub base_revision: i64,
    pub base_revision_id: Option<String>,
    pub lifecycle: String,
    pub schema_version: String,
    pub render_version: String,
    pub content_json: String,
    pub rendered_view: String,
    pub change_summary: String,
    pub author_type: String,
    pub author_id: Option<String>,
    pub source_refs_json: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocumentApprovalRecord {
    pub id: String,
    pub document_id: String,
    pub revision_id: String,
    pub principal_type: String,
    pub principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub explicit_event: String,
    pub authorization_occurred_at: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub lifecycle: String,
    pub idempotency_key: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveProjectDocument {
    pub id: String,
    pub document_id: String,
    pub revision_id: String,
    pub expected_document_version: i64,
    pub principal_type: String,
    pub principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub explicit_event: String,
    pub authorization_occurred_at: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub idempotency_key: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDecisionCandidateRecord {
    pub id: String,
    pub project_id: String,
    pub lifecycle: String,
    pub question: String,
    pub context_json: String,
    pub options_json: String,
    pub selected_outcome: Option<String>,
    pub rationale: Option<String>,
    pub principal_type: Option<String>,
    pub principal_id: Option<String>,
    pub source_refs_json: String,
    pub expected_project_version: i64,
    pub effective_decision_id: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectDecisionCandidate {
    pub id: String,
    pub project_id: String,
    pub lifecycle: String,
    pub question: String,
    pub context_json: String,
    pub options_json: String,
    pub selected_outcome: Option<String>,
    pub rationale: Option<String>,
    pub principal_type: Option<String>,
    pub principal_id: Option<String>,
    pub source_refs_json: String,
    pub expected_project_version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDecisionRecord {
    pub id: String,
    pub project_id: String,
    pub state: String,
    pub decision_class: String,
    pub question: String,
    pub context_json: String,
    pub options_json: String,
    pub selected_outcome: String,
    pub rationale: String,
    pub principal_type: String,
    pub principal_id: String,
    pub authority_basis: String,
    pub authorization_action: String,
    pub explicit_event: String,
    pub authorization_occurred_at: String,
    pub charter_revision_id: Option<String>,
    pub baseline_revision_id: Option<String>,
    pub source_refs_json: String,
    pub affected_records_json: String,
    pub supersedes_decision_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectDecision {
    pub id: String,
    pub project_id: String,
    pub expected_project_version: i64,
    pub state: String,
    pub decision_class: String,
    pub question: String,
    pub context_json: String,
    pub options_json: String,
    pub selected_outcome: String,
    pub rationale: String,
    pub principal_type: String,
    pub principal_id: String,
    pub authority_basis: String,
    pub authorization_action: String,
    pub explicit_event: String,
    pub authorization_occurred_at: String,
    pub charter_revision_id: Option<String>,
    pub baseline_revision_id: Option<String>,
    pub source_refs_json: String,
    pub affected_records_json: String,
    pub supersedes_decision_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectExecutionBaselineRecord {
    pub id: String,
    pub project_id: String,
    pub current_revision_id: Option<String>,
    pub lifecycle: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectExecutionBaseline {
    pub id: String,
    pub project_id: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectExecutionBaselineRevisionRecord {
    pub id: String,
    pub baseline_id: String,
    pub revision: i64,
    pub base_revision: i64,
    pub base_revision_id: Option<String>,
    pub lifecycle: String,
    pub charter_revision_id: String,
    pub document_revisions_json: String,
    pub plan_items_json: String,
    pub milestone_id: Option<String>,
    pub milestone_ids_json: String,
    pub milestone_definition_revision_ids_json: String,
    pub primary_milestone_id: Option<String>,
    pub release_policy_json: String,
    pub release_policy_revision: String,
    pub release_policy_digest: String,
    pub acceptance_matrix_json: String,
    pub capability_classes_json: String,
    pub risk_classes_json: String,
    pub adaptive_envelope_json: String,
    pub elevated_operations_json: String,
    pub exclusions_json: String,
    pub rollback_recovery_json: String,
    pub schema_version: String,
    pub render_version: String,
    pub rendered_view: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub source_refs_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectExecutionBaselineRevision {
    pub id: String,
    pub baseline_id: String,
    pub expected_baseline_version: i64,
    pub base_revision: i64,
    pub base_revision_id: Option<String>,
    pub lifecycle: String,
    pub charter_revision_id: String,
    pub document_revisions_json: String,
    pub plan_items_json: String,
    pub milestone_id: Option<String>,
    pub milestone_ids_json: String,
    pub milestone_definition_revision_ids_json: String,
    pub primary_milestone_id: Option<String>,
    pub release_policy_json: String,
    pub release_policy_revision: String,
    pub release_policy_digest: String,
    pub acceptance_matrix_json: String,
    pub capability_classes_json: String,
    pub risk_classes_json: String,
    pub adaptive_envelope_json: String,
    pub elevated_operations_json: String,
    pub exclusions_json: String,
    pub rollback_recovery_json: String,
    pub schema_version: String,
    pub render_version: String,
    pub rendered_view: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub source_refs_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectExecutionBaselineApprovalRecord {
    pub id: String,
    pub baseline_id: String,
    pub revision_id: String,
    pub expected_project_version: i64,
    pub principal_type: String,
    pub principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub authorization_occurred_at: String,
    pub explicit_event: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub lifecycle: String,
    pub idempotency_key: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveProjectExecutionBaseline {
    pub id: String,
    pub baseline_id: String,
    pub revision_id: String,
    pub expected_baseline_version: i64,
    pub expected_project_version: i64,
    pub principal_type: String,
    pub principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub authorization_occurred_at: String,
    pub explicit_event: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub idempotency_key: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivateProjectExecutionBaseline {
    pub approval_id: String,
    pub expected_baseline_version: i64,
    pub expected_project_version: i64,
    pub idempotency_key: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMilestoneRecord {
    pub id: String,
    pub project_id: String,
    pub milestone_sequence: i64,
    pub milestone_key: String,
    pub display_label: Option<String>,
    pub current_definition_revision_id: Option<String>,
    pub lifecycle: String,
    pub blocker_reason_json: String,
    pub stale_reason_json: String,
    pub reconciliation_reason_json: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectMilestone {
    pub id: String,
    pub project_id: String,
    pub expected_project_version: i64,
    pub milestone_sequence: i64,
    pub milestone_key: String,
    pub display_label: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMilestoneRevisionRecord {
    pub id: String,
    pub milestone_id: String,
    pub revision: i64,
    pub base_revision: i64,
    pub base_revision_id: Option<String>,
    pub lifecycle: String,
    pub display_label: Option<String>,
    pub outcome: String,
    pub included_scope_json: String,
    pub excluded_scope_json: String,
    pub charter_revision_id: Option<String>,
    pub document_revisions_json: String,
    pub task_selection_json: String,
    pub dependencies_json: String,
    pub risks_json: String,
    pub acceptance_checks_json: String,
    pub evidence_requirements_json: String,
    pub known_issues_json: String,
    pub change_summary: String,
    pub schema_version: String,
    pub render_version: String,
    pub rendered_view: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub author_type: String,
    pub author_id: Option<String>,
    pub source_refs_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectMilestoneRevision {
    pub id: String,
    pub milestone_id: String,
    pub expected_milestone_version: i64,
    pub base_revision: i64,
    pub base_revision_id: Option<String>,
    pub lifecycle: String,
    pub display_label: Option<String>,
    pub outcome: String,
    pub included_scope_json: String,
    pub excluded_scope_json: String,
    pub charter_revision_id: Option<String>,
    pub document_revisions_json: String,
    pub task_selection_json: String,
    pub dependencies_json: String,
    pub risks_json: String,
    pub acceptance_checks_json: String,
    pub evidence_requirements_json: String,
    pub known_issues_json: String,
    pub change_summary: String,
    pub schema_version: String,
    pub render_version: String,
    pub rendered_view: String,
    pub content_digest: String,
    pub rendered_digest: String,
    pub author_type: String,
    pub author_id: Option<String>,
    pub source_refs_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMilestoneCheckRecord {
    pub id: String,
    pub project_id: String,
    pub milestone_id: String,
    pub definition_revision_id: String,
    pub check_key: String,
    pub description: String,
    pub required: bool,
    pub source_kind: String,
    pub expected_result: String,
    pub evidence_required: bool,
    pub version: i64,
    pub current_result_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectMilestoneCheck {
    pub id: String,
    pub project_id: String,
    pub milestone_id: String,
    pub definition_revision_id: String,
    pub expected_milestone_version: i64,
    pub check_key: String,
    pub description: String,
    pub required: bool,
    pub source_kind: String,
    pub expected_result: String,
    pub evidence_required: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMilestoneCheckResultRecord {
    pub id: String,
    pub project_id: String,
    pub milestone_id: String,
    pub check_id: String,
    pub definition_revision_id: String,
    pub outcome: String,
    pub source_kind: String,
    pub source_manifest_json: String,
    pub input_digest: String,
    pub governing_charter_revision_id: Option<String>,
    pub governing_baseline_revision_id: Option<String>,
    pub principal_type: String,
    pub principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub authorization_occurred_at: String,
    pub expected_version: i64,
    pub explicit_event: String,
    pub idempotency_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectMilestoneCheckResult {
    pub id: String,
    pub project_id: String,
    pub milestone_id: String,
    pub check_id: String,
    pub definition_revision_id: String,
    pub outcome: String,
    pub source_kind: String,
    pub source_manifest_json: String,
    pub input_digest: String,
    pub governing_charter_revision_id: Option<String>,
    pub governing_baseline_revision_id: Option<String>,
    pub principal_type: String,
    pub principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub authorization_occurred_at: String,
    pub expected_version: i64,
    pub explicit_event: String,
    pub idempotency_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectReadinessSnapshotRecord {
    pub id: String,
    pub project_id: String,
    pub milestone_id: String,
    pub definition_revision_id: String,
    pub baseline_id: String,
    pub baseline_revision_id: String,
    pub baseline_digest: String,
    pub release_policy_revision: String,
    pub release_policy_digest: String,
    pub input_manifest_json: String,
    pub event_watermark: String,
    pub outcome: String,
    pub blocking_reasons_json: String,
    pub check_results_json: String,
    pub waiver_manifest_json: String,
    pub evidence_manifest_json: String,
    pub commit_context_json: String,
    pub computing_policy_revision: String,
    pub readiness_digest: String,
    pub principal_type: String,
    pub principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub authorization_occurred_at: String,
    pub expected_milestone_version: i64,
    pub explicit_event: String,
    pub idempotency_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectReadinessSnapshot {
    pub id: String,
    pub project_id: String,
    pub milestone_id: String,
    pub definition_revision_id: String,
    pub baseline_id: String,
    pub baseline_revision_id: String,
    pub baseline_digest: String,
    pub release_policy_revision: String,
    pub release_policy_digest: String,
    pub input_manifest_json: String,
    pub event_watermark: String,
    pub outcome: String,
    pub blocking_reasons_json: String,
    pub check_results_json: String,
    pub waiver_manifest_json: String,
    pub evidence_manifest_json: String,
    pub commit_context_json: String,
    pub computing_policy_revision: String,
    pub readiness_digest: String,
    pub principal_type: String,
    pub principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub authorization_occurred_at: String,
    pub expected_milestone_version: i64,
    pub explicit_event: String,
    pub idempotency_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectReleaseRecord {
    pub id: String,
    pub project_id: String,
    pub milestone_id: String,
    pub release_sequence: i64,
    pub release_revision: i64,
    pub release_identifier: String,
    pub milestone_revision_id: String,
    pub readiness_snapshot_id: String,
    pub readiness_digest: String,
    pub baseline_id: String,
    pub baseline_revision_id: String,
    pub baseline_digest: String,
    pub release_policy_revision: String,
    pub release_policy_digest: String,
    pub summary: String,
    pub changelog: String,
    pub known_issues_json: String,
    pub charter_revision_id: Option<String>,
    pub document_revisions_json: String,
    pub decision_ids_json: String,
    pub task_references_json: String,
    pub validation_references_json: String,
    pub git_references_json: String,
    pub evidence_references_json: String,
    pub waivers_json: String,
    pub releasing_principal_type: String,
    pub releasing_principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub authorization_occurred_at: String,
    pub explicit_event: String,
    pub schema_version: String,
    pub snapshot_digest: String,
    pub idempotency_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectRelease {
    pub id: String,
    pub project_id: String,
    pub milestone_id: String,
    pub expected_milestone_version: i64,
    pub release_sequence: i64,
    pub release_revision: i64,
    pub release_identifier: String,
    pub milestone_revision_id: String,
    pub readiness_snapshot_id: String,
    pub readiness_digest: String,
    pub baseline_id: String,
    pub baseline_revision_id: String,
    pub baseline_digest: String,
    pub release_policy_revision: String,
    pub release_policy_digest: String,
    pub summary: String,
    pub changelog: String,
    pub known_issues_json: String,
    pub charter_revision_id: Option<String>,
    pub document_revisions_json: String,
    pub decision_ids_json: String,
    pub task_references_json: String,
    pub validation_references_json: String,
    pub git_references_json: String,
    pub evidence_references_json: String,
    pub waivers_json: String,
    pub releasing_principal_type: String,
    pub releasing_principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub authorization_occurred_at: String,
    pub explicit_event: String,
    pub schema_version: String,
    pub snapshot_digest: String,
    pub idempotency_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectReleaseReferenceRecord {
    pub release_id: String,
    pub ordinal: i64,
    pub reference_kind: String,
    pub record_id: String,
    pub record_version: Option<String>,
    pub record_state: Option<String>,
    pub record_digest: Option<String>,
    pub metadata_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectReleaseReference {
    pub release_id: String,
    pub ordinal: i64,
    pub reference_kind: String,
    pub record_id: String,
    pub record_version: Option<String>,
    pub record_state: Option<String>,
    pub record_digest: Option<String>,
    pub metadata_json: String,
}

/// Inputs for the one transaction which turns an exact Charter approval into
/// a Project, Project Agent binding, and typed handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectFromCharterApproval {
    pub approval_id: String,
    pub idempotency_key: String,
    pub account_id: String,
    pub project: CreateProject,
    pub project_agent_binding_id: String,
    pub handoff_id: String,
    pub target_message_id: String,
    pub target_turn_id: String,
    /// Historical Main identity that authored the Genesis source turn.  This
    /// must not be reconstructed from the current account Main binding after
    /// discovery has completed.
    pub source_identity_id: Option<String>,
    pub source_profile_id: Option<String>,
    pub source_instruction_revision_id: Option<String>,
    pub source_message_id: Option<String>,
    pub source_turn_id: Option<String>,
    pub handoff_content: String,
    pub content_guard_json: String,
    pub source_revisions_json: String,
    /// The authenticated user authorization which consumes the approval and
    /// creates the Project. This is deliberately distinct from the Charter
    /// approval provenance: approving a Charter and materializing its Project
    /// are two separate user actions.
    pub create_principal_type: String,
    pub create_principal_id: String,
    pub create_authorization_basis: String,
    pub create_action: String,
    pub create_event_id: String,
    pub create_occurred_at: String,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub causation_depth: i64,
    pub max_attempts: i64,
    pub policy_revision: String,
    pub policy_digest: String,
    pub member_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedProjectFromCharterApproval {
    pub project: Project,
    pub project_agent_binding_id: String,
    pub project_chat_id: String,
    pub charter_id: String,
    pub charter_revision_id: String,
    pub handoff_id: String,
    pub target_message_id: String,
    pub target_turn_id: String,
}

#[async_trait]
pub trait ProjectOrchestrationRepo: Send + Sync {
    async fn get_project_charter(&self, id: &str) -> Result<Option<ProjectCharterRecord>>;
    async fn get_project_charter_for_account(
        &self,
        id: &str,
        account_id: &str,
    ) -> Result<Option<ProjectCharterRecord>>;
    async fn create_project_charter(
        &self,
        input: CreateProjectCharter,
    ) -> Result<ProjectCharterRecord>;
    async fn get_project_charter_revision(
        &self,
        id: &str,
    ) -> Result<Option<ProjectCharterRevisionRecord>>;
    async fn list_project_charter_revisions(
        &self,
        charter_id: &str,
    ) -> Result<Vec<ProjectCharterRevisionRecord>>;
    async fn create_project_charter_revision(
        &self,
        input: CreateProjectCharterRevision,
    ) -> Result<ProjectCharterRevisionRecord>;
    async fn create_project_charter_revision_atomically(
        &self,
        input: CreateProjectCharterRevisionAtomically,
    ) -> Result<ProjectCharterRevisionRecord>;
    async fn get_project_charter_approval(
        &self,
        id: &str,
    ) -> Result<Option<ProjectCharterApprovalRecord>>;
    async fn approve_project_charter(
        &self,
        input: ApproveProjectCharter,
    ) -> Result<ProjectCharterApprovalRecord>;
    async fn create_project_from_charter_approval(
        &self,
        input: CreateProjectFromCharterApproval,
    ) -> Result<CreatedProjectFromCharterApproval>;

    async fn create_project_canonical_conflict(
        &self,
        input: CreateProjectCanonicalConflict,
    ) -> Result<ProjectCanonicalConflictRecord>;
    async fn get_project_canonical_conflict(
        &self,
        id: &str,
    ) -> Result<Option<ProjectCanonicalConflictRecord>>;
    async fn list_project_canonical_conflicts(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectCanonicalConflictRecord>>;
    async fn create_project_reconciliation(
        &self,
        input: CreateProjectReconciliation,
    ) -> Result<ProjectReconciliationRecord>;
    async fn get_project_reconciliation(
        &self,
        id: &str,
    ) -> Result<Option<ProjectReconciliationRecord>>;
    async fn list_project_reconciliations(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectReconciliationRecord>>;
    async fn resolve_project_reconciliation(
        &self,
        input: ResolveProjectReconciliation,
    ) -> Result<ProjectReconciliationRecord>;

    async fn create_project_document(
        &self,
        input: CreateProjectDocument,
    ) -> Result<ProjectDocumentRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Project document persistence is not wired".to_owned(),
        ))
    }
    async fn get_project_document(&self, id: &str) -> Result<Option<ProjectDocumentRecord>> {
        let _ = id;
        Err(crate::DbError::Check(
            "Project document persistence is not wired".to_owned(),
        ))
    }
    async fn create_project_document_revision(
        &self,
        input: CreateProjectDocumentRevision,
    ) -> Result<ProjectDocumentRevisionRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Project document persistence is not wired".to_owned(),
        ))
    }
    async fn approve_project_document(
        &self,
        input: ApproveProjectDocument,
    ) -> Result<ProjectDocumentApprovalRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Project document persistence is not wired".to_owned(),
        ))
    }

    async fn get_project_document_revision(
        &self,
        id: &str,
    ) -> Result<Option<ProjectDocumentRevisionRecord>> {
        let _ = id;
        Err(crate::DbError::Check(
            "Project document persistence is not wired".to_owned(),
        ))
    }
    async fn list_project_document_revisions(
        &self,
        document_id: &str,
    ) -> Result<Vec<ProjectDocumentRevisionRecord>> {
        let _ = document_id;
        Err(crate::DbError::Check(
            "Project document persistence is not wired".to_owned(),
        ))
    }

    async fn create_project_decision_candidate(
        &self,
        input: CreateProjectDecisionCandidate,
    ) -> Result<ProjectDecisionCandidateRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Project decision persistence is not wired".to_owned(),
        ))
    }
    async fn get_project_decision_candidate(
        &self,
        id: &str,
    ) -> Result<Option<ProjectDecisionCandidateRecord>> {
        let _ = id;
        Err(crate::DbError::Check(
            "Project decision persistence is not wired".to_owned(),
        ))
    }
    async fn append_project_decision(
        &self,
        input: CreateProjectDecision,
    ) -> Result<ProjectDecisionRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Project decision persistence is not wired".to_owned(),
        ))
    }
    async fn list_project_decision_candidates(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectDecisionCandidateRecord>> {
        let _ = project_id;
        Err(crate::DbError::Check(
            "Project decision persistence is not wired".to_owned(),
        ))
    }
    async fn list_effective_project_decisions(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectDecisionRecord>> {
        let _ = project_id;
        Err(crate::DbError::Check(
            "Project decision persistence is not wired".to_owned(),
        ))
    }

    async fn create_project_execution_baseline(
        &self,
        input: CreateProjectExecutionBaseline,
    ) -> Result<ProjectExecutionBaselineRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Execution baseline persistence is not wired".to_owned(),
        ))
    }
    async fn get_project_execution_baseline(
        &self,
        id: &str,
    ) -> Result<Option<ProjectExecutionBaselineRecord>> {
        let _ = id;
        Err(crate::DbError::Check(
            "Execution baseline persistence is not wired".to_owned(),
        ))
    }
    async fn get_project_execution_baseline_revision(
        &self,
        id: &str,
    ) -> Result<Option<ProjectExecutionBaselineRevisionRecord>> {
        let _ = id;
        Err(crate::DbError::Check(
            "Execution baseline persistence is not wired".to_owned(),
        ))
    }
    async fn approve_project_execution_baseline(
        &self,
        input: ApproveProjectExecutionBaseline,
    ) -> Result<ProjectExecutionBaselineApprovalRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Execution baseline persistence is not wired".to_owned(),
        ))
    }
    async fn create_project_execution_baseline_revision(
        &self,
        input: CreateProjectExecutionBaselineRevision,
    ) -> Result<ProjectExecutionBaselineRevisionRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Execution baseline persistence is not wired".to_owned(),
        ))
    }
    async fn activate_project_execution_baseline(
        &self,
        input: ActivateProjectExecutionBaseline,
    ) -> Result<ProjectExecutionBaselineRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Execution baseline persistence is not wired".to_owned(),
        ))
    }

    async fn create_project_milestone(
        &self,
        input: CreateProjectMilestone,
    ) -> Result<ProjectMilestoneRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Milestone persistence is not wired".to_owned(),
        ))
    }
    async fn list_project_milestones(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectMilestoneRecord>> {
        let _ = project_id;
        Err(crate::DbError::Check(
            "Milestone persistence is not wired".to_owned(),
        ))
    }
    async fn get_project_milestone_revision(
        &self,
        id: &str,
    ) -> Result<Option<ProjectMilestoneRevisionRecord>> {
        let _ = id;
        Err(crate::DbError::Check(
            "Milestone persistence is not wired".to_owned(),
        ))
    }
    async fn list_project_milestone_revisions(
        &self,
        milestone_id: &str,
    ) -> Result<Vec<ProjectMilestoneRevisionRecord>> {
        let _ = milestone_id;
        Err(crate::DbError::Check(
            "Milestone persistence is not wired".to_owned(),
        ))
    }
    async fn create_project_milestone_revision(
        &self,
        input: CreateProjectMilestoneRevision,
    ) -> Result<ProjectMilestoneRevisionRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Milestone persistence is not wired".to_owned(),
        ))
    }
    async fn get_project_milestone(&self, id: &str) -> Result<Option<ProjectMilestoneRecord>> {
        let _ = id;
        Err(crate::DbError::Check(
            "Milestone persistence is not wired".to_owned(),
        ))
    }
    async fn create_project_milestone_check(
        &self,
        input: CreateProjectMilestoneCheck,
    ) -> Result<ProjectMilestoneCheckRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Milestone persistence is not wired".to_owned(),
        ))
    }
    async fn append_project_milestone_check_result(
        &self,
        input: CreateProjectMilestoneCheckResult,
    ) -> Result<ProjectMilestoneCheckResultRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Milestone persistence is not wired".to_owned(),
        ))
    }
    async fn create_project_readiness_snapshot(
        &self,
        input: CreateProjectReadinessSnapshot,
    ) -> Result<ProjectReadinessSnapshotRecord> {
        let _ = input;
        Err(crate::DbError::Check(
            "Readiness persistence is not wired".to_owned(),
        ))
    }
    async fn create_project_release(
        &self,
        input: CreateProjectRelease,
        references: Vec<CreateProjectReleaseReference>,
    ) -> Result<ProjectReleaseRecord> {
        let _ = (input, references);
        Err(crate::DbError::Check(
            "Release persistence is not wired".to_owned(),
        ))
    }
    async fn list_project_release_references(
        &self,
        release_id: &str,
    ) -> Result<Vec<ProjectReleaseReferenceRecord>> {
        let _ = release_id;
        Err(crate::DbError::Check(
            "Release persistence is not wired".to_owned(),
        ))
    }
}
