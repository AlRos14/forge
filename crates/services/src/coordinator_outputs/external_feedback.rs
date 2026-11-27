//! `external_feedback` triage turn, `schema_version: 2` (plan:
//! coordinator-verification-and-pr-loop, «Congelado 11» D3', node P2).
//!
//! Parsing, validation and the deterministic scope classifier are pure. The
//! only I/O in this module is [`apply_external_feedback_triage`], which records
//! the V173 triage rows and the V175 disposition rows in ONE transaction and
//! returns the effects the wiring node (RT) and the PR node (P3) execute:
//!
//! - `extend_task`: the stored `work_unit_plan` plus the item ids; RT calls
//!   `PrReworkService::supersede_for_rework` and the resume that materializes it.
//! - `propose_task`: a synthesized `ask_human` (`approve` carries
//!   `authority_effect = scope`), raised by RT with the exact resume binding.
//! - `human`: the coordinator's own `ask_human`, raised by RT.
//! - `dismiss`: an immutable receipt (nothing else to execute).
//!
//! The human decisions are raised through `HumanDecisionService::raise`, which
//! owns its transaction; they are idempotent by `idx_human_decision_exact_source`
//! (`source_kind = external_feedback`, `source_id = feedback_item_id`) so a
//! replay after a crash is safe.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use api_types::ActorRef as ApiActorRef;
use db::{
    now_rfc3339, AuthorityEffect, CreateExternalFeedbackTriage, ExternalFeedbackDisposition,
    ExternalFeedbackRepo, SqliteDb,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;

use super::{
    ensure_new_kind_permitted, AskHumanOutput, CoordinatorOutput, CoordinatorOutputKind,
    EvidenceKind, EvidenceLink, HumanDecisionOption, MicroFixOutput, OutputError, OutputResult,
    CODE_OUTPUT_NOT_PERMITTED, MAX_NEW_OUTPUTS_PER_TURN,
};
use crate::task_coordination::{CoordinationEnvelope, PlanWorkUnit};
use crate::{Result, ServiceError};

pub const EXTERNAL_FEEDBACK_SCHEMA_VERSION: u32 = 2;
pub const MAX_EXTERNAL_FEEDBACK_TURN_BYTES: usize = 64 * 1024;
pub const MAX_EXTERNAL_FEEDBACK_ITEMS: usize = 20;
pub const MAX_FEEDBACK_RATIONALE_BYTES: usize = 1024;
pub const MAX_FEEDBACK_INSTRUCTIONS_BYTES: usize = 4 * 1024;
pub const MAX_PROPOSAL_TITLE_BYTES: usize = 120;
pub const MAX_PROPOSAL_DESCRIPTION_BYTES: usize = 2 * 1024;
pub const MAX_PROPOSAL_CRITERIA: usize = 8;
pub const MAX_PROPOSAL_CRITERION_BYTES: usize = 256;
pub const MAX_PR_SUMMARY_BYTES: usize = 16 * 1024;

const MAX_PLAN_ITEMS: usize = 64;
const MAX_PLAN_KEY_BYTES: usize = 128;
const MAX_PLAN_TITLE_BYTES: usize = 512;
const MAX_PLAN_SCOPE_BYTES: usize = 4096;
const MAX_PLAN_INSTRUCTIONS_BYTES: usize = 64 * 1024;
const MAX_PLAN_ACCEPTANCE: usize = 32;
const MAX_PLAN_DEPENDENCIES: usize = 32;
const PROVENANCE_KIND: &str = "external_feedback";

pub const CODE_EXTEND_OUT_OF_SCOPE: &str = "pr_feedback.extend_out_of_scope";
pub const CODE_PLAN_WITHOUT_EXTEND: &str = "pr_feedback.plan_without_extend";

pub const PROPOSAL_OPTION_APPROVE: &str = "approve";
pub const PROPOSAL_OPTION_REJECT: &str = "reject";

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackDisposition {
    ExtendTask,
    ProposeTask,
    Human,
    Dismiss,
}

impl FeedbackDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExtendTask => "extend_task",
            Self::ProposeTask => "propose_task",
            Self::Human => "human",
            Self::Dismiss => "dismiss",
        }
    }

    /// Mandatory V173 mapping written to `task_external_feedback_triage`.
    pub fn legacy(self) -> ExternalFeedbackDisposition {
        match self {
            Self::ExtendTask => ExternalFeedbackDisposition::Correct,
            Self::ProposeTask | Self::Human => ExternalFeedbackDisposition::Human,
            Self::Dismiss => ExternalFeedbackDisposition::Dismiss,
        }
    }

    fn legacy_str(self) -> &'static str {
        match self {
            Self::ExtendTask => "correct",
            Self::ProposeTask | Self::Human => "human",
            Self::Dismiss => "dismiss",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DismissReason {
    NotActionable,
    Disagree,
    Duplicate,
    AlreadyAddressed,
    OutOfDate,
}

impl DismissReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotActionable => "not_actionable",
            Self::Disagree => "disagree",
            Self::Duplicate => "duplicate",
            Self::AlreadyAddressed => "already_addressed",
            Self::OutOfDate => "out_of_date",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::NotActionable => "not actionable",
            Self::Disagree => "not adopted",
            Self::Duplicate => "duplicate of another item",
            Self::AlreadyAddressed => "already addressed",
            Self::OutOfDate => "out of date",
        }
    }
}

/// Both the coordinator's advice (`scope_assessment`) and the authoritative
/// Rust result of [`classify_rework_scope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReworkScope {
    WithinTaskScope,
    ScopeChange,
}

impl ReworkScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WithinTaskScope => "within_task_scope",
            Self::ScopeChange => "scope_change",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FollowUpProposal {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
}

impl FollowUpProposal {
    pub fn validate(&self) -> OutputResult<()> {
        if self.title.trim().is_empty() || self.title.len() > MAX_PROPOSAL_TITLE_BYTES {
            return Err(OutputError::invalid(
                "proposal title must contain 1 to 120 bytes",
            ));
        }
        if self.description.len() > MAX_PROPOSAL_DESCRIPTION_BYTES {
            return Err(OutputError::invalid(
                "proposal description is limited to 2048 bytes",
            ));
        }
        if self.acceptance_criteria.len() > MAX_PROPOSAL_CRITERIA
            || self.acceptance_criteria.iter().any(|criterion| {
                criterion.trim().is_empty() || criterion.len() > MAX_PROPOSAL_CRITERION_BYTES
            })
        {
            return Err(OutputError::invalid(
                "proposal acceptance_criteria allows at most 8 entries of 1 to 256 bytes",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalFeedbackEntry {
    pub feedback_item_id: String,
    pub body_digest: String,
    pub disposition: FeedbackDisposition,
    pub rationale: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub scope_assessment: Option<ReworkScope>,
    #[serde(default)]
    pub dismiss_reason: Option<DismissReason>,
    #[serde(default)]
    pub proposal: Option<FollowUpProposal>,
    /// A bounded correction variant for an exact `extend_task` item.
    #[serde(default)]
    pub micro_fix: Option<MicroFixOutput>,
}

impl ExternalFeedbackEntry {
    /// Digest stored in the V173 row; covers the REAL disposition.
    pub fn decision_digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.disposition.as_str().as_bytes());
        hasher.update(b"|");
        hasher.update(self.body_digest.as_bytes());
        hasher.update(b"|");
        hasher.update(self.rationale.as_bytes());
        hasher.update(b"|");
        hasher.update(self.instructions.as_bytes());
        if let Some(micro_fix) = self.micro_fix.as_ref() {
            if let Ok(encoded) = serde_json::to_vec(micro_fix) {
                hasher.update(b"|micro_fix|");
                hasher.update(encoded);
            }
        }
        hex::encode(hasher.finalize())
    }

    fn validate(&self) -> OutputResult<()> {
        if self.rationale.trim().is_empty() || self.rationale.len() > MAX_FEEDBACK_RATIONALE_BYTES {
            return Err(OutputError::invalid(
                "external_feedback rationale must contain 1 to 1024 bytes",
            ));
        }
        if self.instructions.len() > MAX_FEEDBACK_INSTRUCTIONS_BYTES {
            return Err(OutputError::invalid(
                "external_feedback instructions are limited to 4096 bytes",
            ));
        }
        if let Some(micro_fix) = self.micro_fix.as_ref() {
            if self.disposition != FeedbackDisposition::ExtendTask {
                return Err(OutputError::invalid(
                    "micro_fix is only valid for an extend_task disposition",
                ));
            }
            micro_fix.validate()?;
        }
        match (self.disposition, self.dismiss_reason) {
            (FeedbackDisposition::Dismiss, None) => {
                return Err(OutputError::invalid(
                    "a dismiss disposition requires dismiss_reason",
                ));
            }
            (FeedbackDisposition::Dismiss, Some(_)) | (_, None) => {}
            (_, Some(_)) => {
                return Err(OutputError::invalid(
                    "dismiss_reason is only valid for the dismiss disposition",
                ));
            }
        }
        match (self.disposition, self.proposal.as_ref()) {
            (FeedbackDisposition::ProposeTask, Some(proposal)) => proposal.validate()?,
            (FeedbackDisposition::ProposeTask, None) => {
                return Err(OutputError::invalid(
                    "a propose_task disposition requires a proposal",
                ));
            }
            (_, Some(_)) => {
                return Err(OutputError::invalid(
                    "a proposal is only valid for the propose_task disposition",
                ));
            }
            (_, None) => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// work_unit_plan with provenance
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanProvenance {
    pub kind: String,
    pub item_ids: Vec<String>,
}

/// Current plan WorkUnit shape plus the mandatory provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalFeedbackPlanItem {
    pub key: String,
    pub title: String,
    pub scope: String,
    pub role: String,
    #[serde(default)]
    pub assigned_actor: Option<ApiActorRef>,
    #[serde(default)]
    pub parent_key: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub requires_integration: bool,
    #[serde(default)]
    pub execution_instructions: Option<String>,
    #[serde(default)]
    pub review_subject_key: Option<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub artifact_inputs: Vec<String>,
    pub provenance: PlanProvenance,
}

impl ExternalFeedbackPlanItem {
    /// The ordinary plan WorkUnit (provenance travels separately).
    pub fn to_plan_work_unit(&self) -> PlanWorkUnit {
        PlanWorkUnit {
            key: self.key.clone(),
            title: self.title.clone(),
            scope: self.scope.clone(),
            role: self.role.clone(),
            assigned_actor: self.assigned_actor.clone(),
            parent_key: self.parent_key.clone(),
            depends_on: self.depends_on.clone(),
            requires_integration: self.requires_integration,
            execution_instructions: self.execution_instructions.clone(),
            review_subject_key: self.review_subject_key.clone(),
            acceptance: self.acceptance.clone(),
            artifact_inputs: self.artifact_inputs.clone(),
            repository_ids: Vec::new(),
        }
    }

    /// Stored WorkUnit provenance:
    /// `{"kind":"external_feedback","item_ids":[…],"body_digests":[…]}`.
    /// Digests come from the request set, never from the coordinator.
    pub fn stored_provenance(&self, digests: &HashMap<String, String>) -> Value {
        let body_digests = self
            .provenance
            .item_ids
            .iter()
            .filter_map(|id| digests.get(id).cloned())
            .collect::<Vec<_>>();
        serde_json::json!({
            "kind": PROVENANCE_KIND,
            "item_ids": self.provenance.item_ids,
            "body_digests": body_digests,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalFeedbackPlan {
    pub items: Vec<ExternalFeedbackPlanItem>,
}

impl ExternalFeedbackPlan {
    fn validate(&self, extend_ids: &BTreeSet<&str>) -> OutputResult<()> {
        if self.items.is_empty() || self.items.len() > MAX_PLAN_ITEMS {
            return Err(OutputError::invalid(
                "work_unit_plan requires 1 to 64 items",
            ));
        }
        let mut keys = HashSet::with_capacity(self.items.len());
        for item in &self.items {
            if item.key.trim().is_empty()
                || item.key.len() > MAX_PLAN_KEY_BYTES
                || !keys.insert(item.key.as_str())
            {
                return Err(OutputError::invalid(
                    "work_unit_plan keys must be unique and 1 to 128 bytes",
                ));
            }
            if item.title.trim().is_empty()
                || item.title.len() > MAX_PLAN_TITLE_BYTES
                || item.scope.trim().is_empty()
                || item.scope.len() > MAX_PLAN_SCOPE_BYTES
                || item.role.trim().is_empty()
            {
                return Err(OutputError::invalid(
                    "work_unit_plan title, scope and role must be bounded and nonempty",
                ));
            }
            if item
                .execution_instructions
                .as_deref()
                .is_some_and(|value| value.len() > MAX_PLAN_INSTRUCTIONS_BYTES)
                || item.acceptance.len() > MAX_PLAN_ACCEPTANCE
                || item.depends_on.len() > MAX_PLAN_DEPENDENCIES
            {
                return Err(OutputError::invalid(
                    "work_unit_plan item exceeds the instruction, acceptance or dependency limits",
                ));
            }
            if item.provenance.kind != PROVENANCE_KIND || item.provenance.item_ids.is_empty() {
                return Err(OutputError::invalid(
                    "every work_unit_plan item requires external_feedback provenance with item_ids",
                ));
            }
            let mut seen = HashSet::new();
            for id in &item.provenance.item_ids {
                if !extend_ids.contains(id.as_str()) || !seen.insert(id.as_str()) {
                    return Err(OutputError::invalid(
                        "plan provenance item_ids must be distinct extend_task items",
                    ));
                }
            }
        }
        for item in &self.items {
            if item
                .depends_on
                .iter()
                .any(|dependency| dependency == &item.key || !keys.contains(dependency.as_str()))
            {
                return Err(OutputError::invalid(
                    "work_unit_plan dependencies must name other items of the plan",
                ));
            }
        }
        let covered = self
            .items
            .iter()
            .flat_map(|item| item.provenance.item_ids.iter().map(String::as_str))
            .collect::<BTreeSet<_>>();
        if let Some(missing) = extend_ids.iter().find(|id| !covered.contains(**id)) {
            return Err(OutputError::invalid(format!(
                "extend_task item {missing} is not covered by any work_unit_plan item"
            )));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Turn parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalFeedbackTurn {
    schema_version: u32,
    request_id: String,
    task_id: String,
    authorization_version: i64,
    external_feedback: Vec<ExternalFeedbackEntry>,
    #[serde(default)]
    work_unit_plan: Option<ExternalFeedbackPlan>,
    #[serde(default)]
    outputs: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedFeedbackItem {
    pub feedback_item_id: String,
    pub body_digest: String,
}

/// The exact request the turn answers (from the stored set and request row).
#[derive(Debug, Clone)]
pub struct ExpectedFeedbackSet<'a> {
    pub request_id: &'a str,
    pub task_id: &'a str,
    pub authorization_version: i64,
    pub items: &'a [ExpectedFeedbackItem],
}

/// `ask_human` of a `human` item (raised by RT; evidence links the item).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanItemAsk {
    pub feedback_item_id: String,
    pub ask: AskHumanOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedExternalFeedback {
    pub entries: Vec<ExternalFeedbackEntry>,
    pub plan: Option<ExternalFeedbackPlan>,
    pub asks: Vec<HumanItemAsk>,
    pub outputs: Vec<CoordinatorOutput>,
}

impl ParsedExternalFeedback {
    pub fn extend_item_ids(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| entry.disposition == FeedbackDisposition::ExtendTask)
            .map(|entry| entry.feedback_item_id.clone())
            .collect()
    }
}

fn not_permitted(message: impl Into<String>) -> OutputError {
    OutputError::new(CODE_OUTPUT_NOT_PERMITTED, message)
}

/// Strictly parse and validate an `external_feedback` turn against the exact
/// request set. `allowed_actions` gates the `ask_human` outputs only (same
/// asymmetric rule as every new output kind); it is not consulted when the
/// turn carries no output.
pub fn parse_external_feedback_v2(
    value: Value,
    expected: &ExpectedFeedbackSet<'_>,
    allowed_actions: Option<&[CoordinatorOutputKind]>,
) -> OutputResult<ParsedExternalFeedback> {
    if value.to_string().len() > MAX_EXTERNAL_FEEDBACK_TURN_BYTES {
        return Err(OutputError::invalid(
            "external_feedback turn exceeds 65536 bytes",
        ));
    }
    let turn: ExternalFeedbackTurn = serde_json::from_value(value).map_err(|error| {
        OutputError::invalid(format!("external_feedback turn is not valid: {error}"))
    })?;
    if turn.schema_version != EXTERNAL_FEEDBACK_SCHEMA_VERSION {
        return Err(OutputError::invalid(
            "external_feedback requires schema_version 2",
        ));
    }
    if turn.request_id != expected.request_id
        || turn.task_id != expected.task_id
        || turn.authorization_version != expected.authorization_version
    {
        return Err(not_permitted(
            "external_feedback turn does not name the exact request, Task and authorization",
        ));
    }
    if expected.items.is_empty()
        || expected.items.len() > MAX_EXTERNAL_FEEDBACK_ITEMS
        || turn.external_feedback.len() > MAX_EXTERNAL_FEEDBACK_ITEMS
    {
        return Err(OutputError::invalid(
            "an external_feedback set holds 1 to 20 items",
        ));
    }
    let digests = expected
        .items
        .iter()
        .map(|item| (item.feedback_item_id.as_str(), item.body_digest.as_str()))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::with_capacity(turn.external_feedback.len());
    for entry in &turn.external_feedback {
        let Some(digest) = digests.get(entry.feedback_item_id.as_str()) else {
            return Err(not_permitted(
                "external_feedback names an item outside the request set",
            ));
        };
        if !seen.insert(entry.feedback_item_id.as_str()) {
            return Err(not_permitted(
                "external_feedback repeats an item of the request set",
            ));
        }
        if entry.body_digest != *digest {
            return Err(not_permitted(
                "external_feedback body_digest does not match the request set",
            ));
        }
        entry.validate()?;
    }
    if seen.len() != digests.len() {
        return Err(not_permitted(
            "external_feedback must carry exactly one entry per request item",
        ));
    }

    let extend_ids = turn
        .external_feedback
        .iter()
        .filter(|entry| entry.disposition == FeedbackDisposition::ExtendTask)
        .map(|entry| entry.feedback_item_id.as_str())
        .collect::<BTreeSet<_>>();
    match (&turn.work_unit_plan, extend_ids.is_empty()) {
        (Some(_), true) => {
            return Err(OutputError::new(
                CODE_PLAN_WITHOUT_EXTEND,
                "work_unit_plan is only valid when at least one item is extend_task",
            ));
        }
        (None, false) => {
            return Err(OutputError::invalid(
                "extend_task items require a work_unit_plan",
            ));
        }
        (Some(plan), false) => plan.validate(&extend_ids)?,
        (None, true) => {}
    }

    let human_ids = turn
        .external_feedback
        .iter()
        .filter(|entry| entry.disposition == FeedbackDisposition::Human)
        .map(|entry| entry.feedback_item_id.as_str())
        .collect::<BTreeSet<_>>();
    if turn.outputs.len() > MAX_NEW_OUTPUTS_PER_TURN {
        return Err(OutputError::invalid(format!(
            "a coordinator turn may carry at most {MAX_NEW_OUTPUTS_PER_TURN} new outputs"
        )));
    }
    let mut asks = Vec::with_capacity(turn.outputs.len());
    let mut outputs = Vec::with_capacity(turn.outputs.len());
    let mut asked = BTreeSet::new();
    for output in turn.outputs {
        let output = CoordinatorOutput::from_value(output)?;
        ensure_new_kind_permitted(output.kind(), allowed_actions)?;
        if !matches!(
            output,
            CoordinatorOutput::AskHuman(_)
                | CoordinatorOutput::ProposeReplan(_)
                | CoordinatorOutput::RequestRangeReview(_)
        ) {
            return Err(not_permitted(
                "external_feedback supports ask_human, propose_replan and request_range_review outputs only",
            ));
        }
        if let CoordinatorOutput::AskHuman(ask) = &output {
            let item_id = {
                let linked = ask
                    .evidence_links
                    .iter()
                    .filter(|link| link.kind == EvidenceKind::ExternalFeedback)
                    .map(|link| link.id.as_str())
                    .collect::<BTreeSet<_>>();
                let mut linked = linked.into_iter();
                match (linked.next(), linked.next()) {
                    (Some(id), None) => id.to_owned(),
                    _ => {
                        return Err(OutputError::new(
                            super::CODE_EVIDENCE_INVALID,
                            "each ask_human must link exactly one external_feedback item",
                        ));
                    }
                }
            };
            if !human_ids.contains(item_id.as_str()) || !asked.insert(item_id.clone()) {
                return Err(not_permitted(
                    "ask_human must target a distinct item with the human disposition",
                ));
            }
            asks.push(HumanItemAsk {
                feedback_item_id: item_id,
                ask: ask.clone(),
            });
        }
        outputs.push(output);
    }
    if let Some(missing) = human_ids.iter().find(|id| !asked.contains(**id)) {
        return Err(not_permitted(format!(
            "human item {missing} requires an ask_human output"
        )));
    }
    Ok(ParsedExternalFeedback {
        entries: turn.external_feedback,
        plan: turn.work_unit_plan,
        asks,
        outputs,
    })
}

// ---------------------------------------------------------------------------
// classify_rework_scope
// ---------------------------------------------------------------------------

/// Measured facts about the Task's original delivery scope. The caller (RT)
/// measures them; this module does no I/O for them.
#[derive(Debug, Clone)]
pub struct TaskOriginalScope {
    /// `objective_digest` the plan/authorization was made against.
    pub objective_digest: String,
    /// `objective_digest` of the Task now.
    pub current_objective_digest: String,
    /// Envelope as originally authorized (flags may not widen).
    pub original_envelope: CoordinationEnvelope,
    /// Canonical TaskRole names of the current team.
    pub team_roles: BTreeSet<String>,
    /// Current team members as [`actor_key`] strings.
    pub team_actors: BTreeSet<String>,
    /// Non-cancelled WorkUnits already on the Task.
    pub existing_work_units: usize,
    /// `scope` text of every previous WorkUnit (union = original path scope).
    pub original_work_unit_scopes: Vec<String>,
}

/// Canonical actor key shared by the plan and `TaskOriginalScope::team_actors`.
pub fn actor_key(actor: &ApiActorRef) -> String {
    match actor {
        ApiActorRef::Human(id) => format!("human:{id}"),
        ApiActorRef::Agent(id) => format!("agent:{id}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReworkScopeFacts {
    pub objective_digest_unchanged: bool,
    pub roles_within_team: bool,
    pub actors_within_team: bool,
    pub work_units_within_limit: bool,
    pub work_units_total: usize,
    pub max_work_units: usize,
    pub paths_within_original: bool,
    pub no_new_envelope_flags: bool,
    pub new_task_roles_disallowed: bool,
    pub plan_items: usize,
}

impl ReworkScopeFacts {
    fn all_hold(&self) -> bool {
        self.objective_digest_unchanged
            && self.roles_within_team
            && self.actors_within_team
            && self.work_units_within_limit
            && self.paths_within_original
            && self.no_new_envelope_flags
            && self.new_task_roles_disallowed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReworkClassification {
    pub scope: ReworkScope,
    pub facts: ReworkScopeFacts,
}

impl ReworkClassification {
    pub fn facts_json(&self) -> String {
        serde_json::to_string(&self.facts).unwrap_or_else(|_| "{}".to_owned())
    }
}

fn path_tokens(scope: &str) -> Vec<String> {
    scope
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '`' | '"' | '\'' | '(' | ')'))
        .map(|token| token.trim_start_matches("./").trim_end_matches('/'))
        .filter(|token| token.contains('/'))
        .map(str::to_owned)
        .collect()
}

fn path_within(candidate: &str, originals: &BTreeSet<String>) -> bool {
    if candidate.starts_with('/') || candidate.split('/').any(|part| part == "..") {
        return false;
    }
    originals.iter().any(|original| {
        candidate == original
            || candidate
                .strip_prefix(original.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Deterministic scope classifier (D3', 11.1). Any false fact is
/// `scope_change`. `plan` is `None` for a turn without `extend_task` (its facts
/// then cover only the objective and envelope).
pub fn classify_rework_scope(
    plan: Option<&ExternalFeedbackPlan>,
    envelope: &CoordinationEnvelope,
    original: &TaskOriginalScope,
) -> ReworkClassification {
    let items: &[ExternalFeedbackPlanItem] = plan.map(|plan| plan.items.as_slice()).unwrap_or(&[]);
    let roles_within_team = items.iter().all(|item| {
        let name = db::canonical_task_role_name(&item.role)
            .map(|name| name.to_string())
            .unwrap_or_else(|| item.role.clone());
        original.team_roles.contains(&name)
    });
    let actors_within_team = items.iter().all(|item| {
        item.assigned_actor
            .as_ref()
            .is_none_or(|actor| original.team_actors.contains(&actor_key(actor)))
    });
    let work_units_total = original.existing_work_units + items.len();
    let original_paths = original
        .original_work_unit_scopes
        .iter()
        .flat_map(|scope| path_tokens(scope))
        .collect::<BTreeSet<_>>();
    let paths_within_original = items
        .iter()
        .flat_map(|item| path_tokens(&item.scope))
        .all(|path| path_within(&path, &original_paths));
    let before = &original.original_envelope;
    let no_new_envelope_flags = !(envelope.allow_membership_changes
        && !before.allow_membership_changes)
        && !(envelope.allow_capability_changes && !before.allow_capability_changes)
        && !(envelope.allow_repository_write && !before.allow_repository_write)
        && !(envelope.allow_external_actions && !before.allow_external_actions)
        && !(envelope.allow_integration && !before.allow_integration)
        && !(envelope.allow_merge && !before.allow_merge)
        && !(!envelope.require_independent_review && before.require_independent_review)
        && envelope
            .allowed_review_roles
            .iter()
            .all(|role| before.allowed_review_roles.contains(role));
    let facts = ReworkScopeFacts {
        objective_digest_unchanged: original.objective_digest == original.current_objective_digest,
        roles_within_team,
        actors_within_team,
        work_units_within_limit: work_units_total <= envelope.max_work_units,
        work_units_total,
        max_work_units: envelope.max_work_units,
        paths_within_original,
        no_new_envelope_flags,
        new_task_roles_disallowed: !envelope.allow_new_task_roles,
        plan_items: items.len(),
    };
    let scope = if facts.all_hold() {
        ReworkScope::WithinTaskScope
    } else {
        ReworkScope::ScopeChange
    };
    ReworkClassification { scope, facts }
}

/// Reject `extend_task` when Rust classifies the turn as `scope_change`.
pub fn ensure_extend_within_scope(
    parsed: &ParsedExternalFeedback,
    classification: &ReworkClassification,
) -> OutputResult<()> {
    if classification.scope == ReworkScope::ScopeChange
        && parsed
            .entries
            .iter()
            .any(|entry| entry.disposition == FeedbackDisposition::ExtendTask)
    {
        return Err(OutputError::new(
            CODE_EXTEND_OUT_OF_SCOPE,
            "extend_task is outside the original Task scope; emit propose_task or human",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Applying the triage
// ---------------------------------------------------------------------------

pub struct ApplyExternalFeedbackTriage<'a> {
    pub request_id: &'a str,
    pub task_id: &'a str,
    /// Execution of the coordinator turn (V173 triage guard binds it).
    pub execution_id: &'a str,
    pub parsed: &'a ParsedExternalFeedback,
    pub classification: &'a ReworkClassification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedDisposition {
    pub disposition_id: String,
    pub triage_id: String,
    pub feedback_item_id: String,
    pub body_digest: String,
    pub disposition: FeedbackDisposition,
    pub scope_classification: Option<ReworkScope>,
    pub dismiss_reason: Option<DismissReason>,
}

/// `propose_task`: raise this decision (`source_kind = external_feedback`,
/// `source_id = feedback_item_id`), then bind it with
/// [`bind_decision_to_disposition`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposeTaskEffect {
    pub disposition_id: String,
    pub feedback_item_id: String,
    pub proposal: FollowUpProposal,
    pub ask: AskHumanOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanItemEffect {
    pub disposition_id: String,
    pub feedback_item_id: String,
    pub ask: AskHumanOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DismissReceipt {
    pub disposition_id: String,
    pub feedback_item_id: String,
    pub dismiss_reason: DismissReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFeedbackTriageEffects {
    pub request_id: String,
    pub task_id: String,
    pub dispositions: Vec<RecordedDisposition>,
    /// `extend_task` item ids (non-empty ⇒ supersede + plan, in this order).
    pub extend_item_ids: Vec<String>,
    /// Plan to store pending in the `external_feedback_triage` batch origin.
    pub work_unit_plan: Option<ExternalFeedbackPlan>,
    /// `feedback_item_id -> body_digest` for WorkUnit provenance.
    pub body_digests: BTreeMap<String, String>,
    pub proposals: Vec<ProposeTaskEffect>,
    pub human_asks: Vec<HumanItemEffect>,
    pub dismissals: Vec<DismissReceipt>,
    pub classification_facts_json: String,
}

/// The `ask_human` synthesized for an approved-or-not follow-up Task. Only
/// `approve` carries `authority_effect = scope`.
pub fn propose_task_ask(
    proposal: &FollowUpProposal,
    feedback_item_id: &str,
    body_digest: &str,
) -> AskHumanOutput {
    AskHumanOutput {
        question: format!("Create a follow-up Task for PR feedback: {}?", proposal.title),
        options: vec![
            HumanDecisionOption {
                id: PROPOSAL_OPTION_APPROVE.to_owned(),
                label: "Create follow-up Task".to_owned(),
                description: "Add the proposed Task to the Project plan.".to_owned(),
                consequence:
                    "A new Task that depends on the current Task is created; the current PR is unchanged."
                        .to_owned(),
                authority_effect: AuthorityEffect::Scope,
            },
            HumanDecisionOption {
                id: PROPOSAL_OPTION_REJECT.to_owned(),
                label: "Do not create".to_owned(),
                description: "Record the decision and create nothing.".to_owned(),
                consequence: "No Task is created; the current PR is unchanged.".to_owned(),
                authority_effect: AuthorityEffect::None,
            },
        ],
        recommended_option_id: None,
        recommendation_rationale: None,
        evidence_links: vec![EvidenceLink {
            kind: EvidenceKind::ExternalFeedback,
            id: feedback_item_id.to_owned(),
            digest: Some(body_digest.to_owned()),
        }],
    }
}

struct DispositionRow<'a> {
    id: String,
    task_id: &'a str,
    triage_id: &'a str,
    request_id: &'a str,
    entry: &'a ExternalFeedbackEntry,
    scope_classification: Option<ReworkScope>,
    facts_json: Option<String>,
    created_at: &'a str,
}

/// Insert (or replay) the V175 disposition row.
// Swap for `ExternalFeedbackRepo::record_external_feedback_disposition_in_tx`
// once the PX repo lands; the table shape is frozen in «Congelado 11» §11.2.
async fn upsert_disposition_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    row: &DispositionRow<'_>,
) -> Result<String> {
    if let Some(existing) = sqlx::query(
        "SELECT id, disposition, body_digest FROM task_external_feedback_disposition
         WHERE request_id = ? AND feedback_item_id = ?",
    )
    .bind(row.request_id)
    .bind(&row.entry.feedback_item_id)
    .fetch_optional(&mut **tx)
    .await?
    {
        let disposition: String = existing.try_get("disposition")?;
        let digest: String = existing.try_get("body_digest")?;
        if disposition != row.entry.disposition.as_str() || digest != row.entry.body_digest {
            return Err(db::DbError::IdempotencyConflict.into());
        }
        return Ok(existing.try_get("id")?);
    }
    let proposal_json = row
        .entry
        .proposal
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
    sqlx::query(
        "INSERT INTO task_external_feedback_disposition (
            id, task_id, triage_id, request_id, feedback_item_id, body_digest,
            disposition, legacy_disposition, scope_classification,
            classification_facts_json, dismiss_reason, proposal_json, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.id)
    .bind(row.task_id)
    .bind(row.triage_id)
    .bind(row.request_id)
    .bind(&row.entry.feedback_item_id)
    .bind(&row.entry.body_digest)
    .bind(row.entry.disposition.as_str())
    .bind(row.entry.disposition.legacy_str())
    .bind(row.scope_classification.map(ReworkScope::as_str))
    .bind(&row.facts_json)
    .bind(row.entry.dismiss_reason.map(DismissReason::as_str))
    .bind(proposal_json)
    .bind(row.created_at)
    .execute(&mut **tx)
    .await?;
    Ok(row.id.clone())
}

/// Record the whole triage in ONE `BEGIN IMMEDIATE` transaction: a V173 row and
/// a V175 disposition row per item (idempotent on replay), then return the
/// effects. The caller owns the request/Execution state (the V173 guards
/// require a `running|uncertain` request of the same Execution) and the
/// transition of the request to `triaged`.
pub async fn apply_external_feedback_triage(
    db: &SqliteDb,
    input: ApplyExternalFeedbackTriage<'_>,
) -> Result<ExternalFeedbackTriageEffects> {
    ensure_extend_within_scope(input.parsed, input.classification).map_err(ServiceError::from)?;
    let set = ExternalFeedbackRepo::list_external_feedback_set(db, input.request_id).await?;
    let set_digests = set
        .iter()
        .map(|item| (item.id.as_str(), item.body_digest.as_str()))
        .collect::<HashMap<_, _>>();
    if set.len() != input.parsed.entries.len()
        || input.parsed.entries.iter().any(|entry| {
            set_digests.get(entry.feedback_item_id.as_str()) != Some(&entry.body_digest.as_str())
        })
    {
        return Err(not_permitted("the triage does not cover the exact request set").into());
    }

    let now = now_rfc3339();
    let facts_json = input.classification.facts_json();
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    let mut dispositions = Vec::with_capacity(input.parsed.entries.len());
    for entry in &input.parsed.entries {
        let triage = ExternalFeedbackRepo::record_external_feedback_triage_in_tx(
            db,
            &mut tx,
            CreateExternalFeedbackTriage {
                id: db::new_uuid_v4(),
                task_id: input.task_id.to_owned(),
                request_id: input.request_id.to_owned(),
                feedback_item_id: entry.feedback_item_id.clone(),
                body_digest: entry.body_digest.clone(),
                disposition: entry.disposition.legacy(),
                rationale: entry.rationale.clone(),
                instructions: entry.instructions.clone(),
                decision_digest: entry.decision_digest(),
                execution_id: input.execution_id.to_owned(),
                created_at: now.clone(),
            },
        )
        .await?;
        // extend_task is within scope by construction (checked above);
        // propose_task carries the turn classification (CHECK requires one).
        let scope_classification = match entry.disposition {
            FeedbackDisposition::ExtendTask => Some(ReworkScope::WithinTaskScope),
            FeedbackDisposition::ProposeTask => Some(input.classification.scope),
            FeedbackDisposition::Human | FeedbackDisposition::Dismiss => None,
        };
        let disposition_id = upsert_disposition_row(
            &mut tx,
            &DispositionRow {
                id: db::new_uuid_v4(),
                task_id: input.task_id,
                triage_id: &triage.id,
                request_id: input.request_id,
                entry,
                scope_classification,
                facts_json: scope_classification.map(|_| facts_json.clone()),
                created_at: &now,
            },
        )
        .await?;
        dispositions.push(RecordedDisposition {
            disposition_id,
            triage_id: triage.id,
            feedback_item_id: entry.feedback_item_id.clone(),
            body_digest: entry.body_digest.clone(),
            disposition: entry.disposition,
            scope_classification,
            dismiss_reason: entry.dismiss_reason,
        });
    }
    tx.commit().await?;

    let mut effects = ExternalFeedbackTriageEffects {
        request_id: input.request_id.to_owned(),
        task_id: input.task_id.to_owned(),
        dispositions: Vec::new(),
        extend_item_ids: input.parsed.extend_item_ids(),
        work_unit_plan: input.parsed.plan.clone(),
        body_digests: set
            .iter()
            .map(|item| (item.id.clone(), item.body_digest.clone()))
            .collect(),
        proposals: Vec::new(),
        human_asks: Vec::new(),
        dismissals: Vec::new(),
        classification_facts_json: facts_json,
    };
    for (entry, recorded) in input.parsed.entries.iter().zip(&dispositions) {
        match entry.disposition {
            FeedbackDisposition::ExtendTask => {}
            FeedbackDisposition::ProposeTask => {
                if let Some(proposal) = entry.proposal.clone() {
                    let ask =
                        propose_task_ask(&proposal, &entry.feedback_item_id, &entry.body_digest);
                    effects.proposals.push(ProposeTaskEffect {
                        disposition_id: recorded.disposition_id.clone(),
                        feedback_item_id: entry.feedback_item_id.clone(),
                        proposal,
                        ask,
                    });
                }
            }
            FeedbackDisposition::Human => {
                if let Some(item_ask) = input
                    .parsed
                    .asks
                    .iter()
                    .find(|ask| ask.feedback_item_id == entry.feedback_item_id)
                {
                    effects.human_asks.push(HumanItemEffect {
                        disposition_id: recorded.disposition_id.clone(),
                        feedback_item_id: entry.feedback_item_id.clone(),
                        ask: item_ask.ask.clone(),
                    });
                }
            }
            FeedbackDisposition::Dismiss => {
                if let Some(dismiss_reason) = entry.dismiss_reason {
                    effects.dismissals.push(DismissReceipt {
                        disposition_id: recorded.disposition_id.clone(),
                        feedback_item_id: entry.feedback_item_id.clone(),
                        dismiss_reason,
                    });
                }
            }
        }
    }
    effects.dispositions = dispositions;
    Ok(effects)
}

/// Set-once link from a disposition to the human decision RT raised for it.
/// An identical replay is a no-op; a different decision is a conflict.
pub async fn bind_decision_to_disposition(
    db: &SqliteDb,
    disposition_id: &str,
    decision_id: &str,
) -> Result<()> {
    let updated = sqlx::query(
        "UPDATE task_external_feedback_disposition SET human_decision_id = ?
         WHERE id = ? AND human_decision_id IS NULL",
    )
    .bind(decision_id)
    .bind(disposition_id)
    .execute(db.pool())
    .await?
    .rows_affected();
    if updated == 0 {
        let current: Option<String> = sqlx::query_scalar(
            "SELECT human_decision_id FROM task_external_feedback_disposition WHERE id = ?",
        )
        .bind(disposition_id)
        .fetch_optional(db.pool())
        .await?
        .flatten();
        if current.as_deref() != Some(decision_id) {
            return Err(db::DbError::IdempotencyConflict.into());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// PR summary comment text
// ---------------------------------------------------------------------------

/// Deterministic, bounded text P3 posts as the `post_comment` action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrFeedbackSummary {
    /// Full comment, ending with the anti-loop marker.
    pub text: String,
    /// SHA-256 hex of the text WITHOUT the marker (the marker's `digest=`).
    pub digest: String,
}

/// Build the summary of a triage: one line per item with a fixed disposition
/// phrase (and the dismiss reason). Third-party bodies and coordinator free
/// text are never included. At most 16 KiB; overflow is summarized.
pub fn build_pr_feedback_summary(
    entries: &[ExternalFeedbackEntry],
    summary_artifact_id: &str,
) -> PrFeedbackSummary {
    let mut body = String::from("Forge triaged the review feedback on this pull request.\n\n");
    let mut counts = BTreeMap::<&'static str, usize>::new();
    for entry in entries {
        *counts.entry(entry.disposition.as_str()).or_default() += 1;
    }
    let marker_reserve = 256;
    let mut omitted = 0usize;
    for entry in entries {
        let short = entry.feedback_item_id.chars().take(8).collect::<String>();
        let outcome = match entry.disposition {
            FeedbackDisposition::ExtendTask => {
                "accepted; the change will be reworked in this pull request".to_owned()
            }
            FeedbackDisposition::ProposeTask => {
                "outside this Task's scope; a follow-up Task was proposed to the team".to_owned()
            }
            FeedbackDisposition::Human => "a team member was asked to decide".to_owned(),
            FeedbackDisposition::Dismiss => format!(
                "not actioned ({})",
                entry
                    .dismiss_reason
                    .map(DismissReason::label)
                    .unwrap_or("dismissed")
            ),
        };
        let line = format!("- item {short}: {outcome}\n");
        if body.len() + line.len() + marker_reserve > MAX_PR_SUMMARY_BYTES {
            omitted += 1;
        } else {
            body.push_str(&line);
        }
    }
    if omitted > 0 {
        body.push_str(&format!("- {omitted} more item(s) omitted\n"));
    }
    let digest = hex::encode(Sha256::digest(body.as_bytes()));
    let marker = crate::pr_feedback::forge_comment_marker(summary_artifact_id, &digest);
    PrFeedbackSummary {
        text: format!("{body}\n{marker}"),
        digest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const D1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const D2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const D3: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    fn items() -> Vec<ExpectedFeedbackItem> {
        vec![
            ExpectedFeedbackItem {
                feedback_item_id: "i1".into(),
                body_digest: D1.into(),
            },
            ExpectedFeedbackItem {
                feedback_item_id: "i2".into(),
                body_digest: D2.into(),
            },
            ExpectedFeedbackItem {
                feedback_item_id: "i3".into(),
                body_digest: D3.into(),
            },
        ]
    }

    fn expected(items: &[ExpectedFeedbackItem]) -> ExpectedFeedbackSet<'_> {
        ExpectedFeedbackSet {
            request_id: "r1",
            task_id: "t1",
            authorization_version: 3,
            items,
        }
    }

    fn entry(id: &str, digest: &str, disposition: &str) -> Value {
        json!({"feedback_item_id": id, "body_digest": digest, "disposition": disposition,
               "rationale": "because", "instructions": "", "scope_assessment": null,
               "dismiss_reason": null, "proposal": null, "micro_fix": null})
    }

    fn plan_item(key: &str, ids: &[&str]) -> Value {
        json!({"key": key, "title": "Fix", "scope": "crates/a/src", "role": "implementer",
               "provenance": {"kind": "external_feedback", "item_ids": ids}})
    }

    fn dismiss(id: &str, digest: &str) -> Value {
        let mut value = entry(id, digest, "dismiss");
        value["dismiss_reason"] = json!("disagree");
        value
    }

    fn propose(id: &str, digest: &str) -> Value {
        let mut value = entry(id, digest, "propose_task");
        value["proposal"] = json!({"title": "Support X", "description": "d",
                                   "acceptance_criteria": ["works"]});
        value
    }

    fn turn(entries: Vec<Value>, plan: Value, outputs: Vec<Value>) -> Value {
        json!({"schema_version": 2, "request_id": "r1", "task_id": "t1",
               "authorization_version": 3, "external_feedback": entries,
               "work_unit_plan": plan, "outputs": outputs})
    }

    fn ask_for(id: &str) -> Value {
        json!({"type": "ask_human", "question": "Which?",
               "options": [
                 {"id": "a", "label": "A", "description": "", "consequence": "", "authority_effect": "none"},
                 {"id": "b", "label": "B", "description": "", "consequence": "", "authority_effect": "none"}],
               "evidence_links": [{"kind": "external_feedback", "id": id}]})
    }

    fn all_allowed() -> Vec<CoordinatorOutputKind> {
        vec![CoordinatorOutputKind::AskHuman]
    }

    fn mixed_turn() -> Value {
        turn(
            vec![
                entry("i1", D1, "extend_task"),
                propose("i2", D2),
                dismiss("i3", D3),
            ],
            json!({"items": [plan_item("w1", &["i1"])]}),
            vec![],
        )
    }

    #[test]
    fn coordinator_outputs_external_feedback_parses_one_entry_per_item() {
        let items = items();
        let parsed = parse_external_feedback_v2(mixed_turn(), &expected(&items), None)
            .expect("valid turn parses");
        assert_eq!(parsed.entries.len(), 3);
        assert_eq!(parsed.extend_item_ids(), vec!["i1".to_owned()]);
        assert!(parsed.plan.is_some());
        assert_eq!(
            FeedbackDisposition::ProposeTask.legacy(),
            ExternalFeedbackDisposition::Human
        );
        assert_eq!(
            FeedbackDisposition::ExtendTask.legacy(),
            ExternalFeedbackDisposition::Correct
        );
    }

    #[test]
    fn coordinator_outputs_external_feedback_rejects_unknown_repeated_and_missing_items() {
        let items = items();
        let missing = turn(
            vec![dismiss("i1", D1), dismiss("i2", D2)],
            Value::Null,
            vec![],
        );
        let repeated = turn(
            vec![dismiss("i1", D1), dismiss("i1", D1), dismiss("i2", D2)],
            Value::Null,
            vec![],
        );
        let unknown = turn(
            vec![dismiss("i1", D1), dismiss("i2", D2), dismiss("zz", D3)],
            Value::Null,
            vec![],
        );
        let wrong_digest = turn(
            vec![dismiss("i1", D2), dismiss("i2", D2), dismiss("i3", D3)],
            Value::Null,
            vec![],
        );
        for value in [missing, repeated, unknown, wrong_digest] {
            let error = parse_external_feedback_v2(value, &expected(&items), None)
                .expect_err("set mismatch is rejected");
            assert_eq!(error.code, CODE_OUTPUT_NOT_PERMITTED);
        }
    }

    #[test]
    fn coordinator_outputs_external_feedback_rejects_unknown_fields_and_wrong_schema() {
        let items = items();
        let mut value = mixed_turn();
        value["command"] = json!("rm -rf");
        assert!(parse_external_feedback_v2(value, &expected(&items), None).is_err());
        let mut value = mixed_turn();
        value["schema_version"] = json!(1);
        assert!(parse_external_feedback_v2(value, &expected(&items), None).is_err());
        let mut value = mixed_turn();
        value["authorization_version"] = json!(4);
        assert_eq!(
            parse_external_feedback_v2(value, &expected(&items), None)
                .expect_err("stale authorization")
                .code,
            CODE_OUTPUT_NOT_PERMITTED
        );
    }

    #[test]
    fn coordinator_outputs_external_feedback_plan_iff_extend() {
        let items = items();
        let no_plan = turn(
            vec![
                entry("i1", D1, "extend_task"),
                dismiss("i2", D2),
                dismiss("i3", D3),
            ],
            Value::Null,
            vec![],
        );
        assert!(parse_external_feedback_v2(no_plan, &expected(&items), None).is_err());
        let stray_plan = turn(
            vec![dismiss("i1", D1), dismiss("i2", D2), dismiss("i3", D3)],
            json!({"items": [plan_item("w1", &["i1"])]}),
            vec![],
        );
        assert_eq!(
            parse_external_feedback_v2(stray_plan, &expected(&items), None)
                .expect_err("plan without extend")
                .code,
            CODE_PLAN_WITHOUT_EXTEND
        );
    }

    #[test]
    fn coordinator_outputs_external_feedback_plan_provenance_must_cover_extend_items() {
        let items = items();
        let two_extend = |plan: Value| {
            turn(
                vec![
                    entry("i1", D1, "extend_task"),
                    entry("i2", D2, "extend_task"),
                    dismiss("i3", D3),
                ],
                plan,
                vec![],
            )
        };
        let uncovered = two_extend(json!({"items": [plan_item("w1", &["i1"])]}));
        assert!(parse_external_feedback_v2(uncovered, &expected(&items), None).is_err());
        let foreign = two_extend(json!({"items": [plan_item("w1", &["i1", "i3"])]}));
        assert!(parse_external_feedback_v2(foreign, &expected(&items), None).is_err());
        let covered =
            two_extend(json!({"items": [plan_item("w1", &["i1"]), plan_item("w2", &["i2"])]}));
        assert!(parse_external_feedback_v2(covered, &expected(&items), None).is_ok());
        let duplicate_key =
            two_extend(json!({"items": [plan_item("w1", &["i1"]), plan_item("w1", &["i2"])]}));
        assert!(parse_external_feedback_v2(duplicate_key, &expected(&items), None).is_err());
    }

    #[test]
    fn coordinator_outputs_external_feedback_disposition_field_rules() {
        let items = items();
        let base = |second: Value| {
            turn(
                vec![dismiss("i1", D1), second, dismiss("i3", D3)],
                Value::Null,
                vec![],
            )
        };
        let mut no_reason = dismiss("i2", D2);
        no_reason["dismiss_reason"] = Value::Null;
        assert!(parse_external_feedback_v2(base(no_reason), &expected(&items), None).is_err());
        let mut stray_reason = entry("i2", D2, "human");
        stray_reason["dismiss_reason"] = json!("duplicate");
        assert!(parse_external_feedback_v2(base(stray_reason), &expected(&items), None).is_err());
        let no_proposal = entry("i2", D2, "propose_task");
        assert!(parse_external_feedback_v2(base(no_proposal), &expected(&items), None).is_err());
        let mut stray_proposal = dismiss("i2", D2);
        stray_proposal["proposal"] = json!({"title": "t", "description": "d"});
        assert!(parse_external_feedback_v2(base(stray_proposal), &expected(&items), None).is_err());
        let mut micro = dismiss("i2", D2);
        micro["micro_fix"] = json!({"anything": 1});
        assert!(parse_external_feedback_v2(base(micro), &expected(&items), None).is_err());
        let mut long_title = propose("i2", D2);
        long_title["proposal"]["title"] = json!("x".repeat(121));
        assert!(parse_external_feedback_v2(base(long_title), &expected(&items), None).is_err());
        let mut long_rationale = dismiss("i2", D2);
        long_rationale["rationale"] = json!("x".repeat(1025));
        assert!(parse_external_feedback_v2(base(long_rationale), &expected(&items), None).is_err());
    }

    #[test]
    fn coordinator_outputs_external_feedback_human_items_require_ask_human() {
        let items = items();
        let entries = || {
            vec![
                entry("i1", D1, "human"),
                dismiss("i2", D2),
                dismiss("i3", D3),
            ]
        };
        let without = turn(entries(), Value::Null, vec![]);
        assert!(parse_external_feedback_v2(
            without,
            &expected(&items),
            Some(all_allowed().as_slice())
        )
        .is_err());
        let with = turn(entries(), Value::Null, vec![ask_for("i1")]);
        let parsed = parse_external_feedback_v2(
            with.clone(),
            &expected(&items),
            Some(all_allowed().as_slice()),
        )
        .expect("human item with its ask parses");
        assert_eq!(parsed.asks.len(), 1);
        assert_eq!(parsed.asks[0].feedback_item_id, "i1");
        // Policy: absent allowed_actions denies the new kind.
        assert_eq!(
            parse_external_feedback_v2(with, &expected(&items), None)
                .expect_err("asymmetric policy")
                .code,
            CODE_OUTPUT_NOT_PERMITTED
        );
        let wrong_target = turn(entries(), Value::Null, vec![ask_for("i2")]);
        assert!(parse_external_feedback_v2(
            wrong_target,
            &expected(&items),
            Some(all_allowed().as_slice())
        )
        .is_err());
    }

    fn original() -> TaskOriginalScope {
        TaskOriginalScope {
            objective_digest: "o".into(),
            current_objective_digest: "o".into(),
            original_envelope: CoordinationEnvelope::default(),
            team_roles: ["implementer".to_owned(), "reviewer".to_owned()].into(),
            team_actors: ["agent:a1".to_owned()].into(),
            existing_work_units: 2,
            original_work_unit_scopes: vec!["Edit crates/a/src and crates/a/tests".into()],
        }
    }

    fn plan_with(scope: &str, role: &str, actor: Option<ApiActorRef>) -> ExternalFeedbackPlan {
        ExternalFeedbackPlan {
            items: vec![ExternalFeedbackPlanItem {
                key: "w1".into(),
                title: "t".into(),
                scope: scope.into(),
                role: role.into(),
                assigned_actor: actor,
                parent_key: None,
                depends_on: vec![],
                requires_integration: false,
                execution_instructions: None,
                review_subject_key: None,
                acceptance: vec![],
                artifact_inputs: vec![],
                provenance: PlanProvenance {
                    kind: PROVENANCE_KIND.into(),
                    item_ids: vec!["i1".into()],
                },
            }],
        }
    }

    #[test]
    fn coordinator_outputs_external_feedback_classifier_within_scope() {
        let plan = plan_with(
            "crates/a/src/lib.rs",
            "implementer",
            Some(ApiActorRef::Agent("a1".into())),
        );
        let result =
            classify_rework_scope(Some(&plan), &CoordinationEnvelope::default(), &original());
        assert_eq!(result.scope, ReworkScope::WithinTaskScope);
        assert!(result.facts.paths_within_original);
        assert_eq!(result.facts.work_units_total, 3);
        assert!(result.facts_json().contains("\"roles_within_team\":true"));
    }

    #[test]
    fn coordinator_outputs_external_feedback_classifier_flags_each_measurable_fact() {
        let envelope = CoordinationEnvelope::default();
        let within = plan_with("crates/a/src/lib.rs", "implementer", None);

        let mut changed = original();
        changed.current_objective_digest = "other".into();
        assert_eq!(
            classify_rework_scope(Some(&within), &envelope, &changed).scope,
            ReworkScope::ScopeChange
        );

        let new_path = plan_with("crates/b/src/lib.rs", "implementer", None);
        assert_eq!(
            classify_rework_scope(Some(&new_path), &envelope, &original()).scope,
            ReworkScope::ScopeChange
        );
        let traversal = plan_with("crates/a/src/../../b/x.rs", "implementer", None);
        assert_eq!(
            classify_rework_scope(Some(&traversal), &envelope, &original()).scope,
            ReworkScope::ScopeChange
        );

        let new_role = plan_with("crates/a/src/lib.rs", "tester", None);
        assert!(
            !classify_rework_scope(Some(&new_role), &envelope, &original())
                .facts
                .roles_within_team
        );

        let new_actor = plan_with(
            "crates/a/src/lib.rs",
            "implementer",
            Some(ApiActorRef::Agent("stranger".into())),
        );
        assert!(
            !classify_rework_scope(Some(&new_actor), &envelope, &original())
                .facts
                .actors_within_team
        );

        let mut tight = envelope.clone();
        tight.max_work_units = 2;
        assert!(
            !classify_rework_scope(Some(&within), &tight, &original())
                .facts
                .work_units_within_limit
        );

        let mut widened = envelope.clone();
        let mut before = original();
        before.original_envelope.allow_merge = false;
        widened.allow_merge = true;
        assert!(
            !classify_rework_scope(Some(&within), &widened, &before)
                .facts
                .no_new_envelope_flags
        );

        let mut roles = envelope;
        roles.allow_new_task_roles = true;
        assert_eq!(
            classify_rework_scope(Some(&within), &roles, &original()).scope,
            ReworkScope::ScopeChange
        );
    }

    #[test]
    fn coordinator_outputs_external_feedback_extend_rejected_on_scope_change() {
        let items = items();
        let parsed = parse_external_feedback_v2(mixed_turn(), &expected(&items), None)
            .expect("valid turn parses");
        let outside = plan_with("elsewhere/src/x.rs", "implementer", None);
        let classification = classify_rework_scope(
            Some(&outside),
            &CoordinationEnvelope::default(),
            &original(),
        );
        assert_eq!(classification.scope, ReworkScope::ScopeChange);
        let error = ensure_extend_within_scope(&parsed, &classification)
            .expect_err("extend_task is rejected");
        assert_eq!(error.code, CODE_EXTEND_OUT_OF_SCOPE);

        let inside = plan_with("crates/a/src/x.rs", "implementer", None);
        let ok =
            classify_rework_scope(Some(&inside), &CoordinationEnvelope::default(), &original());
        assert!(ensure_extend_within_scope(&parsed, &ok).is_ok());

        // A turn without extend_task is never rejected by the classifier.
        let no_extend = ParsedExternalFeedback {
            entries: vec![],
            plan: None,
            asks: vec![],
            outputs: vec![],
        };
        assert!(ensure_extend_within_scope(&no_extend, &classification).is_ok());
    }

    #[test]
    fn coordinator_outputs_external_feedback_propose_ask_scope_only_on_approve() {
        let proposal = FollowUpProposal {
            title: "Support X".into(),
            description: "d".into(),
            acceptance_criteria: vec![],
        };
        let ask = propose_task_ask(&proposal, "i2", D2);
        ask.validate().expect("synthesized ask is valid");
        assert_eq!(ask.max_authority_effect(), AuthorityEffect::Scope);
        let approve = ask
            .options
            .iter()
            .find(|option| option.id == PROPOSAL_OPTION_APPROVE)
            .expect("approve option");
        assert_eq!(approve.authority_effect, AuthorityEffect::Scope);
        let reject = ask
            .options
            .iter()
            .find(|option| option.id == PROPOSAL_OPTION_REJECT)
            .expect("reject option");
        assert_eq!(reject.authority_effect, AuthorityEffect::None);
    }

    #[test]
    fn coordinator_outputs_external_feedback_decision_digest_covers_disposition() {
        let items = items();
        let parsed = parse_external_feedback_v2(mixed_turn(), &expected(&items), None)
            .expect("valid turn parses");
        let mut changed = parsed.entries[0].clone();
        let before = changed.decision_digest();
        changed.disposition = FeedbackDisposition::Human;
        assert_ne!(before, changed.decision_digest());
        assert_eq!(before.len(), 64);
    }

    #[test]
    fn coordinator_outputs_external_feedback_summary_is_bounded_marked_and_body_free() {
        let items = items();
        let parsed = parse_external_feedback_v2(mixed_turn(), &expected(&items), None)
            .expect("valid turn parses");
        let summary = build_pr_feedback_summary(&parsed.entries, "artifact-1");
        assert!(crate::pr_feedback::is_forge_authored(&summary.text));
        assert!(summary
            .text
            .contains(&crate::pr_feedback::forge_comment_marker(
                "artifact-1",
                &summary.digest
            )));
        assert!(summary.text.len() <= MAX_PR_SUMMARY_BYTES);
        assert!(!summary.text.contains("because"));
        assert_eq!(
            summary,
            build_pr_feedback_summary(&parsed.entries, "artifact-1")
        );

        let many = (0..2000)
            .map(|index| {
                let mut copy = parsed.entries[2].clone();
                copy.feedback_item_id = format!("item-{index:05}");
                copy
            })
            .collect::<Vec<_>>();
        let bounded = build_pr_feedback_summary(&many, "artifact-1");
        assert!(bounded.text.len() <= MAX_PR_SUMMARY_BYTES);
        assert!(bounded.text.contains("more item(s) omitted"));
    }

    #[test]
    fn coordinator_outputs_external_feedback_stored_provenance_uses_set_digests() {
        let plan = plan_with("crates/a/src", "implementer", None);
        let digests = HashMap::from([("i1".to_owned(), D1.to_owned())]);
        let stored = plan.items[0].stored_provenance(&digests);
        assert_eq!(stored["kind"], "external_feedback");
        assert_eq!(stored["item_ids"], json!(["i1"]));
        assert_eq!(stored["body_digests"], json!([D1]));
    }
}
