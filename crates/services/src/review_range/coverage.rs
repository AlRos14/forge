//! Per-repository coverage: a pure evaluator over durable rows and git facts,
//! and the service that gathers both.

use super::canonical_digest;
use crate::{task_service::workspace::resolve_repo_source, Result, ServiceError};
use db::{
    ActorKind, RepoRepo, ReviewCheckpoint, ReviewCheckpointState, ReviewCoverageRows,
    ReviewLedgerRepo, ReviewPolicyRef, ReviewRange, ReviewVerdict, SqliteDb, TaskRepo,
    TaskRepositoryRole, WorkUnitRepositoryIntegrationOutcome,
};
use serde::Serialize;
use sqlx::SqliteConnection;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

const BASE_UNKNOWN: &str = "review.coverage.base_unknown";
const BASE_NOT_ANCESTOR: &str = "review.coverage.base_not_ancestor";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCandidate {
    pub repo_id: String,
    pub commit_sha: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewCoverageStatus {
    NotRequired,
    Unchanged,
    Uncovered,
    Partial,
    InReview,
    FindingsOpen,
    HistoryRewritten,
    Blocked,
    Satisfied,
}

impl ReviewCoverageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Unchanged => "unchanged",
            Self::Uncovered => "uncovered",
            Self::Partial => "partial",
            Self::InReview => "in_review",
            Self::FindingsOpen => "findings_open",
            Self::HistoryRewritten => "history_rewritten",
            Self::Blocked => "blocked",
            Self::Satisfied => "satisfied",
        }
    }

    fn gate_status(self) -> api_types::ReviewCoverageStatus {
        match self {
            Self::NotRequired => api_types::ReviewCoverageStatus::NotRequired,
            Self::Unchanged => api_types::ReviewCoverageStatus::Unchanged,
            Self::Uncovered => api_types::ReviewCoverageStatus::Uncovered,
            Self::Partial => api_types::ReviewCoverageStatus::Partial,
            Self::InReview => api_types::ReviewCoverageStatus::InReview,
            Self::FindingsOpen => api_types::ReviewCoverageStatus::FindingsOpen,
            Self::HistoryRewritten => api_types::ReviewCoverageStatus::HistoryRewritten,
            Self::Blocked => api_types::ReviewCoverageStatus::Blocked,
            Self::Satisfied => api_types::ReviewCoverageStatus::Satisfied,
        }
    }
}

/// Field order is part of the snapshot digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoveredRange {
    pub range_id: String,
    pub base_commit: String,
    pub head_commit: String,
    pub valid_edge: bool,
    pub report_artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeverityCounts {
    pub p0: u32,
    pub p1: u32,
    pub p2: u32,
    pub p3: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCoverage {
    pub repo_id: String,
    pub status: ReviewCoverageStatus,
    pub reason_code: Option<String>,
    pub revision: i64,
    pub base_commit: Option<String>,
    pub candidate_commit: Option<String>,
    pub frontier_commit: Option<String>,
    pub unreviewed_commits: u32,
    pub ranges: Vec<CoveredRange>,
    pub open_findings: SeverityCounts,
    pub open_finding_ids: Vec<String>,
    pub resolution_ids: Vec<String>,
    pub active_checkpoint_id: Option<String>,
    pub snapshot_digest: String,
}

/// Git facts computed outside any transaction. An absent fact is read as "not
/// an ancestor", so incomplete facts never cover a candidate or close a finding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitFacts {
    /// Empty when no candidate commit is known.
    pub candidate: String,
    /// Descendant commit to those queried commits that are its ancestors.
    pub ancestors_of: BTreeMap<String, BTreeSet<String>>,
    /// Commits between an ancestor of the candidate and the candidate.
    pub distance_to_candidate: BTreeMap<String, u32>,
    pub base_tree: String,
    pub candidate_tree: String,
}

impl CommitFacts {
    /// Ancestor of, or equal to, `descendant`.
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> bool {
        ancestor == descendant
            || self
                .ancestors_of
                .get(descendant)
                .is_some_and(|ancestors| ancestors.contains(ancestor))
    }

    fn distance(&self, commit: &str) -> u32 {
        if commit == self.candidate {
            return 0;
        }
        self.distance_to_candidate
            .get(commit)
            .copied()
            .unwrap_or(u32::MAX)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoverageFacts {
    pub task_id: String,
    /// `repo_id` to its coverage revision and facts.
    pub repos: BTreeMap<String, (i64, CommitFacts)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageSnapshot {
    pub policy: ReviewPolicyRef,
    /// Ordered by `repo_id`.
    pub repos: Vec<RepoCoverage>,
}

impl CoverageSnapshot {
    /// Converts the exact prepared snapshot to Gate v4 evidence. Repositories
    /// retain the snapshot order established by `CoverageFacts`.
    pub fn gate_evidence(&self) -> crate::gate_engine::ReviewCoverageEvidence {
        crate::gate_engine::ReviewCoverageEvidence {
            policy: self.policy.clone(),
            repositories: self
                .repos
                .iter()
                .map(|repo| crate::gate_engine::RepositoryCoverageSubject {
                    repo_id: repo.repo_id.clone(),
                    revision: repo.revision,
                    status: repo.status.gate_status(),
                    snapshot_digest: repo.snapshot_digest.clone(),
                    base_commit: repo.base_commit.clone(),
                    candidate_commit: repo.candidate_commit.clone(),
                })
                .collect(),
        }
    }
}

/// Field order is part of the digest.
#[derive(Serialize)]
struct CanonicalSnapshot<'a> {
    repo_id: &'a str,
    review_policy_digest: Option<&'a str>,
    base_commit: Option<&'a str>,
    candidate_commit: Option<&'a str>,
    ranges: &'a [CoveredRange],
    open_finding_ids: &'a [String],
    resolution_ids: &'a [String],
    active_checkpoint_id: Option<&'a str>,
    status: ReviewCoverageStatus,
}

/// Evaluates one repository. The repository identity is taken from the rows;
/// rows that name none yield an empty `repo_id`.
pub fn evaluate_coverage(rows: &ReviewCoverageRows, facts: &CommitFacts) -> RepoCoverage {
    let repo_id = rows
        .base
        .as_ref()
        .map(|base| &base.repo_id)
        .or_else(|| rows.integrations.first().map(|row| &row.repo_id))
        .or_else(|| rows.checkpoints.first().map(|row| &row.repo_id))
        .or_else(|| rows.ranges.first().map(|row| &row.repo_id))
        .cloned()
        .unwrap_or_default();
    evaluate_repo(&repo_id, rows, facts)
}

fn evaluate_repo(repo_id: &str, rows: &ReviewCoverageRows, facts: &CommitFacts) -> RepoCoverage {
    let candidate = Some(facts.candidate.as_str()).filter(|commit| !commit.is_empty());
    let base = rows.base.as_ref().map(|base| base.base_commit.as_str());
    // The base is usable only while it remains an ancestor of the candidate.
    let anchored =
        base.filter(|base| candidate.is_some_and(|candidate| facts.is_ancestor(base, candidate)));

    let mut history_rewritten = false;
    let mut ranges = rows
        .ranges
        .iter()
        .filter_map(|range| {
            let review = review_of(rows, range)?;
            let descends = range.base_commit != range.head_commit
                && facts.is_ancestor(&range.base_commit, &range.head_commit);
            let in_candidate =
                candidate.is_some_and(|candidate| facts.is_ancestor(&range.head_commit, candidate));
            history_rewritten |= review.quorum && !in_candidate;
            Some(CoveredRange {
                range_id: range.id.clone(),
                base_commit: range.base_commit.clone(),
                head_commit: range.head_commit.clone(),
                valid_edge: review.quorum && descends && in_candidate,
                report_artifact_ids: review.report_artifact_ids,
            })
        })
        .collect::<Vec<_>>();
    ranges.sort_by(|left, right| left.range_id.cmp(&right.range_id));

    let frontier = anchored.map(|base| frontier(base, &ranges, facts));
    let covered = frontier.is_some() && frontier == candidate;
    let unreviewed_commits = frontier.map_or(0, |node| facts.distance(node));

    let mut open_findings = SeverityCounts::default();
    let mut open_finding_ids = Vec::new();
    let mut blocking_open = false;
    for finding in &rows.findings {
        let resolved = candidate.is_some_and(|candidate| {
            rows.resolutions.iter().any(|resolution| {
                resolution.finding_id == finding.id
                    && facts.is_ancestor(&resolution.anchor_commit, candidate)
            })
        });
        if resolved {
            continue;
        }
        let count = match finding.severity {
            db::ReviewFindingSeverity::P0 => &mut open_findings.p0,
            db::ReviewFindingSeverity::P1 => &mut open_findings.p1,
            db::ReviewFindingSeverity::P2 => &mut open_findings.p2,
            db::ReviewFindingSeverity::P3 => &mut open_findings.p3,
        };
        *count = count.saturating_add(1);
        blocking_open |= finding.severity.is_blocking();
        open_finding_ids.push(finding.id.clone());
    }
    open_finding_ids.sort();
    let mut resolution_ids = rows
        .resolutions
        .iter()
        .map(|resolution| resolution.id.clone())
        .collect::<Vec<_>>();
    resolution_ids.sort();

    let blocked = first_checkpoint(rows, &[ReviewCheckpointState::Blocked]);
    let active_checkpoint_id = first_checkpoint(
        rows,
        &[
            ReviewCheckpointState::Pending,
            ReviewCheckpointState::InReview,
        ],
    )
    .map(|checkpoint| checkpoint.id.clone());

    let (status, reason_code) = match rows.policy.as_ref() {
        // A Task without a policy has no coverage condition.
        None => (ReviewCoverageStatus::NotRequired, None),
        Some(policy) => {
            if base.is_none() {
                (ReviewCoverageStatus::Blocked, Some(BASE_UNKNOWN.to_owned()))
            } else if anchored.is_none() {
                (
                    ReviewCoverageStatus::Blocked,
                    Some(BASE_NOT_ANCESTOR.to_owned()),
                )
            } else if let Some(checkpoint) = blocked {
                (
                    ReviewCoverageStatus::Blocked,
                    checkpoint.blocked_code.clone(),
                )
            } else if !facts.base_tree.is_empty() && facts.base_tree == facts.candidate_tree {
                (ReviewCoverageStatus::Unchanged, None)
            } else if active_checkpoint_id.is_some() {
                (ReviewCoverageStatus::InReview, None)
            } else if blocking_open || !rows.failing_result_lines.is_empty() {
                (ReviewCoverageStatus::FindingsOpen, None)
            } else if !policy.policy.coverage_required {
                (ReviewCoverageStatus::NotRequired, None)
            } else if covered {
                (ReviewCoverageStatus::Satisfied, None)
            } else if history_rewritten {
                (ReviewCoverageStatus::HistoryRewritten, None)
            } else if frontier != base {
                (ReviewCoverageStatus::Partial, None)
            } else {
                (ReviewCoverageStatus::Uncovered, None)
            }
        }
    };

    let snapshot_digest = canonical_digest(&CanonicalSnapshot {
        repo_id,
        review_policy_digest: rows.policy.as_ref().map(|policy| policy.digest.as_str()),
        base_commit: base,
        candidate_commit: candidate,
        ranges: &ranges,
        open_finding_ids: &open_finding_ids,
        resolution_ids: &resolution_ids,
        active_checkpoint_id: active_checkpoint_id.as_deref(),
        status,
    })
    .expect("a coverage snapshot of strings and booleans always serializes");

    RepoCoverage {
        repo_id: repo_id.to_owned(),
        status,
        reason_code,
        revision: rows.state_revision,
        base_commit: base.map(str::to_owned),
        candidate_commit: candidate.map(str::to_owned),
        frontier_commit: frontier.map(str::to_owned),
        unreviewed_commits,
        ranges,
        open_findings,
        open_finding_ids,
        resolution_ids,
        active_checkpoint_id,
        snapshot_digest,
    }
}

/// The oldest checkpoint in one of the wanted states.
fn first_checkpoint<'a>(
    rows: &'a ReviewCoverageRows,
    wanted: &[ReviewCheckpointState],
) -> Option<&'a ReviewCheckpoint> {
    rows.checkpoints
        .iter()
        .filter(|checkpoint| wanted.contains(&checkpoint.state))
        .min_by(|left, right| (&left.created_at, &left.id).cmp(&(&right.created_at, &right.id)))
}

struct RangeReview {
    quorum: bool,
    report_artifact_ids: Vec<String>,
}

/// The `pass` and `request_changes` reports of a range, or `None` without any.
/// A `questions` report reviews nothing.
fn review_of(rows: &ReviewCoverageRows, range: &ReviewRange) -> Option<RangeReview> {
    let policy = rows.policy.as_ref()?;
    let reports = rows
        .reports
        .iter()
        .filter(|report| report.range_id == range.id && report.verdict != ReviewVerdict::Questions)
        .collect::<Vec<_>>();
    if reports.is_empty() {
        return None;
    }
    let reviewers = reports
        .iter()
        .map(|report| (report.actor.kind() == ActorKind::Human, report.actor.id()))
        .collect::<BTreeSet<_>>();
    let quorum = reviewers.len() >= policy.policy.required_reviewers.max(1) as usize
        && (!policy.policy.require_human || reviewers.iter().any(|(human, _)| *human));
    let mut report_artifact_ids = reports
        .iter()
        .map(|report| report.report_artifact_id.clone())
        .collect::<Vec<_>>();
    report_artifact_ids.sort();
    Some(RangeReview {
        quorum,
        report_artifact_ids,
    })
}

/// The node reachable from `base` over valid edges with the fewest commits left
/// to the candidate; ties are broken by SHA.
fn frontier<'a>(base: &'a str, ranges: &'a [CoveredRange], facts: &CommitFacts) -> &'a str {
    let mut reachable = BTreeSet::from([base]);
    loop {
        let before = reachable.len();
        for range in ranges.iter().filter(|range| range.valid_edge) {
            if reachable.contains(range.base_commit.as_str()) {
                reachable.insert(range.head_commit.as_str());
            }
        }
        if reachable.len() == before {
            break;
        }
    }
    reachable
        .into_iter()
        .min_by_key(|node| (facts.distance(node), *node))
        .unwrap_or(base)
}

/// The commit after the latest successful integration of the repository.
fn latest_integrated_head(rows: &ReviewCoverageRows) -> Option<&str> {
    rows.integrations
        .iter()
        .filter(|row| row.outcome == WorkUnitRepositoryIntegrationOutcome::Success)
        .filter_map(|row| {
            row.target_after_sha
                .as_deref()
                .map(|after| ((&row.started_at, &row.id), after))
        })
        .max_by_key(|(order, _)| *order)
        .map(|(_, after)| after)
}

pub struct ReviewCoverageService {
    db: Arc<SqliteDb>,
    workspace_root: PathBuf,
}

impl ReviewCoverageService {
    pub fn new(db: Arc<SqliteDb>, workspace_root: PathBuf) -> Self {
        Self { db, workspace_root }
    }

    /// Coverage of one repository for `candidate`, or for the latest integrated
    /// head (the base when nothing was integrated).
    pub async fn compute(
        &self,
        task_id: &str,
        repo_id: &str,
        candidate: Option<&str>,
    ) -> Result<RepoCoverage> {
        let rows = self.db.load_review_coverage_rows(task_id, repo_id).await?;
        let candidate = candidate
            .or_else(|| latest_integrated_head(&rows))
            .or_else(|| rows.base.as_ref().map(|base| base.base_commit.as_str()))
            .unwrap_or_default();
        let facts = self.facts(repo_id, &rows, candidate).await?;
        Ok(evaluate_repo(repo_id, &rows, &facts))
    }

    /// Every writable repository with a successful integration, by `repo_id`.
    pub async fn compute_task(&self, task_id: &str) -> Result<Vec<RepoCoverage>> {
        let mut repo_ids = TaskRepo::list_repositories(&*self.db, task_id)
            .await?
            .into_iter()
            .filter(|member| {
                matches!(
                    member.role,
                    TaskRepositoryRole::Primary | TaskRepositoryRole::Affected
                )
            })
            .map(|member| member.repo_id)
            .collect::<Vec<_>>();
        repo_ids.sort();
        let mut coverage = Vec::new();
        for repo_id in repo_ids {
            let rows = self.db.load_review_coverage_rows(task_id, &repo_id).await?;
            let Some(candidate) = latest_integrated_head(&rows) else {
                continue;
            };
            let facts = self.facts(&repo_id, &rows, candidate).await?;
            coverage.push(evaluate_repo(&repo_id, &rows, &facts));
        }
        Ok(coverage)
    }

    /// Computes git facts outside any transaction.
    pub async fn prepare(
        &self,
        task_id: &str,
        candidates: &[RepoCandidate],
    ) -> Result<CoverageFacts> {
        let mut repos = BTreeMap::new();
        for candidate in candidates {
            let rows = self
                .db
                .load_review_coverage_rows(task_id, &candidate.repo_id)
                .await?;
            let facts = self
                .facts(&candidate.repo_id, &rows, &candidate.commit_sha)
                .await?;
            repos.insert(candidate.repo_id.clone(), (rows.state_revision, facts));
        }
        Ok(CoverageFacts {
            task_id: task_id.to_owned(),
            repos,
        })
    }

    /// Re-reads the rows inside the caller's transaction and rejects facts
    /// prepared against an older coverage revision.
    pub async fn snapshot_in_tx(
        &self,
        conn: &mut SqliteConnection,
        facts: &CoverageFacts,
    ) -> Result<CoverageSnapshot> {
        let stale = |message: &str| ServiceError::Coordination {
            code: "review.coverage.facts_stale",
            message: message.to_owned(),
        };
        let mut policy = None;
        let mut repos = Vec::with_capacity(facts.repos.len());
        for (repo_id, (revision, commit_facts)) in &facts.repos {
            let rows = self
                .db
                .load_review_coverage_rows_in_tx(conn, &facts.task_id, repo_id)
                .await?;
            if rows.state_revision != *revision {
                return Err(stale(
                    "review coverage changed after its facts were prepared",
                ));
            }
            repos.push(evaluate_repo(repo_id, &rows, commit_facts));
            policy = rows.policy;
        }
        if repos.is_empty() {
            policy = self
                .db
                .load_review_coverage_rows_in_tx(conn, &facts.task_id, "")
                .await?
                .policy;
        }
        let policy = policy.ok_or_else(|| stale("the Task has no current review policy"))?;
        Ok(CoverageSnapshot {
            policy: ReviewPolicyRef {
                revision: policy.revision,
                digest: policy.digest,
            },
            repos,
        })
    }

    /// Only the ancestry the evaluator asks for: each range base against its
    /// head, and the base, heads and anchors against the candidate.
    async fn facts(
        &self,
        repo_id: &str,
        rows: &ReviewCoverageRows,
        candidate: &str,
    ) -> Result<CommitFacts> {
        let mut facts = CommitFacts {
            candidate: candidate.to_owned(),
            ..CommitFacts::default()
        };
        let Some(base) = rows.base.as_ref() else {
            return Ok(facts);
        };
        if candidate.is_empty() {
            return Ok(facts);
        }
        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
        let source = resolve_repo_source(&repo, &self.workspace_root).await?;
        collect_facts(Path::new(&source), &base.base_commit, rows, &mut facts).await?;
        Ok(facts)
    }
}

/// A commit that no longer exists (rewritten and collected) has no facts.
async fn collect_facts(
    repo_path: &Path,
    base: &str,
    rows: &ReviewCoverageRows,
    facts: &mut CommitFacts,
) -> Result<()> {
    let candidate = facts.candidate.clone();
    let mut pairs = BTreeSet::new();
    pairs.insert((base, candidate.as_str()));
    for range in &rows.ranges {
        pairs.insert((range.base_commit.as_str(), range.head_commit.as_str()));
        pairs.insert((range.head_commit.as_str(), candidate.as_str()));
    }
    for resolution in &rows.resolutions {
        pairs.insert((resolution.anchor_commit.as_str(), candidate.as_str()));
    }
    let mut known = BTreeMap::new();
    let mut commits = BTreeSet::new();
    for (ancestor, descendant) in &pairs {
        commits.extend([*ancestor, *descendant]);
    }
    for commit in commits {
        known.insert(commit, git::commit_exists(repo_path, commit).await?);
    }
    for (ancestor, descendant) in pairs {
        if !known[ancestor]
            || !known[descendant]
            || !git::is_ancestor(repo_path, ancestor, descendant).await?
        {
            continue;
        }
        facts
            .ancestors_of
            .entry(descendant.to_owned())
            .or_default()
            .insert(ancestor.to_owned());
        if descendant == candidate {
            let distance = git::count_commits_between(repo_path, ancestor, descendant).await?;
            facts.distance_to_candidate.insert(
                ancestor.to_owned(),
                u32::try_from(distance).unwrap_or(u32::MAX),
            );
        }
    }
    if known[base] {
        facts.base_tree = git::resolve_tree(repo_path, base).await?;
    }
    if known[candidate.as_str()] {
        facts.candidate_tree = git::resolve_tree(repo_path, &candidate).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
