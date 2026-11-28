use super::*;
use db::{
    ActorRef, ReviewBaseSource, ReviewCheckpointReason, ReviewFinding, ReviewFindingResolution,
    ReviewFindingSeverity, ReviewPolicyDocument, ReviewPolicyRevisionState, ReviewPolicySource,
    ReviewRangeReport, ReviewResolutionOutcome, ReviewRiskClass, ReviewSizeThreshold,
    ReviewTriggerMode, TaskReviewBase, TaskReviewPolicyRevision,
};

const AT: &str = "2026-11-27T00:00:00Z";

/// Facts of a linear history, oldest first; the last commit is the candidate.
fn linear(commits: &[&str]) -> CommitFacts {
    let candidate = *commits.last().unwrap();
    let mut facts = CommitFacts {
        candidate: candidate.to_owned(),
        base_tree: format!("tree-{}", commits[0]),
        candidate_tree: format!("tree-{candidate}"),
        ..CommitFacts::default()
    };
    for (index, commit) in commits.iter().enumerate() {
        facts.ancestors_of.insert(
            (*commit).to_owned(),
            commits[..=index]
                .iter()
                .map(|ancestor| (*ancestor).to_owned())
                .collect(),
        );
        facts
            .distance_to_candidate
            .insert((*commit).to_owned(), (commits.len() - 1 - index) as u32);
    }
    facts
}

fn rows() -> ReviewCoverageRows {
    ReviewCoverageRows {
        policy: Some(TaskReviewPolicyRevision {
            task_id: "task".to_owned(),
            revision: 1,
            state: ReviewPolicyRevisionState::Current,
            policy: ReviewPolicyDocument {
                schema_version: 1,
                risk_class: ReviewRiskClass::R1,
                coverage_required: true,
                trigger_mode: ReviewTriggerMode::Auto,
                reviewer_role: "reviewer".to_owned(),
                required_reviewers: 1,
                require_human: false,
                size_threshold: ReviewSizeThreshold {
                    changed_lines: 800,
                    changed_files: 25,
                },
                coordinator_may_request: true,
                max_checkpoints_per_repo: 12,
            },
            digest: "d".repeat(64),
            source: ReviewPolicySource::Owner,
            created_by_user_id: "owner".to_owned(),
            created_at: AT.to_owned(),
            superseded_at: None,
        }),
        base: Some(TaskReviewBase {
            task_id: "task".to_owned(),
            repo_id: "repo".to_owned(),
            revision: 1,
            base_commit: "b".to_owned(),
            base_tree: "tree-b".to_owned(),
            source: ReviewBaseSource::WorkspacePrepare,
            decision_id: None,
            pinned_at: AT.to_owned(),
        }),
        state_revision: 7,
        integrations: Vec::new(),
        ranges: Vec::new(),
        checkpoints: Vec::new(),
        reports: Vec::new(),
        findings: Vec::new(),
        resolutions: Vec::new(),
        attributions: Vec::new(),
        failing_result_lines: Vec::new(),
    }
}

fn reviewed(
    rows: &mut ReviewCoverageRows,
    id: &str,
    base: &str,
    head: &str,
    verdict: ReviewVerdict,
) {
    rows.ranges.push(ReviewRange {
        id: id.to_owned(),
        task_id: "task".to_owned(),
        repo_id: "repo".to_owned(),
        repository_set_revision: 1,
        base_commit: base.to_owned(),
        head_commit: head.to_owned(),
        base_tree: format!("tree-{base}"),
        head_tree: format!("tree-{head}"),
        range_digest: "r".repeat(64),
        integration_workspace_id: "workspace".to_owned(),
        head_integration_id: "integration".to_owned(),
        created_at: AT.to_owned(),
    });
    rows.reports.push(ReviewRangeReport {
        report_artifact_id: format!("report-{id}"),
        task_id: "task".to_owned(),
        repo_id: "repo".to_owned(),
        range_id: id.to_owned(),
        checkpoint_id: format!("checkpoint-{id}"),
        review_work_unit_id: format!("work-{id}"),
        execution_id: format!("execution-{id}"),
        actor: ActorRef::Agent("reviewer-agent".to_owned()),
        verdict,
        report_digest: "p".repeat(64),
        created_at: AT.to_owned(),
    });
}

fn finding(rows: &mut ReviewCoverageRows, id: &str, severity: ReviewFindingSeverity, anchor: &str) {
    rows.findings.push(ReviewFinding {
        id: id.to_owned(),
        task_id: "task".to_owned(),
        repo_id: "repo".to_owned(),
        range_id: "range".to_owned(),
        report_artifact_id: "report".to_owned(),
        ordinal: 0,
        label: "F-1".to_owned(),
        severity,
        title: "title".to_owned(),
        evidence: "evidence".to_owned(),
        invariant: None,
        anchor_commit: anchor.to_owned(),
        digest: "f".repeat(64),
        created_at: AT.to_owned(),
    });
}

fn resolve(rows: &mut ReviewCoverageRows, finding_id: &str, anchor: &str) {
    rows.resolutions.push(ReviewFindingResolution {
        id: format!("resolution-{finding_id}"),
        finding_id: finding_id.to_owned(),
        task_id: "task".to_owned(),
        repo_id: "repo".to_owned(),
        outcome: ReviewResolutionOutcome::Fixed,
        anchor_commit: anchor.to_owned(),
        report_artifact_id: Some("report".to_owned()),
        decision_id: None,
        waiver_reference: None,
        evidence: "evidence".to_owned(),
        actor: ActorRef::Agent("reviewer-agent".to_owned()),
        created_at: AT.to_owned(),
    });
}

fn checkpoint(rows: &mut ReviewCoverageRows, id: &str, state: ReviewCheckpointState) {
    rows.checkpoints.push(ReviewCheckpoint {
        id: id.to_owned(),
        task_id: "task".to_owned(),
        repo_id: "repo".to_owned(),
        state,
        reasons: vec![ReviewCheckpointReason::Baseline],
        attempt: 1,
        base_commit: None,
        head_commit: "c".to_owned(),
        range_id: None,
        policy_revision: 1,
        blocked_code: (state == ReviewCheckpointState::Blocked)
            .then(|| "review.range.unrecorded_commits".to_owned()),
        blocked_detail_json: None,
        version: 1,
        created_at: AT.to_owned(),
        updated_at: AT.to_owned(),
    });
}

#[test]
fn review_range_coverage_chains_baseline_and_delta() {
    let facts = linear(&["b", "h", "c"]);
    let mut rows = rows();
    let uncovered = evaluate_coverage(&rows, &facts);
    assert_eq!(uncovered.status, ReviewCoverageStatus::Uncovered);
    assert_eq!(uncovered.frontier_commit.as_deref(), Some("b"));
    assert_eq!(uncovered.unreviewed_commits, 2);
    assert_eq!(uncovered.revision, 7);

    reviewed(&mut rows, "1", "b", "h", ReviewVerdict::RequestChanges);
    let partial = evaluate_coverage(&rows, &facts);
    assert_eq!(partial.status, ReviewCoverageStatus::Partial);
    assert_eq!(partial.frontier_commit.as_deref(), Some("h"));
    assert_eq!(partial.unreviewed_commits, 1);

    reviewed(&mut rows, "2", "h", "c", ReviewVerdict::Pass);
    let satisfied = evaluate_coverage(&rows, &facts);
    assert_eq!(satisfied.status, ReviewCoverageStatus::Satisfied);
    assert_eq!(satisfied.repo_id, "repo");
    assert_eq!(satisfied.unreviewed_commits, 0);
    assert!(satisfied.ranges.iter().all(|range| range.valid_edge));
    assert_ne!(satisfied.snapshot_digest, partial.snapshot_digest);
    assert_eq!(
        satisfied.snapshot_digest,
        evaluate_coverage(&rows, &facts).snapshot_digest
    );
}

#[test]
fn review_range_coverage_requires_a_covered_boundary() {
    let facts = linear(&["b", "h", "c"]);
    let mut rows = rows();
    reviewed(&mut rows, "1", "h", "c", ReviewVerdict::Pass);

    let coverage = evaluate_coverage(&rows, &facts);
    assert_eq!(coverage.status, ReviewCoverageStatus::Uncovered);
    assert!(coverage.ranges[0].valid_edge);
}

#[test]
fn review_range_coverage_ignores_question_reports() {
    let facts = linear(&["b", "c"]);
    let mut rows = rows();
    reviewed(&mut rows, "1", "b", "c", ReviewVerdict::Questions);

    let coverage = evaluate_coverage(&rows, &facts);
    assert_eq!(coverage.status, ReviewCoverageStatus::Uncovered);
    assert!(coverage.ranges.is_empty());
}

#[test]
fn review_range_coverage_requires_a_human_when_the_policy_does() {
    let facts = linear(&["b", "c"]);
    let mut rows = rows();
    rows.policy.as_mut().unwrap().policy.require_human = true;
    reviewed(&mut rows, "1", "b", "c", ReviewVerdict::Pass);
    assert_eq!(
        evaluate_coverage(&rows, &facts).status,
        ReviewCoverageStatus::Uncovered
    );

    rows.reports[0].actor = ActorRef::Human("human".to_owned());
    assert_eq!(
        evaluate_coverage(&rows, &facts).status,
        ReviewCoverageStatus::Satisfied
    );
}

#[test]
fn review_range_coverage_reports_rewritten_history() {
    // `old` was reviewed and is no longer part of the candidate.
    let facts = linear(&["b", "c"]);
    let mut rows = rows();
    reviewed(&mut rows, "1", "b", "old", ReviewVerdict::Pass);

    let coverage = evaluate_coverage(&rows, &facts);
    assert_eq!(coverage.status, ReviewCoverageStatus::HistoryRewritten);
    assert!(!coverage.ranges[0].valid_edge);
}

#[test]
fn review_range_coverage_blocks_unknown_or_detached_base() {
    let facts = linear(&["b", "c"]);
    let mut unknown = rows();
    unknown.base = None;
    let coverage = evaluate_coverage(&unknown, &facts);
    assert_eq!(coverage.status, ReviewCoverageStatus::Blocked);
    assert_eq!(coverage.reason_code.as_deref(), Some(BASE_UNKNOWN));

    // Missing facts never count as ancestry.
    let detached = CommitFacts {
        candidate: "c".to_owned(),
        ..CommitFacts::default()
    };
    let coverage = evaluate_coverage(&rows(), &detached);
    assert_eq!(coverage.status, ReviewCoverageStatus::Blocked);
    assert_eq!(coverage.reason_code.as_deref(), Some(BASE_NOT_ANCESTOR));
    assert_eq!(coverage.frontier_commit, None);
}

#[test]
fn review_range_coverage_keeps_findings_open_until_resolved_in_the_candidate() {
    let facts = linear(&["b", "h", "c"]);
    let mut rows = rows();
    reviewed(&mut rows, "1", "b", "c", ReviewVerdict::RequestChanges);
    finding(&mut rows, "p1", ReviewFindingSeverity::P1, "h");
    finding(&mut rows, "p2", ReviewFindingSeverity::P2, "h");

    let open = evaluate_coverage(&rows, &facts);
    assert_eq!(open.status, ReviewCoverageStatus::FindingsOpen);
    assert_eq!(open.open_finding_ids, vec!["p1", "p2"]);
    assert_eq!((open.open_findings.p1, open.open_findings.p2), (1, 1));

    // A fix lost by rewriting history leaves the finding open.
    resolve(&mut rows, "p1", "lost");
    assert_eq!(
        evaluate_coverage(&rows, &facts).status,
        ReviewCoverageStatus::FindingsOpen
    );

    rows.resolutions[0].anchor_commit = "c".to_owned();
    let closed = evaluate_coverage(&rows, &facts);
    assert_eq!(closed.status, ReviewCoverageStatus::Satisfied);
    assert_eq!(closed.open_finding_ids, vec!["p2"]);
}

#[test]
fn review_range_coverage_applies_status_precedence() {
    let facts = linear(&["b", "c"]);
    let mut rows = rows();
    rows.policy.as_mut().unwrap().policy.coverage_required = false;
    assert_eq!(
        evaluate_coverage(&rows, &facts).status,
        ReviewCoverageStatus::NotRequired
    );

    rows.failing_result_lines.push("line".to_owned());
    assert_eq!(
        evaluate_coverage(&rows, &facts).status,
        ReviewCoverageStatus::FindingsOpen
    );

    checkpoint(&mut rows, "active", ReviewCheckpointState::InReview);
    let in_review = evaluate_coverage(&rows, &facts);
    assert_eq!(in_review.status, ReviewCoverageStatus::InReview);
    assert_eq!(in_review.active_checkpoint_id.as_deref(), Some("active"));

    let mut unchanged = facts.clone();
    unchanged.candidate_tree = unchanged.base_tree.clone();
    assert_eq!(
        evaluate_coverage(&rows, &unchanged).status,
        ReviewCoverageStatus::Unchanged
    );

    checkpoint(&mut rows, "blocked", ReviewCheckpointState::Blocked);
    let blocked = evaluate_coverage(&rows, &unchanged);
    assert_eq!(blocked.status, ReviewCoverageStatus::Blocked);
    assert_eq!(
        blocked.reason_code.as_deref(),
        Some("review.range.unrecorded_commits")
    );
}

#[test]
fn review_range_unchanged_tree_with_open_p0_or_p1_finding_blocks_gate_coverage() {
    let mut unchanged = linear(&["b", "c"]);
    unchanged.candidate_tree = unchanged.base_tree.clone();

    for coverage_required in [true, false] {
        for (id, severity) in [
            ("open-p0", ReviewFindingSeverity::P0),
            ("open-p1", ReviewFindingSeverity::P1),
        ] {
            let mut rows = rows();
            rows.policy.as_mut().unwrap().policy.coverage_required = coverage_required;
            finding(&mut rows, id, severity, "b");
            let coverage = evaluate_coverage(&rows, &unchanged);
            assert_eq!(coverage.status, ReviewCoverageStatus::FindingsOpen);
            assert_eq!(
                coverage.gate_status(),
                api_types::ReviewCoverageStatus::FindingsOpen
            );
        }
    }
}

#[test]
fn review_range_coverage_is_not_required_without_a_policy() {
    let mut rows = rows();
    rows.policy = None;

    assert_eq!(
        evaluate_coverage(&rows, &linear(&["b", "c"])).status,
        ReviewCoverageStatus::NotRequired
    );
}

#[test]
fn review_range_coverage_gate_evidence_preserves_revision_status_and_subject() {
    let snapshot = CoverageSnapshot {
        policy: ReviewPolicyRef {
            revision: 4,
            digest: "p".repeat(64),
        },
        repos: vec![RepoCoverage {
            repo_id: "repo".to_owned(),
            status: ReviewCoverageStatus::Satisfied,
            reason_code: None,
            revision: 17,
            base_commit: Some("base".to_owned()),
            candidate_commit: Some("head".to_owned()),
            frontier_commit: Some("head".to_owned()),
            unreviewed_commits: 0,
            ranges: Vec::new(),
            open_findings: SeverityCounts::default(),
            open_finding_ids: Vec::new(),
            resolution_ids: Vec::new(),
            active_checkpoint_id: None,
            snapshot_digest: "s".repeat(64),
        }],
    };

    assert_eq!(
        snapshot.gate_evidence(),
        crate::gate_engine::ReviewCoverageEvidence {
            policy: ReviewPolicyRef {
                revision: 4,
                digest: "p".repeat(64),
            },
            repositories: vec![crate::gate_engine::RepositoryCoverageSubject {
                repo_id: "repo".to_owned(),
                revision: 17,
                status: api_types::ReviewCoverageStatus::Satisfied,
                snapshot_digest: "s".repeat(64),
                base_commit: Some("base".to_owned()),
                candidate_commit: Some("head".to_owned()),
            }],
        }
    );
}

#[tokio::test]
async fn review_range_coverage_collects_git_facts() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git::init(repo).await.unwrap();
    std::fs::write(repo.join("a.txt"), "base").unwrap();
    let base = git::commit_all(repo, "base").await.unwrap();
    std::fs::write(repo.join("a.txt"), "head").unwrap();
    let head = git::commit_all(repo, "head").await.unwrap();
    std::fs::write(repo.join("a.txt"), "candidate").unwrap();
    let candidate = git::commit_all(repo, "candidate").await.unwrap();

    let mut rows = rows();
    rows.base.as_mut().unwrap().base_commit = base.clone();
    reviewed(&mut rows, "1", &base, &head, ReviewVerdict::Pass);
    reviewed(&mut rows, "2", &head, &candidate, ReviewVerdict::Pass);
    reviewed(&mut rows, "3", &head, &"0".repeat(40), ReviewVerdict::Pass);
    let mut facts = CommitFacts {
        candidate: candidate.clone(),
        ..CommitFacts::default()
    };
    collect_facts(repo, &base, &rows, &mut facts).await.unwrap();

    assert_eq!(facts.distance_to_candidate[&base], 2);
    assert_eq!(facts.distance_to_candidate[&head], 1);
    assert_ne!(facts.base_tree, facts.candidate_tree);
    let coverage = evaluate_coverage(&rows, &facts);
    assert_eq!(coverage.status, ReviewCoverageStatus::Satisfied);
    assert_eq!(
        coverage
            .ranges
            .iter()
            .map(|range| range.valid_edge)
            .collect::<Vec<_>>(),
        vec![true, true, false]
    );
}
