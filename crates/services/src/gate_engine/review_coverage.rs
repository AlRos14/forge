//! Gate policy schema v4: schema v3 plus the Task review-coverage requirement.
//!
//! Every generator of a `merge_readiness` policy goes through
//! [`merge_policy_preserving_coverage`] so a current Task review policy is never
//! dropped. The evaluator is pure and fails closed: without exact, current
//! coverage evidence a v4 policy is never satisfied.

use std::collections::{BTreeMap, BTreeSet};

use api_types::ReviewCoverageStatus;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    invalid, normalize_policy, normalize_required_checks, normalize_required_checks_v3, sha256,
    validate_text, DecisionRequirement, GatePolicyDocument, GatePolicyDocumentV2,
    GatePolicyDocumentV3, GateScopeRequirement, InputDraft, RepositoryValidationSubject,
    RequiredCheckV3, ReviewSetPolicy, ValidationRequirement, WorkUnitRequirement,
};
use crate::{Result, ServiceError};

const COVERAGE_MISSING: &str = "gate.review_coverage_missing";
const FACTS_STALE: &str = "review.coverage.facts_stale";

/// Exact Task review policy revision a v4 Gate policy is pinned to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateReviewCoverageRequirement {
    pub review_policy_revision: i64,
    pub review_policy_digest: String,
}

/// Gate policy schema v4. `review` stays optional and, when present, is required
/// in addition to coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatePolicyDocumentV4 {
    pub schema_version: u32,
    #[serde(default)]
    pub scope_requirement: Option<GateScopeRequirement>,
    #[serde(default)]
    pub review: Option<ReviewSetPolicy>,
    #[serde(default)]
    pub validations: Vec<ValidationRequirement>,
    #[serde(default)]
    pub decisions: Vec<DecisionRequirement>,
    #[serde(default)]
    pub work_units: Vec<WorkUnitRequirement>,
    #[serde(default)]
    pub required_checks: Vec<RequiredCheckV3>,
    pub review_coverage: GateReviewCoverageRequirement,
}

impl GatePolicyDocumentV4 {
    pub(super) fn into_parts(
        self,
    ) -> Result<(
        GatePolicyDocument,
        Vec<RequiredCheckV3>,
        GateReviewCoverageRequirement,
    )> {
        if self.schema_version != 4 {
            return Err(invalid("Gate policy v4 must use schema version 4"));
        }
        Ok((
            GatePolicyDocument {
                schema_version: 1,
                scope_requirement: self.scope_requirement,
                review: self.review,
                validations: self.validations,
                decisions: self.decisions,
                work_units: self.work_units,
            },
            self.required_checks,
            self.review_coverage,
        ))
    }

    pub(super) fn from_parts(
        base: GatePolicyDocument,
        required_checks: Vec<RequiredCheckV3>,
        review_coverage: GateReviewCoverageRequirement,
    ) -> Self {
        Self {
            schema_version: 4,
            scope_requirement: base.scope_requirement,
            review: base.review,
            validations: base.validations,
            decisions: base.decisions,
            work_units: base.work_units,
            required_checks,
            review_coverage,
        }
    }
}

pub(super) fn validate_coverage_requirement(
    requirement: &GateReviewCoverageRequirement,
) -> Result<()> {
    if requirement.review_policy_revision < 1 || !is_digest(&requirement.review_policy_digest) {
        return Err(invalid(
            "review_coverage requires a positive policy revision and a 64 lowercase hex digest",
        ));
    }
    Ok(())
}

/// The single constructor for `merge_readiness` policy documents produced by
/// Gate generators. It must not be applied to any other Gate kind.
///
/// Without a current Task review policy `next` is returned unchanged and a v4
/// document is rejected. With one, a v1, v2, v3 or v4 document becomes a
/// normalized v4 document pinned to exactly that policy. Schema v2 checks carry
/// no repository, so converting them requires `v2_check_repo_id`.
pub fn merge_policy_preserving_coverage(
    next: Value,
    policy: Option<&db::ReviewPolicyRef>,
    v2_check_repo_id: Option<&str>,
) -> Result<Value> {
    let schema_version = next.get("schema_version").and_then(Value::as_u64);
    let Some(policy) = policy else {
        if schema_version == Some(4) {
            return Err(coverage_missing(
                "Gate policy v4 requires a current Task review policy",
            ));
        }
        return Ok(next);
    };
    let review_coverage = GateReviewCoverageRequirement {
        review_policy_revision: policy.revision,
        review_policy_digest: policy.digest.clone(),
    };
    validate_coverage_requirement(&review_coverage)?;
    let (mut base, required_checks) = match schema_version {
        Some(1) => (
            decode::<GatePolicyDocument>(next)?,
            Vec::<RequiredCheckV3>::new(),
        ),
        Some(2) => {
            let (base, checks) = decode::<GatePolicyDocumentV2>(next)?.into_parts()?;
            let checks = normalize_required_checks(checks)?;
            let checks = if checks.is_empty() {
                Vec::new()
            } else {
                let repo_id = v2_check_repo_id.ok_or_else(|| {
                    invalid("converting schema v2 required_checks needs their repository")
                })?;
                checks
                    .into_iter()
                    .map(|check| RequiredCheckV3 {
                        repo_id: repo_id.to_owned(),
                        check_key: check.check_key,
                        definition_digest: check.definition_digest,
                        required_outcome: check.required_outcome,
                    })
                    .collect()
            };
            (base, checks)
        }
        Some(3) => decode::<GatePolicyDocumentV3>(next)?.into_parts()?,
        Some(4) => {
            let (base, checks, _) = decode::<GatePolicyDocumentV4>(next)?.into_parts()?;
            (base, checks)
        }
        _ => return Err(invalid("unknown Gate policy schema version")),
    };
    normalize_policy(&mut base)?;
    let required_checks = normalize_required_checks_v3(required_checks)?;
    serde_json::to_value(GatePolicyDocumentV4::from_parts(
        base,
        required_checks,
        review_coverage,
    ))
    .map_err(|error| invalid(format!("Gate policy cannot be serialized: {error}")))
}

/// Canonical stored form of a v4 policy: `(policy_json, policy_digest)`. Writers
/// that revise a Gate inside their own transaction use this so the bytes match
/// those written by `GateEngine`.
pub fn encode_gate_policy_v4(policy: GatePolicyDocumentV4) -> Result<(String, String)> {
    let (mut base, required_checks, review_coverage) = policy.into_parts()?;
    validate_coverage_requirement(&review_coverage)?;
    normalize_policy(&mut base)?;
    let required_checks = normalize_required_checks_v3(required_checks)?;
    let policy_json = serde_json::to_string(&GatePolicyDocumentV4::from_parts(
        base,
        required_checks,
        review_coverage,
    ))
    .map_err(|error| invalid(format!("Gate policy cannot be serialized: {error}")))?;
    let policy_digest = sha256(policy_json.as_bytes());
    Ok((policy_json, policy_digest))
}

/// Coverage of one repository, computed outside the Gate engine because it needs
/// Git facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryCoverageSubject {
    pub repo_id: String,
    /// Coverage ledger revision the snapshot was computed from.
    pub revision: i64,
    pub status: ReviewCoverageStatus,
    pub snapshot_digest: String,
    pub base_commit: Option<String>,
    pub candidate_commit: Option<String>,
}

/// Exact coverage evidence supplied to one evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCoverageEvidence {
    pub policy: db::ReviewPolicyRef,
    pub repositories: Vec<RepositoryCoverageSubject>,
}

/// Durable facts the evidence is checked against.
pub(super) struct CoverageExpectation<'a> {
    pub current_policy: Option<&'a db::ReviewPolicyRef>,
    /// Every writable repository with a successful integration, mapped to its
    /// current coverage ledger revision.
    pub repository_revisions: &'a BTreeMap<String, i64>,
    pub repository_subjects: Option<&'a [RepositoryValidationSubject]>,
}

pub(super) fn evaluate_review_coverage(
    requirement: &GateReviewCoverageRequirement,
    expected: &CoverageExpectation<'_>,
    evidence: Option<&ReviewCoverageEvidence>,
    inputs: &mut Vec<InputDraft>,
    missing: &mut Vec<String>,
    issues: &mut Vec<String>,
) {
    let Some(current) = expected.current_policy else {
        issues.push(COVERAGE_MISSING.to_owned());
        return;
    };
    if !requirement_matches(requirement, current) {
        issues.push(COVERAGE_MISSING.to_owned());
        return;
    }
    let Some(evidence) = evidence else {
        missing.push("review_coverage_facts".to_owned());
        return;
    };
    if &evidence.policy != current {
        issues.push(FACTS_STALE.to_owned());
        return;
    }
    let mut seen = BTreeSet::new();
    if evidence
        .repositories
        .iter()
        .any(|subject| !seen.insert(subject.repo_id.as_str()))
    {
        issues.push("review_coverage_duplicate_repository".to_owned());
        return;
    }
    for (repo_id, revision) in expected.repository_revisions {
        let Some(subject) = evidence
            .repositories
            .iter()
            .find(|subject| &subject.repo_id == repo_id)
        else {
            missing.push(format!("review_coverage:{repo_id}"));
            continue;
        };
        let candidate_moved = expected.repository_subjects.is_some_and(|subjects| {
            subjects.iter().any(|changed| {
                &changed.repo_id == repo_id
                    && subject.candidate_commit.as_deref() != Some(changed.commit_sha.as_str())
            })
        });
        if *revision < 1
            || subject.revision != *revision
            || !is_digest(&subject.snapshot_digest)
            || candidate_moved
        {
            issues.push(format!("{FACTS_STALE}:{repo_id}"));
            continue;
        }
        let status = status_str(subject.status);
        if !status_satisfies(subject.status) {
            issues.push(format!("review_coverage_not_satisfied:{repo_id}:{status}"));
        }
        inputs.push(InputDraft {
            input_kind: "review_coverage".to_owned(),
            input_id: repo_id.clone(),
            input_version: subject.revision,
            input_digest: subject.snapshot_digest.clone(),
            producer_ref: None,
            subject_json: json!({
                "review_policy_revision": current.revision,
                "review_policy_digest": current.digest,
                "snapshot_digest": subject.snapshot_digest,
                "base_commit": subject.base_commit,
                "candidate_commit": subject.candidate_commit,
            })
            .to_string(),
            status: status.to_owned(),
        });
    }
}

pub(super) fn requirement_matches(
    requirement: &GateReviewCoverageRequirement,
    current: &db::ReviewPolicyRef,
) -> bool {
    requirement.review_policy_revision == current.revision
        && requirement.review_policy_digest == current.digest
}

pub(super) fn coverage_missing(message: &str) -> ServiceError {
    ServiceError::Coordination {
        code: COVERAGE_MISSING,
        message: message.to_owned(),
    }
}

/// Validates the repository identifiers of evidence supplied by a caller.
pub(super) fn validate_evidence(evidence: &ReviewCoverageEvidence) -> Result<()> {
    for subject in &evidence.repositories {
        validate_text("review_coverage.repo_id", &subject.repo_id, 128)?;
    }
    Ok(())
}

fn status_satisfies(status: ReviewCoverageStatus) -> bool {
    matches!(
        status,
        ReviewCoverageStatus::Satisfied
            | ReviewCoverageStatus::NotRequired
            | ReviewCoverageStatus::Unchanged
    )
}

fn status_str(status: ReviewCoverageStatus) -> &'static str {
    match status {
        ReviewCoverageStatus::NotRequired => "not_required",
        ReviewCoverageStatus::Unchanged => "unchanged",
        ReviewCoverageStatus::Uncovered => "uncovered",
        ReviewCoverageStatus::Partial => "partial",
        ReviewCoverageStatus::InReview => "in_review",
        ReviewCoverageStatus::FindingsOpen => "findings_open",
        ReviewCoverageStatus::HistoryRewritten => "history_rewritten",
        ReviewCoverageStatus::Blocked => "blocked",
        ReviewCoverageStatus::Satisfied => "satisfied",
    }
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value)
        .map_err(|error| invalid(format!("invalid Gate policy document: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn policy_ref() -> db::ReviewPolicyRef {
        db::ReviewPolicyRef {
            revision: 2,
            digest: digest('a'),
        }
    }

    fn requirement() -> GateReviewCoverageRequirement {
        GateReviewCoverageRequirement {
            review_policy_revision: 2,
            review_policy_digest: digest('a'),
        }
    }

    fn work_units() -> Value {
        json!([{
            "work_unit_id": "unit",
            "version": 2,
            "dependency_digest": digest('d'),
            "execution_id": "execution",
            "execution_result_sha": "abc",
            "require_integration": false,
            "integration_id": null,
            "integration_version": null,
        }])
    }

    fn check() -> Value {
        json!({
            "check_key": "unit-tests",
            "definition_digest": digest('c'),
            "required_outcome": serde_json::to_value(db::ValidationRunStatus::Passed)
                .expect("status"),
        })
    }

    fn convert(next: Value, repo: Option<&str>) -> Result<GatePolicyDocumentV4> {
        let value = merge_policy_preserving_coverage(next, Some(&policy_ref()), repo)?;
        Ok(serde_json::from_value(value).expect("v4 document"))
    }

    #[test]
    fn review_range_gate_helper_without_policy_is_identity_and_rejects_v4() {
        let next = json!({"schema_version": 1, "work_units": work_units()});
        assert_eq!(
            merge_policy_preserving_coverage(next.clone(), None, None).expect("unchanged"),
            next
        );
        let v4 =
            merge_policy_preserving_coverage(next, Some(&policy_ref()), None).expect("v4 policy");
        assert!(matches!(
            merge_policy_preserving_coverage(v4, None, None),
            Err(ServiceError::Coordination {
                code: COVERAGE_MISSING,
                ..
            })
        ));
    }

    #[test]
    fn review_range_gate_helper_converts_v1_v2_and_v3() {
        let v1 = convert(
            json!({"schema_version": 1, "work_units": work_units()}),
            None,
        )
        .expect("v1");
        assert_eq!(v1.schema_version, 4);
        assert_eq!(v1.work_units.len(), 1);
        assert!(v1.review.is_none());
        assert_eq!(v1.review_coverage, requirement());

        let v2 = json!({"schema_version": 2, "required_checks": [check()]});
        assert!(convert(v2.clone(), None).is_err());
        let v2 = convert(v2, Some("repo")).expect("v2");
        assert_eq!(v2.required_checks[0].repo_id, "repo");
        assert_eq!(v2.required_checks[0].check_key, "unit-tests");

        let mut scoped = check();
        scoped["repo_id"] = json!("other");
        let v3 = convert(
            json!({"schema_version": 3, "required_checks": [scoped]}),
            Some("ignored"),
        )
        .expect("v3");
        assert_eq!(v3.required_checks[0].repo_id, "other");
    }

    #[test]
    fn review_range_gate_helper_repins_v4_and_fails_closed() {
        let stale = json!({
            "schema_version": 4,
            "review_coverage": {
                "review_policy_revision": 1,
                "review_policy_digest": digest('b'),
            },
        });
        assert_eq!(
            convert(stale, None).expect("repinned").review_coverage,
            requirement()
        );
        assert!(convert(json!({"schema_version": 5}), None).is_err());
        assert!(convert(json!({}), None).is_err());
        assert!(convert(json!({"schema_version": 1, "required_checks": []}), None).is_err());
        let invalid_ref = db::ReviewPolicyRef {
            revision: 0,
            digest: digest('a'),
        };
        assert!(merge_policy_preserving_coverage(
            json!({"schema_version": 1}),
            Some(&invalid_ref),
            None
        )
        .is_err());
    }

    fn subject(status: ReviewCoverageStatus) -> RepositoryCoverageSubject {
        RepositoryCoverageSubject {
            repo_id: "repo".to_owned(),
            revision: 3,
            status,
            snapshot_digest: digest('e'),
            base_commit: Some("base".to_owned()),
            candidate_commit: Some("candidate".to_owned()),
        }
    }

    fn evidence(subjects: Vec<RepositoryCoverageSubject>) -> ReviewCoverageEvidence {
        ReviewCoverageEvidence {
            policy: policy_ref(),
            repositories: subjects,
        }
    }

    fn evaluate(
        current: Option<&db::ReviewPolicyRef>,
        evidence: Option<&ReviewCoverageEvidence>,
        candidate: Option<&str>,
    ) -> (Vec<InputDraft>, Vec<String>, Vec<String>) {
        let revisions = BTreeMap::from([("repo".to_owned(), 3)]);
        let changed = candidate.map(|commit| {
            vec![RepositoryValidationSubject {
                repo_id: "repo".to_owned(),
                workspace_id: "workspace".to_owned(),
                commit_sha: commit.to_owned(),
            }]
        });
        let (mut inputs, mut missing, mut issues) = (Vec::new(), Vec::new(), Vec::new());
        evaluate_review_coverage(
            &requirement(),
            &CoverageExpectation {
                current_policy: current,
                repository_revisions: &revisions,
                repository_subjects: changed.as_deref(),
            },
            evidence,
            &mut inputs,
            &mut missing,
            &mut issues,
        );
        (inputs, missing, issues)
    }

    #[test]
    fn review_range_gate_coverage_accepts_only_satisfying_statuses() {
        let current = policy_ref();
        for status in [
            ReviewCoverageStatus::Satisfied,
            ReviewCoverageStatus::NotRequired,
            ReviewCoverageStatus::Unchanged,
            ReviewCoverageStatus::Uncovered,
            ReviewCoverageStatus::Partial,
            ReviewCoverageStatus::InReview,
            ReviewCoverageStatus::FindingsOpen,
            ReviewCoverageStatus::HistoryRewritten,
            ReviewCoverageStatus::Blocked,
        ] {
            let evidence = evidence(vec![subject(status)]);
            let (inputs, missing, issues) =
                evaluate(Some(&current), Some(&evidence), Some("candidate"));
            assert!(missing.is_empty());
            assert_eq!(issues.is_empty(), status_satisfies(status));
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0].input_kind, "review_coverage");
            assert_eq!(inputs[0].input_id, "repo");
            assert_eq!(inputs[0].input_version, 3);
            assert_eq!(
                serde_json::to_value(status).expect("status"),
                json!(inputs[0].status)
            );
        }
    }

    #[test]
    fn review_range_gate_coverage_fails_closed_without_exact_evidence() {
        let current = policy_ref();
        let satisfied = evidence(vec![subject(ReviewCoverageStatus::Satisfied)]);

        let (_, _, issues) = evaluate(None, Some(&satisfied), None);
        assert_eq!(issues, [COVERAGE_MISSING]);

        let newer = db::ReviewPolicyRef {
            revision: 3,
            digest: digest('f'),
        };
        let (_, _, issues) = evaluate(Some(&newer), Some(&satisfied), None);
        assert_eq!(issues, [COVERAGE_MISSING]);

        let (inputs, missing, _) = evaluate(Some(&current), None, None);
        assert!(inputs.is_empty());
        assert_eq!(missing, ["review_coverage_facts"]);

        let (_, missing, _) = evaluate(Some(&current), Some(&evidence(Vec::new())), None);
        assert_eq!(missing, ["review_coverage:repo"]);

        let mut wrong_policy = satisfied.clone();
        wrong_policy.policy = newer;
        let mut wrong_revision = satisfied.clone();
        wrong_revision.repositories[0].revision = 2;
        let mut wrong_digest = satisfied.clone();
        wrong_digest.repositories[0].snapshot_digest = "short".to_owned();
        let duplicate = evidence(vec![
            subject(ReviewCoverageStatus::Satisfied),
            subject(ReviewCoverageStatus::Blocked),
        ]);
        for invalid in [wrong_policy, wrong_revision, wrong_digest, duplicate] {
            let (inputs, _, issues) = evaluate(Some(&current), Some(&invalid), None);
            assert!(inputs.is_empty());
            assert_eq!(issues.len(), 1);
        }

        let (inputs, _, issues) = evaluate(Some(&current), Some(&satisfied), Some("moved"));
        assert!(inputs.is_empty());
        assert_eq!(issues, [format!("{FACTS_STALE}:repo")]);
    }
}
