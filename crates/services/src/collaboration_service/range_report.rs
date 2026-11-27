//! Commit-range ReviewReport contract (`docs/plans/review-ranges.md`, C0.7):
//! the `FORGE_RESULT` v2 parser, its pure all-or-nothing validation, and the
//! ingestion that writes the Artifact, the finding ledger and the checkpoint in
//! one transaction. An invalid report writes nothing.

use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use db::{
    new_uuid_v4, now_rfc3339, ActorRef, Artifact, ArtifactKind, ArtifactStorageKind,
    CreateArtifact, CreateDomainEvent, IngestRangeReport, ReviewCheckpoint, ReviewCheckpointRepo,
    ReviewCheckpointState, ReviewFinding, ReviewFindingLocation, ReviewFindingResolution,
    ReviewFindingSeverity, ReviewLedgerRepo, ReviewPolicyRepo, ReviewRange, ReviewRangeReport,
    ReviewRangeReportRepo, ReviewResolutionOutcome, ReviewVerdict, WorkUnitReviewRange,
    WorkspaceRepo,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{CollaborationActorSource, CollaborationService, EventScope, ReviewReportInputs};
use crate::{
    review_range::{canonical_digest, FindingLocation, ReviewAttribution},
    work_unit_service::range_review::{self, RangeCheckout},
    Result, ServiceError,
};

const MAX_FINDINGS: usize = 100;
const MAX_LOCATIONS: usize = 16;
const MAX_RESOLUTIONS: usize = 500;
const MAX_QUESTIONS: usize = 32;

/// One report, from an Agent `FORGE_RESULT` v2 line or a Human submission.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RangeReportInput {
    pub schema_version: u32,
    pub kind: String,
    pub verdict: String,
    pub summary: String,
    #[serde(default)]
    pub criteria: Vec<Value>,
    pub questions: Vec<String>,
    #[serde(default)]
    pub evidence_considered: Vec<Value>,
    pub findings: Vec<api_types::StructuredReviewFinding>,
    pub resolutions: Vec<api_types::ReviewResolutionInput>,
}

pub(crate) struct RangeReportContext {
    pub range: ReviewRange,
    pub input_findings: Vec<ReviewFinding>,
    pub recheck: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ValidatedLocation {
    pub path: String,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedFinding {
    pub label: String,
    pub severity: ReviewFindingSeverity,
    pub title: String,
    pub evidence: String,
    pub invariant: Option<String>,
    pub locations: Vec<ValidatedLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedResolution {
    pub finding_id: String,
    pub claim: api_types::ReviewResolutionClaim,
    pub evidence: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedRangeReport {
    pub verdict: ReviewVerdict,
    pub summary: String,
    pub criteria: Vec<Value>,
    pub questions: Vec<String>,
    pub findings: Vec<ValidatedFinding>,
    pub resolutions: Vec<ValidatedResolution>,
}

fn invalid_findings(message: impl Into<String>) -> ServiceError {
    ServiceError::Coordination {
        code: "review.report.invalid_findings",
        message: message.into(),
    }
}

/// Parse the single final `FORGE_RESULT` line of a range review. Version 1 is
/// never accepted for a range.
pub(crate) fn parse_range_review_result(output: &str) -> Result<RangeReportInput> {
    let markers = output
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            line.trim()
                .strip_prefix("FORGE_RESULT:")
                .map(|payload| (index, payload.trim()))
        })
        .collect::<Vec<_>>();
    let last_nonempty_line = output
        .lines()
        .enumerate()
        .filter_map(|(index, line)| (!line.trim().is_empty()).then_some(index))
        .last();
    let [(index, payload)] = markers.as_slice() else {
        return Err(invalid_findings(
            "review assistant output must end with exactly one FORGE_RESULT line",
        ));
    };
    if Some(*index) != last_nonempty_line {
        return Err(invalid_findings(
            "review assistant output must end with exactly one FORGE_RESULT line",
        ));
    }
    let input: RangeReportInput = serde_json::from_str(payload).map_err(|error| {
        invalid_findings(format!(
            "FORGE_RESULT is not a valid version 2 range review: {error}"
        ))
    })?;
    if input.schema_version != 2 || input.kind != "review" {
        return Err(invalid_findings(
            "a commit-range review requires FORGE_RESULT schema_version 2",
        ));
    }
    Ok(input)
}

fn bounded(value: &str, max: usize, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > max {
        return Err(invalid_findings(format!(
            "{field} must contain between 1 and {max} bytes"
        )));
    }
    Ok(())
}

fn validate_location(location: &api_types::StructuredReviewLocation) -> Result<ValidatedLocation> {
    let path = location.path.as_str();
    if path.is_empty()
        || path.len() > 1024
        || path.contains(['\0', '\\', '\n'])
        || path
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err(invalid_findings(
            "finding locations require a safe repository-relative path of at most 1024 bytes",
        ));
    }
    let line_end = match (location.line_start, location.line_end) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(invalid_findings("line_end requires line_start"));
        }
        (Some(start), end) => {
            let end = end.unwrap_or(start);
            if start < 1 || end < start || end > i64::from(u32::MAX) {
                return Err(invalid_findings("finding location lines are invalid"));
            }
            Some(end)
        }
    };
    Ok(ValidatedLocation {
        path: path.to_owned(),
        line_start: location.line_start,
        line_end,
    })
}

fn validate_finding(finding: &api_types::StructuredReviewFinding) -> Result<ValidatedFinding> {
    if finding.label.is_empty()
        || finding.label.len() > 32
        || !finding
            .label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(invalid_findings(
            "finding labels must match ^[A-Za-z0-9._-]{1,32}$",
        ));
    }
    bounded(&finding.title, 256, "finding title")?;
    bounded(&finding.evidence, 4096, "finding evidence")?;
    let invariant = finding
        .invariant
        .as_deref()
        .filter(|invariant| !invariant.trim().is_empty());
    if invariant.is_some_and(|invariant| invariant.len() > 512) {
        return Err(invalid_findings(
            "finding invariant must be at most 512 bytes",
        ));
    }
    if finding.locations.len() > MAX_LOCATIONS {
        return Err(invalid_findings("a finding has at most 16 locations"));
    }
    Ok(ValidatedFinding {
        label: finding.label.clone(),
        severity: match finding.severity {
            api_types::ReviewFindingSeverity::P0 => ReviewFindingSeverity::P0,
            api_types::ReviewFindingSeverity::P1 => ReviewFindingSeverity::P1,
            api_types::ReviewFindingSeverity::P2 => ReviewFindingSeverity::P2,
            api_types::ReviewFindingSeverity::P3 => ReviewFindingSeverity::P3,
        },
        title: finding.title.clone(),
        evidence: finding.evidence.clone(),
        invariant: invariant.map(str::to_owned),
        locations: finding
            .locations
            .iter()
            .map(validate_location)
            .collect::<Result<_>>()?,
    })
}

/// Everything of C0.7 that needs no repository: paths and ancestry are checked
/// by the ingestion against the reviewer checkout.
pub(crate) fn validate_range_report(
    input: &RangeReportInput,
    ctx: &RangeReportContext,
) -> Result<ValidatedRangeReport> {
    use api_types::ReviewResolutionClaim as Claim;

    let verdict = match input.verdict.as_str() {
        "pass" => ReviewVerdict::Pass,
        "fail" => ReviewVerdict::RequestChanges,
        "needs_human" => ReviewVerdict::Questions,
        _ => return Err(invalid_findings("FORGE_RESULT review verdict is invalid")),
    };
    let summary = input.summary.trim();
    bounded(summary, 8192, "summary")?;
    if input.questions.len() > MAX_QUESTIONS {
        return Err(invalid_findings("a report asks at most 32 questions"));
    }
    for question in &input.questions {
        bounded(question, 2048, "question")?;
    }
    if verdict == ReviewVerdict::Questions {
        if input.questions.is_empty() || !input.findings.is_empty() || !input.resolutions.is_empty()
        {
            return Err(invalid_findings(
                "needs_human requires questions and no findings or resolutions",
            ));
        }
    } else if !input.questions.is_empty() {
        return Err(invalid_findings("only needs_human may carry questions"));
    }
    if input.findings.len() > MAX_FINDINGS {
        return Err(invalid_findings("a report has at most 100 findings"));
    }
    let findings = input
        .findings
        .iter()
        .map(validate_finding)
        .collect::<Result<Vec<_>>>()?;
    let mut labels = HashSet::new();
    if !findings
        .iter()
        .all(|finding| labels.insert(finding.label.as_str()))
    {
        return Err(invalid_findings("finding labels must be unique"));
    }

    if input.resolutions.len() > MAX_RESOLUTIONS {
        return Err(invalid_findings("a report has at most 500 resolutions"));
    }
    let frozen = ctx
        .input_findings
        .iter()
        .map(|finding| (finding.id.as_str(), finding))
        .collect::<BTreeMap<_, _>>();
    let mut claimed = BTreeMap::new();
    let mut resolutions = Vec::with_capacity(input.resolutions.len());
    for resolution in &input.resolutions {
        bounded(&resolution.finding_id, 64, "resolution finding_id")?;
        bounded(&resolution.evidence, 4096, "resolution evidence")?;
        let Some(finding) = frozen.get(resolution.finding_id.as_str()) else {
            return Err(invalid_findings(
                "resolutions may only name the frozen input findings",
            ));
        };
        if claimed
            .insert(resolution.finding_id.as_str(), resolution.outcome)
            .is_some()
        {
            return Err(invalid_findings("a finding is resolved at most once"));
        }
        if resolution.outcome == Claim::NotReproduced && !ctx.recheck {
            return Err(invalid_findings(
                "not_reproduced is only valid for a recheck",
            ));
        }
        // Strict descent from the anchor is verified against Git at ingestion.
        if resolution.outcome == Claim::Fixed && finding.anchor_commit == ctx.range.head_commit {
            return Err(invalid_findings(
                "fixed requires a head that strictly descends from the finding anchor",
            ));
        }
        resolutions.push(ValidatedResolution {
            finding_id: resolution.finding_id.clone(),
            claim: resolution.outcome,
            evidence: resolution.evidence.clone(),
        });
    }
    if verdict != ReviewVerdict::Questions
        && ctx.input_findings.iter().any(|finding| {
            finding.severity.is_blocking() && !claimed.contains_key(finding.id.as_str())
        })
    {
        return Err(invalid_findings(
            "every blocking input finding requires a resolution claim",
        ));
    }
    let still_open = claimed.values().any(|claim| *claim == Claim::StillOpen);
    if verdict == ReviewVerdict::Pass
        && (findings
            .iter()
            .any(|finding| finding.severity.is_blocking())
            || ctx.input_findings.iter().any(|finding| {
                finding.severity.is_blocking()
                    && claimed.get(finding.id.as_str()) == Some(&Claim::StillOpen)
            }))
    {
        return Err(invalid_findings(
            "pass admits no new or still open P0/P1 finding",
        ));
    }
    if verdict == ReviewVerdict::RequestChanges && findings.is_empty() && !still_open {
        return Err(invalid_findings(
            "fail requires a new finding or a still open input finding",
        ));
    }
    Ok(ValidatedRangeReport {
        verdict,
        summary: summary.to_owned(),
        criteria: input.criteria.clone(),
        questions: input.questions.clone(),
        findings,
        resolutions,
    })
}

/// Stable identifier, so that a retried report reproduces the same ledger IDs.
fn stable_id(domain: &str, parts: [&str; 2]) -> String {
    let mut hasher = Sha256::new();
    for component in [domain, parts[0], parts[1]] {
        hasher.update((component.len() as u64).to_be_bytes());
        hasher.update(component.as_bytes());
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).to_string()
}

fn finding_id(execution_id: &str, label: &str) -> String {
    stable_id("forge.review-finding.v1", [execution_id, label])
}

#[derive(Serialize)]
struct FindingDigest<'a> {
    id: &'a str,
    report_artifact_id: &'a str,
    severity: ReviewFindingSeverity,
    title: &'a str,
    evidence: &'a str,
    invariant: Option<&'a str>,
    anchor_commit: &'a str,
    locations: &'a [ValidatedLocation],
}

fn findings_json(execution_id: &str, findings: &[ValidatedFinding]) -> Value {
    Value::Array(
        findings
            .iter()
            .map(|finding| {
                serde_json::json!({
                    "finding_id": finding_id(execution_id, &finding.label),
                    "label": finding.label,
                    "severity": finding.severity.to_string(),
                    "title": finding.title,
                    "evidence": finding.evidence,
                    "invariant": finding.invariant,
                    "locations": finding.locations,
                })
            })
            .collect(),
    )
}

fn resolutions_json(resolutions: &[ValidatedResolution]) -> Value {
    Value::Array(
        resolutions
            .iter()
            .map(|resolution| {
                serde_json::json!({
                    "finding_id": resolution.finding_id,
                    "outcome": resolution.claim,
                    "evidence": resolution.evidence,
                })
            })
            .collect(),
    )
}

/// The report body without its subject; a replay must reproduce it exactly.
fn report_body(
    execution_id: &str,
    report: &ValidatedRangeReport,
    evidence_considered: &[Value],
) -> serde_json::Map<String, Value> {
    let mut body = serde_json::Map::new();
    body.insert("kind".to_owned(), Value::from("review_report"));
    body.insert(
        "verdict".to_owned(),
        Value::from(report.verdict.to_string()),
    );
    body.insert("summary".to_owned(), Value::from(report.summary.clone()));
    body.insert("criteria".to_owned(), Value::from(report.criteria.clone()));
    body.insert("questions".to_owned(), serde_json::json!(report.questions));
    body.insert(
        "evidence_considered".to_owned(),
        Value::from(evidence_considered.to_vec()),
    );
    body.insert(
        "findings".to_owned(),
        findings_json(execution_id, &report.findings),
    );
    body.insert(
        "resolutions".to_owned(),
        resolutions_json(&report.resolutions),
    );
    body
}

fn range_subject_matches(value: &Value, frozen: &WorkUnitReviewRange, range: &ReviewRange) -> bool {
    let text = |pointer: &str| value.pointer(pointer).and_then(Value::as_str);
    text("/subject/kind") == Some("commit_range")
        && text("/subject/checkpoint_id") == Some(frozen.checkpoint_id.as_str())
        && text("/subject/range/id") == Some(range.id.as_str())
        && text("/subject/range/range_digest") == Some(range.range_digest.as_str())
        && text("/subject/input_findings_digest") == Some(frozen.input_findings_digest.as_str())
}

impl CollaborationService {
    async fn range_report_context(
        &self,
        frozen: &WorkUnitReviewRange,
    ) -> Result<(RangeReportContext, ReviewCheckpoint)> {
        let range = ReviewCheckpointRepo::get_review_range(&*self.db, &frozen.range_id)
            .await?
            .filter(|range| range.task_id == frozen.task_id && range.repo_id == frozen.repo_id)
            .ok_or_else(|| range_review::mismatch("review range is unavailable"))?;
        let checkpoint =
            ReviewCheckpointRepo::get_review_checkpoint(&*self.db, &frozen.checkpoint_id)
                .await?
                .filter(|checkpoint| {
                    checkpoint.task_id == frozen.task_id
                        && checkpoint.range_id.as_deref() == Some(range.id.as_str())
                })
                .ok_or_else(|| range_review::mismatch("review checkpoint is unavailable"))?;
        let mut input_findings = Vec::with_capacity(frozen.input_findings.len());
        for input in &frozen.input_findings {
            let finding = ReviewLedgerRepo::get_review_finding(&*self.db, &input.finding_id)
                .await?
                .filter(|finding| {
                    finding.task_id == frozen.task_id
                        && finding.repo_id == frozen.repo_id
                        && finding.digest == input.digest
                })
                .ok_or_else(|| {
                    range_review::mismatch("a frozen input finding no longer matches the ledger")
                })?;
            input_findings.push(finding);
        }
        let recheck = checkpoint
            .reasons
            .contains(&db::ReviewCheckpointReason::Recheck);
        Ok((
            RangeReportContext {
                range,
                input_findings,
                recheck,
            },
            checkpoint,
        ))
    }

    /// Reconciliation of a persisted range report: it must name the frozen range
    /// of its producer's WorkUnit.
    pub(super) async fn ensure_exact_range_report(
        &self,
        execution: &db::Execution,
        frozen: &WorkUnitReviewRange,
        value: &Value,
    ) -> Result<()> {
        let range = ReviewCheckpointRepo::get_review_range(&*self.db, &frozen.range_id)
            .await?
            .ok_or_else(|| range_review::mismatch("review range is unavailable"))?;
        let text = |pointer: &str| value.pointer(pointer).and_then(Value::as_str);
        if frozen.task_id != execution.task_id
            || text("/kind") != Some("review_report")
            || text("/subject/task_id") != Some(execution.task_id.as_str())
            || text("/subject/review_execution_id") != Some(execution.id.as_str())
            || !range_subject_matches(value, frozen, &range)
            || !matches!(
                text("/verdict"),
                Some("pass" | "request_changes" | "questions")
            )
        {
            return Err(range_review::mismatch(
                "persisted range ReviewReport does not match its frozen range",
            ));
        }
        Ok(())
    }

    /// Repository facts the pure validation cannot decide.
    async fn check_range_repository_facts(
        &self,
        repo_path: &Path,
        ctx: &RangeReportContext,
        report: &ValidatedRangeReport,
    ) -> Result<()> {
        let base = ctx.range.base_commit.as_str();
        let head = ctx.range.head_commit.as_str();
        let mut locations = Vec::new();
        for finding in &report.findings {
            locations.extend(finding.locations.iter().cloned());
        }
        for location in locations {
            if !git::path_exists_or_deleted(repo_path, base, head, &location.path).await? {
                return Err(invalid_findings(
                    "a finding path neither exists at the range head nor was deleted by the range",
                ));
            }
            let Some(line_end) = location.line_end else {
                continue;
            };
            // Passing the head twice asks only about the head tree.
            if !git::path_exists_or_deleted(repo_path, head, head, &location.path).await? {
                return Err(invalid_findings(
                    "a deleted file cannot carry finding lines",
                ));
            }
            let line = u32::try_from(line_end)
                .map_err(|_| invalid_findings("finding location lines are invalid"))?;
            let inside = git::blame_lines(repo_path, head, &location.path, line, line)
                .await
                .is_ok_and(|lines| !lines.is_empty());
            if !inside {
                return Err(invalid_findings(
                    "finding lines must lie inside the file at the range head",
                ));
            }
        }
        for resolution in &report.resolutions {
            if resolution.claim != api_types::ReviewResolutionClaim::Fixed {
                continue;
            }
            let anchor = ctx
                .input_findings
                .iter()
                .find(|finding| finding.id == resolution.finding_id)
                .map(|finding| finding.anchor_commit.as_str())
                .ok_or_else(|| invalid_findings("unknown resolved finding"))?;
            // An unprovable ancestry is not ancestry.
            if anchor == head
                || !git::is_ancestor(repo_path, anchor, head)
                    .await
                    .unwrap_or(false)
            {
                return Err(invalid_findings(
                    "fixed requires a head that strictly descends from the finding anchor",
                ));
            }
        }
        Ok(())
    }

    async fn attribute_range_findings(
        &self,
        range: &ReviewRange,
        report: &ValidatedRangeReport,
    ) -> Result<Vec<db::ReviewLocationAttribution>> {
        let locations = report
            .findings
            .iter()
            .flat_map(|finding| &finding.locations)
            .map(|location| FindingLocation {
                path: location.path.clone(),
                line_start: location.line_start,
                line_end: location.line_end,
            })
            .collect::<Vec<_>>();
        if locations.is_empty() {
            return Ok(Vec::new());
        }
        let workspace_root = self.review_workspace_root.clone().ok_or_else(|| {
            ServiceError::invalid_operation("range finding attribution requires a workspace root")
        })?;
        let attributions = ReviewAttribution::new(std::sync::Arc::clone(&self.db), workspace_root)
            .attribute(range, &locations)
            .await?;
        if attributions.len() != locations.len() {
            return Err(ServiceError::invalid_operation(
                "range finding attribution must cover every location",
            ));
        }
        Ok(attributions)
    }

    /// Validate and persist the exact range report of a running reviewer
    /// Execution. A replay returns the persisted Artifact and writes nothing.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn create_range_review_report(
        &self,
        execution: &db::Execution,
        actor: &ActorRef,
        frozen: &WorkUnitReviewRange,
        input: RangeReportInput,
        evidence_considered: Vec<Value>,
        existing_output: Option<Artifact>,
        requested_inputs: Option<ReviewReportInputs>,
    ) -> Result<Artifact> {
        if frozen.task_id != execution.task_id {
            return Err(range_review::mismatch(
                "review range belongs to another Task",
            ));
        }
        let (ctx, checkpoint) = self.range_report_context(frozen).await?;
        let report = validate_range_report(&input, &ctx)?;
        let body = report_body(&execution.id, &report, &evidence_considered);
        if let Some(existing) = existing_output {
            let value: Value = existing
                .content
                .as_deref()
                .and_then(|content| serde_json::from_str(content).ok())
                .ok_or_else(|| range_review::mismatch("persisted ReviewReport is unreadable"))?;
            if existing.task_id != execution.task_id
                || !range_subject_matches(&value, frozen, &ctx.range)
                || body
                    .iter()
                    .any(|(key, field)| value.get(key) != Some(field))
            {
                return Err(range_review::mismatch(
                    "Execution already has a different range ReviewReport",
                ));
            }
            return Ok(existing);
        }
        if execution.status != db::ExecutionStatus::Running {
            return Err(range_review::mismatch(
                "a range ReviewReport requires its running reviewer Execution",
            ));
        }
        if !matches!(
            checkpoint.state,
            ReviewCheckpointState::Pending | ReviewCheckpointState::InReview
        ) {
            return Err(range_review::mismatch(
                "the review checkpoint is no longer active",
            ));
        }
        let contributors =
            ReviewCheckpointRepo::list_review_range_contributors(&*self.db, &ctx.range.id).await?;
        if range_review::contributed(&contributors, actor) {
            let isolated = match actor {
                ActorRef::Human(_) => false,
                ActorRef::Agent(_) => {
                    sqlx::query_scalar::<_, bool>(
                        "SELECT EXISTS(SELECT 1 FROM harness_session s WHERE s.id = ?
                         AND s.status = 'active' AND s.external_session_id IS NOT NULL)",
                    )
                    .bind(execution.harness_session_id.as_deref())
                    .fetch_one(self.db.pool())
                    .await?
                }
            };
            if !isolated {
                return Err(ServiceError::invalid_operation(
                    "range reviewer is not independent of the range",
                ));
            }
        }

        let checkout = range_review::verify_range_checkout(
            &self.db,
            &execution.task_id,
            &frozen.review_work_unit_id,
        )
        .await?;
        let repo_path = range_repository_path(&self.db, &checkout, &ctx.range.repo_id).await?;
        self.check_range_repository_facts(Path::new(&repo_path), &ctx, &report)
            .await?;
        let attributions = self.attribute_range_findings(&ctx.range, &report).await?;

        let range = &ctx.range;
        let artifact_id = new_uuid_v4();
        let now = now_rfc3339();
        let mut content = body;
        content.insert(
            "subject".to_owned(),
            serde_json::json!({
                "task_id": execution.task_id,
                "review_execution_id": execution.id,
                "workspace_id": null,
                "base_commit_sha": null,
                "head_commit_sha": null,
                "workspace_snapshot_digest": null,
                "kind": "commit_range",
                "checkpoint_id": frozen.checkpoint_id,
                "range": {
                    "id": range.id,
                    "repo_id": range.repo_id,
                    "repository_set_revision": range.repository_set_revision,
                    "base_commit": range.base_commit,
                    "head_commit": range.head_commit,
                    "base_tree": range.base_tree,
                    "head_tree": range.head_tree,
                    "range_digest": range.range_digest,
                },
                "checkout": checkout,
                "input_findings_digest": frozen.input_findings_digest,
            }),
        );
        let content = Value::Object(content).to_string();
        let digest = hex::encode(Sha256::digest(content.as_bytes()));

        let mut attributions = attributions.into_iter();
        let mut findings = Vec::with_capacity(report.findings.len());
        let mut counts = [0_u32; 4];
        for (ordinal, finding) in report.findings.iter().enumerate() {
            let id = finding_id(&execution.id, &finding.label);
            counts[match finding.severity {
                ReviewFindingSeverity::P0 => 0,
                ReviewFindingSeverity::P1 => 1,
                ReviewFindingSeverity::P2 => 2,
                ReviewFindingSeverity::P3 => 3,
            }] += 1;
            let finding_digest = canonical_digest(&FindingDigest {
                id: &id,
                report_artifact_id: &artifact_id,
                severity: finding.severity,
                title: &finding.title,
                evidence: &finding.evidence,
                invariant: finding.invariant.as_deref(),
                anchor_commit: &range.head_commit,
                locations: &finding.locations,
            })?;
            let mut locations = Vec::with_capacity(finding.locations.len());
            for (position, location) in finding.locations.iter().enumerate() {
                let attribution = attributions.next().ok_or_else(|| {
                    ServiceError::invalid_operation("range finding attribution is incomplete")
                })?;
                locations.push(ReviewFindingLocation {
                    finding_id: id.clone(),
                    task_id: execution.task_id.clone(),
                    ordinal: position as i64,
                    path: location.path.clone(),
                    line_start: location.line_start,
                    line_end: location.line_end,
                    attribution,
                });
            }
            findings.push((
                ReviewFinding {
                    id,
                    task_id: execution.task_id.clone(),
                    repo_id: range.repo_id.clone(),
                    range_id: range.id.clone(),
                    report_artifact_id: artifact_id.clone(),
                    ordinal: ordinal as i64,
                    label: finding.label.clone(),
                    severity: finding.severity,
                    title: finding.title.clone(),
                    evidence: finding.evidence.clone(),
                    invariant: finding.invariant.clone(),
                    anchor_commit: range.head_commit.clone(),
                    digest: finding_digest,
                    created_at: now.clone(),
                },
                locations,
            ));
        }

        // `still_open` writes no row: the missing resolution keeps it open.
        let resolutions = report
            .resolutions
            .iter()
            .filter_map(|resolution| {
                let outcome = match resolution.claim {
                    api_types::ReviewResolutionClaim::Fixed => ReviewResolutionOutcome::Fixed,
                    api_types::ReviewResolutionClaim::NotReproduced => {
                        ReviewResolutionOutcome::NotReproduced
                    }
                    api_types::ReviewResolutionClaim::StillOpen => return None,
                };
                Some(ReviewFindingResolution {
                    id: stable_id(
                        "forge.review-resolution.v1",
                        [&execution.id, &resolution.finding_id],
                    ),
                    finding_id: resolution.finding_id.clone(),
                    task_id: execution.task_id.clone(),
                    repo_id: range.repo_id.clone(),
                    outcome,
                    anchor_commit: range.head_commit.clone(),
                    report_artifact_id: Some(artifact_id.clone()),
                    decision_id: None,
                    waiver_reference: None,
                    evidence: resolution.evidence.clone(),
                    actor: actor.clone(),
                    created_at: now.clone(),
                })
            })
            .collect::<Vec<_>>();

        let questions = report.verdict == ReviewVerdict::Questions;
        let required_reviewers =
            ReviewPolicyRepo::get_current_task_review_policy(&*self.db, &execution.task_id)
                .await?
                .map_or(1, |policy| policy.policy.required_reviewers);
        let next_state = if questions {
            ReviewCheckpointState::Blocked
        } else if required_reviewers <= 1 {
            ReviewCheckpointState::Reviewed
        } else {
            // v1 admits a single reviewer; more never completes implicitly.
            ReviewCheckpointState::InReview
        };
        let blocked_code = questions.then(|| "review.report.questions".to_owned());

        let source = CollaborationActorSource::Execution(execution.id.clone());
        let artifact_event = self
            .range_event(
                &source,
                actor,
                &execution.task_id,
                ("artifact.created", "artifact", &artifact_id),
                serde_json::json!({
                    "artifact_id": artifact_id,
                    "task_id": execution.task_id,
                    "kind": "review_report",
                    "review_execution_id": execution.id,
                }),
                &now,
            )
            .await?;
        let mut events = vec![
            self.range_event(
                &source,
                actor,
                &execution.task_id,
                ("review.range_reported", "artifact", &artifact_id),
                serde_json::json!({
                    "task_id": execution.task_id,
                    "repo_id": range.repo_id,
                    "checkpoint_id": checkpoint.id,
                    "range_id": range.id,
                    "report_artifact_id": artifact_id,
                    "verdict": report.verdict.to_string(),
                    "p0": counts[0],
                    "p1": counts[1],
                    "p2": counts[2],
                    "p3": counts[3],
                }),
                &now,
            )
            .await?,
        ];
        if next_state != checkpoint.state {
            events.push(
                self.range_event(
                    &source,
                    actor,
                    &execution.task_id,
                    (
                        "review.checkpoint_state_changed",
                        "review_checkpoint",
                        &checkpoint.id,
                    ),
                    serde_json::json!({
                        "task_id": execution.task_id,
                        "repo_id": range.repo_id,
                        "checkpoint_id": checkpoint.id,
                        "from_state": checkpoint.state.to_string(),
                        "state": next_state.to_string(),
                        "version": checkpoint.version + 1,
                        "blocked_code": blocked_code,
                    }),
                    &now,
                )
                .await?,
            );
        }
        for resolution in &resolutions {
            let severity = ctx
                .input_findings
                .iter()
                .find(|finding| finding.id == resolution.finding_id)
                .map(|finding| finding.severity.to_string());
            events.push(
                self.range_event(
                    &source,
                    actor,
                    &execution.task_id,
                    (
                        "review.finding_resolved",
                        "review_finding",
                        &resolution.finding_id,
                    ),
                    serde_json::json!({
                        "task_id": execution.task_id,
                        "repo_id": range.repo_id,
                        "finding_id": resolution.finding_id,
                        "severity": severity,
                        "outcome": resolution.outcome.to_string(),
                        "resolution_id": resolution.id,
                    }),
                    &now,
                )
                .await?,
            );
        }

        let artifact = CreateArtifact {
            id: artifact_id.clone(),
            task_id: execution.task_id.clone(),
            kind: ArtifactKind::ReviewReport,
            storage_kind: ArtifactStorageKind::Inline,
            content: Some(content),
            content_ref: None,
            metadata_json: serde_json::json!({
                "schema_version": 1,
                "review_execution_id": execution.id,
            })
            .to_string(),
            digest: Some(digest.clone()),
            producer_execution_id: execution.id.clone(),
            created_at: now.clone(),
        };
        // Agent inputs were pinned at launch; only a Human submission pins here.
        let (evidence_input_ids, artifact_input_ids) = requested_inputs
            .map(|(evidence, artifacts, _, _)| (evidence, artifacts))
            .unwrap_or_default();
        let write = ReviewRangeReportRepo::ingest_range_review_report(
            &*self.db,
            artifact,
            evidence_input_ids,
            artifact_input_ids,
            artifact_event,
            IngestRangeReport {
                report: ReviewRangeReport {
                    report_artifact_id: artifact_id,
                    task_id: execution.task_id.clone(),
                    repo_id: range.repo_id.clone(),
                    range_id: range.id.clone(),
                    checkpoint_id: checkpoint.id.clone(),
                    review_work_unit_id: frozen.review_work_unit_id.clone(),
                    execution_id: execution.id.clone(),
                    actor: actor.clone(),
                    verdict: report.verdict,
                    report_digest: digest,
                    created_at: now,
                },
                findings,
                resolutions,
                expected_checkpoint_version: checkpoint.version,
                next_checkpoint_state: next_state,
                blocked_code,
                events,
            },
        )
        .await
        .map_err(|error| match error {
            // The checkpoint moved or closed after it was validated.
            db::DbError::VersionConflict => {
                range_review::mismatch("the review checkpoint changed during ingestion")
            }
            error => error.into(),
        })?;
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        Ok(write.artifact)
    }

    async fn range_event(
        &self,
        source: &CollaborationActorSource,
        actor: &ActorRef,
        task_id: &str,
        (event_type, entity_type, entity_id): (&str, &str, &str),
        payload: Value,
        now: &str,
    ) -> Result<CreateDomainEvent> {
        self.event_from_source(
            source,
            EventScope {
                event_type,
                entity_type,
                entity_id,
                task_id,
            },
            actor,
            payload,
            now,
        )
        .await
    }
}

async fn range_repository_path(
    db: &db::SqliteDb,
    checkout: &[RangeCheckout],
    repo_id: &str,
) -> Result<String> {
    let member = checkout
        .iter()
        .find(|member| member.repo_id == repo_id)
        .ok_or_else(|| range_review::mismatch("range repository checkout is missing"))?;
    Ok(WorkspaceRepo::get_by_id(db, &member.workspace_id)
        .await?
        .ok_or_else(|| super::not_found("workspace", member.workspace_id.clone()))?
        .worktree_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn range() -> ReviewRange {
        ReviewRange {
            id: "range".to_owned(),
            task_id: "task".to_owned(),
            repo_id: "repo".to_owned(),
            repository_set_revision: 1,
            base_commit: "base".to_owned(),
            head_commit: "head".to_owned(),
            base_tree: "base-tree".to_owned(),
            head_tree: "head-tree".to_owned(),
            range_digest: "a".repeat(64),
            integration_workspace_id: "workspace".to_owned(),
            head_integration_id: "integration".to_owned(),
            created_at: "2026-11-27T00:00:00Z".to_owned(),
        }
    }

    fn input_finding(id: &str, severity: ReviewFindingSeverity, anchor: &str) -> ReviewFinding {
        ReviewFinding {
            id: id.to_owned(),
            task_id: "task".to_owned(),
            repo_id: "repo".to_owned(),
            range_id: "earlier".to_owned(),
            report_artifact_id: "report".to_owned(),
            ordinal: 0,
            label: id.to_owned(),
            severity,
            title: "title".to_owned(),
            evidence: "evidence".to_owned(),
            invariant: None,
            anchor_commit: anchor.to_owned(),
            digest: "b".repeat(64),
            created_at: "2026-11-27T00:00:00Z".to_owned(),
        }
    }

    fn context(input_findings: Vec<ReviewFinding>, recheck: bool) -> RangeReportContext {
        RangeReportContext {
            range: range(),
            input_findings,
            recheck,
        }
    }

    fn result(verdict: &str, findings: Value, resolutions: Value, questions: Value) -> String {
        format!(
            "Done.\nFORGE_RESULT: {}",
            json!({
                "schema_version": 2,
                "kind": "review",
                "verdict": verdict,
                "summary": "Reviewed the range.",
                "findings": findings,
                "resolutions": resolutions,
                "questions": questions,
            })
        )
    }

    fn finding(label: &str, severity: &str) -> Value {
        json!({
            "label": label,
            "severity": severity,
            "title": "Defect",
            "evidence": "Reproduced",
            "locations": [{"path": "src/a.rs", "line_start": 3}],
        })
    }

    fn resolution(id: &str, outcome: &str) -> Value {
        json!({"finding_id": id, "outcome": outcome, "evidence": "Checked"})
    }

    fn check(output: &str, ctx: &RangeReportContext) -> Result<ValidatedRangeReport> {
        validate_range_report(&parse_range_review_result(output)?, ctx)
    }

    #[test]
    fn review_range_parser_requires_one_final_version_2_result() {
        let valid = result("pass", json!([]), json!([]), json!([]));
        assert!(parse_range_review_result(&valid).is_ok());
        for invalid in [
            format!("{valid}\ntrailing"),
            format!("{valid}\n{valid}"),
            "no marker".to_owned(),
            valid.replace("\"schema_version\":2", "\"schema_version\":1"),
            valid.replace("\"resolutions\":[],", ""),
            valid.replace("\"summary\"", "\"extra\":1,\"summary\""),
        ] {
            assert!(parse_range_review_result(&invalid).is_err(), "{invalid}");
        }
        let unstructured = result("fail", json!(["a string finding"]), json!([]), json!([]));
        assert!(matches!(
            parse_range_review_result(&unstructured),
            Err(ServiceError::Coordination {
                code: "review.report.invalid_findings",
                ..
            })
        ));
    }

    #[test]
    fn review_range_report_verdicts_match_their_findings() {
        let ctx = context(Vec::new(), false);
        let report = check(
            &result("fail", json!([finding("F-1", "P1")]), json!([]), json!([])),
            &ctx,
        )
        .unwrap();
        assert_eq!(report.verdict, ReviewVerdict::RequestChanges);
        assert_eq!(report.findings[0].locations[0].line_end, Some(3));
        assert!(check(
            &result("pass", json!([finding("F-1", "P2")]), json!([]), json!([])),
            &ctx
        )
        .is_ok());
        for invalid in [
            result("pass", json!([finding("F-1", "P0")]), json!([]), json!([])),
            result("fail", json!([]), json!([]), json!([])),
            result("pass", json!([]), json!([]), json!(["why?"])),
            result("needs_human", json!([]), json!([]), json!([])),
            result(
                "needs_human",
                json!([finding("F-1", "P3")]),
                json!([]),
                json!(["why?"]),
            ),
            result(
                "fail",
                json!([finding("F-1", "P1"), finding("F-1", "P2")]),
                json!([]),
                json!([]),
            ),
            result(
                "fail",
                json!([finding("bad label", "P1")]),
                json!([]),
                json!([]),
            ),
        ] {
            assert!(check(&invalid, &ctx).is_err(), "{invalid}");
        }
        assert_eq!(
            check(
                &result("needs_human", json!([]), json!([]), json!(["why?"])),
                &ctx
            )
            .unwrap()
            .verdict,
            ReviewVerdict::Questions
        );
    }

    #[test]
    fn review_range_report_rejects_unsafe_locations() {
        let ctx = context(Vec::new(), false);
        for location in [
            json!({"path": "../a"}),
            json!({"path": "/a"}),
            json!({"path": "a\\b"}),
            json!({"path": "a", "line_end": 2}),
            json!({"path": "a", "line_start": 3, "line_end": 2}),
            json!({"path": "a", "line_start": 0}),
        ] {
            let mut invalid = finding("F-1", "P1");
            invalid["locations"] = json!([location]);
            assert!(check(
                &result("fail", json!([invalid]), json!([]), json!([])),
                &ctx
            )
            .is_err());
        }
    }

    #[test]
    fn review_range_report_resolves_only_frozen_findings() {
        let ctx = context(
            vec![
                input_finding("blocking", ReviewFindingSeverity::P1, "older"),
                input_finding("minor", ReviewFindingSeverity::P2, "older"),
            ],
            false,
        );
        let pass =
            |resolutions: Value| check(&result("pass", json!([]), resolutions, json!([])), &ctx);
        assert!(pass(json!([resolution("blocking", "fixed")])).is_ok());
        assert!(pass(json!([
            resolution("blocking", "fixed"),
            resolution("minor", "still_open")
        ]))
        .is_ok());
        assert!(pass(json!([])).is_err());
        assert!(pass(json!([resolution("blocking", "still_open")])).is_err());
        assert!(pass(json!([resolution("blocking", "not_reproduced")])).is_err());
        assert!(pass(json!([
            resolution("blocking", "fixed"),
            resolution("blocking", "fixed")
        ]))
        .is_err());
        assert!(pass(json!([
            resolution("blocking", "fixed"),
            resolution("unknown", "fixed")
        ]))
        .is_err());
        assert!(check(
            &result(
                "fail",
                json!([]),
                json!([resolution("blocking", "still_open")]),
                json!([])
            ),
            &ctx
        )
        .is_ok());
    }

    #[test]
    fn review_range_report_recheck_and_anchor_rules() {
        let recheck = context(
            vec![input_finding("blocking", ReviewFindingSeverity::P0, "head")],
            true,
        );
        let pass = |outcome: &str| {
            check(
                &result(
                    "pass",
                    json!([]),
                    json!([resolution("blocking", outcome)]),
                    json!([]),
                ),
                &recheck,
            )
        };
        assert!(pass("not_reproduced").is_ok());
        // The anchor is the head itself, so nothing newer can have fixed it.
        assert!(pass("fixed").is_err());
    }

    #[test]
    fn review_range_finding_ids_are_stable_per_execution_and_label() {
        assert_eq!(
            finding_id("execution", "F-1"),
            finding_id("execution", "F-1")
        );
        assert_ne!(
            finding_id("execution", "F-1"),
            finding_id("execution", "F-2")
        );
        assert_ne!(finding_id("execution", "F-1"), finding_id("other", "F-1"));
    }
}
