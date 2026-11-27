//! The context handed to an Agent commit-range reviewer.

use std::{fmt::Write as _, path::Path};

use db::{
    ReviewCheckpointRepo, ReviewFinding, ReviewFindingLocation, ReviewLedgerRepo, ReviewPolicyRepo,
    ReviewRange, ReviewRangeContributor, ReviewRiskClass, SqliteDb, WorkUnitReviewRange,
    WorkUnitWorkspaceRepo,
};

use crate::{work_unit_service::range_review, Result};

/// Files listed individually; the totals and the command for the rest are
/// always stated, so a long diffstat is never silently truncated.
const MAX_DIFFSTAT_FILES: usize = 400;

struct RangePromptInput<'a> {
    range: &'a ReviewRange,
    repository_path: &'a str,
    recheck: bool,
    external_provider_contrast: bool,
    contributors: &'a [ReviewRangeContributor],
    diffstat: &'a [git::FileDiffStat],
    findings: &'a [(ReviewFinding, Vec<ReviewFindingLocation>)],
}

/// Build the context from the frozen range, its checkout and the ledger.
pub(super) async fn range_review_context(
    db: &SqliteDb,
    frozen: &WorkUnitReviewRange,
) -> Result<String> {
    let range = ReviewCheckpointRepo::get_review_range(db, &frozen.range_id)
        .await?
        .filter(|range| range.task_id == frozen.task_id && range.repo_id == frozen.repo_id)
        .ok_or_else(|| range_review::mismatch("review range is unavailable"))?;
    let checkpoint = ReviewCheckpointRepo::get_review_checkpoint(db, &frozen.checkpoint_id)
        .await?
        .ok_or_else(|| range_review::mismatch("review checkpoint is unavailable"))?;
    let workspace = WorkUnitWorkspaceRepo::list_by_work_unit_id(db, &frozen.review_work_unit_id)
        .await?
        .into_iter()
        .find(|workspace| workspace.task_id == frozen.task_id && workspace.repo_id == range.repo_id)
        .ok_or_else(|| range_review::mismatch("range review checkout is not prepared"))?;
    let diffstat = git::diffstat(
        Path::new(&workspace.worktree_path),
        &range.base_commit,
        &range.head_commit,
    )
    .await?;
    let contributors = ReviewCheckpointRepo::list_review_range_contributors(db, &range.id).await?;
    let mut findings = Vec::with_capacity(frozen.input_findings.len());
    for input in &frozen.input_findings {
        let finding = ReviewLedgerRepo::get_review_finding(db, &input.finding_id)
            .await?
            .filter(|finding| finding.digest == input.digest)
            .ok_or_else(|| {
                range_review::mismatch("a frozen input finding no longer matches the ledger")
            })?;
        let locations = ReviewLedgerRepo::list_review_finding_locations(db, &finding.id).await?;
        findings.push((finding, locations));
    }
    let external_provider_contrast =
        ReviewPolicyRepo::get_current_task_review_policy(db, &frozen.task_id)
            .await?
            .is_some_and(|policy| policy.policy.risk_class == ReviewRiskClass::R2);
    Ok(render(&RangePromptInput {
        range: &range,
        repository_path: &workspace.worktree_path,
        recheck: checkpoint
            .reasons
            .contains(&db::ReviewCheckpointReason::Recheck),
        external_provider_contrast,
        contributors: &contributors,
        diffstat: &diffstat,
        findings: &findings,
    }))
}

fn render(input: &RangePromptInput<'_>) -> String {
    let range = input.range;
    let mut prompt = String::from(
        "You are performing formal independent review of an exact commit range. Do not edit files, create commits, implement fixes, or contact external providers. Every repository in this workspace is read-only, and any write fails the review. Do not reuse a session that produced any of these commits.\n\n",
    );
    // Writing to a String cannot fail.
    let _ = writeln!(
        prompt,
        "Range under review: repository `{}`, base `{}`, head `{}` (range digest `{}`).\nCheckout (detached at head): `{}`. Context repositories listed in `FORGE_WORKSPACE.md` are pinned references, not part of the range.\nRead the complete diff yourself with `git diff {}..{}` and `git log {}..{}`; it is not included here.",
        range.repo_id,
        range.base_commit,
        range.head_commit,
        range.range_digest,
        input.repository_path,
        range.base_commit,
        range.head_commit,
        range.base_commit,
        range.head_commit,
    );

    let (added, deleted) = input
        .diffstat
        .iter()
        .fold((0_u64, 0_u64), |(added, deleted), file| {
            (
                added.saturating_add(file.added_lines),
                deleted.saturating_add(file.deleted_lines),
            )
        });
    let _ = writeln!(
        prompt,
        "\nDiffstat: {} files, {added} lines added, {deleted} lines deleted.",
        input.diffstat.len()
    );
    for file in input.diffstat.iter().take(MAX_DIFFSTAT_FILES) {
        if file.binary {
            let _ = writeln!(prompt, "- {:?} (binary)", file.path);
        } else {
            let _ = writeln!(
                prompt,
                "- {:?} +{} -{}",
                file.path, file.added_lines, file.deleted_lines
            );
        }
    }
    if input.diffstat.len() > MAX_DIFFSTAT_FILES {
        let _ = writeln!(
            prompt,
            "- {} further files are NOT listed here; list all of them with `git diff --numstat {}..{}` and review them too.",
            input.diffstat.len() - MAX_DIFFSTAT_FILES,
            range.base_commit,
            range.head_commit,
        );
    }

    prompt.push_str("\nContributors to this range (they are not independent reviewers of it):\n");
    let mut contributors = input
        .contributors
        .iter()
        .map(|contributor| {
            format!(
                "- {} `{}`{}",
                contributor.actor.kind(),
                contributor.actor.id(),
                contributor
                    .work_unit_id
                    .as_deref()
                    .map(|id| format!(" (WorkUnit {id})"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    contributors.sort();
    contributors.dedup();
    if contributors.is_empty() {
        prompt.push_str("- none recorded\n");
    } else {
        prompt.push_str(&contributors.join("\n"));
        prompt.push('\n');
    }

    prompt.push_str("\nOpen findings frozen as input to this review:\n");
    if input.findings.is_empty() {
        prompt.push_str("- none; `resolutions` must be an empty array\n");
    }
    for (finding, locations) in input.findings {
        let locations = locations
            .iter()
            .map(|location| match (location.line_start, location.line_end) {
                (Some(start), Some(end)) => format!("{}:{start}-{end}", location.path),
                _ => location.path.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            prompt,
            "- finding_id `{}` [{}] {} (anchor `{}`; locations: {})\n  Evidence: {}{}",
            finding.id,
            finding.severity,
            finding.title,
            finding.anchor_commit,
            if locations.is_empty() {
                "none"
            } else {
                locations.as_str()
            },
            finding.evidence,
            finding
                .invariant
                .as_deref()
                .map(|invariant| format!("\n  Invariant: {invariant}"))
                .unwrap_or_default(),
        );
    }

    prompt.push_str(
        "\nRequired criteria:\n\
         1. Review every change in the range for correctness, security and the Task scope.\n\
         2. Read the repository's `AGENTS.md` and `CLAUDE.md` at the head commit, and check every MUST (and MUST NOT) rule that applies to the changed paths. Each violation is a finding naming the rule in `invariant`.\n",
    );
    if input.external_provider_contrast {
        prompt.push_str(
            "3. This is an R2 review. For every integration with an external provider touched by the range, require contrast against recorded real provider responses (fixtures or recordings captured from the real provider) present as Task evidence or in the repository. Invented or mock-only responses do not satisfy this; its absence is a finding.\n",
        );
    }

    prompt.push_str(
        "\nSeverity: P0 blocks release, P1 blocks merge, P2 is fixed or accepted, P3 is backlog. `pass` admits no new P0/P1 and no `still_open` P0/P1 input. `fail` needs at least one new finding or one `still_open` input. `needs_human` needs nonempty `questions` and empty `findings` and `resolutions`; otherwise `questions` is empty. Every P0/P1 input finding needs exactly one resolution; resolutions name only the input findings above. `fixed` means the head fixes it. ",
    );
    if input.recheck {
        prompt.push_str(
            "This is a recheck: `not_reproduced` is allowed when independent inspection cannot reproduce an input finding. ",
        );
    } else {
        prompt.push_str("This is not a recheck: `not_reproduced` is not allowed. ");
    }
    prompt.push_str(
        "Finding paths are repository-relative and exist at the head (or were deleted by the range, without lines). At most 100 findings with 16 locations each; labels match [A-Za-z0-9._-]{1,32}; title at most 256 bytes, evidence at most 4096.\n\n\
         End your assistant output with exactly one final line in this contract, with no additional keys: FORGE_RESULT: {\"schema_version\":2,\"kind\":\"review\",\"verdict\":\"pass|fail|needs_human\",\"summary\":\"nonempty factual summary\",\"criteria\":[\"criterion checked\"],\"findings\":[{\"label\":\"F-1\",\"severity\":\"P1\",\"title\":\"defect\",\"evidence\":\"reproduction or proof\",\"invariant\":null,\"locations\":[{\"path\":\"src/file\",\"line_start\":1,\"line_end\":2}]}],\"resolutions\":[{\"finding_id\":\"...\",\"outcome\":\"fixed|not_reproduced|still_open\",\"evidence\":\"...\"}],\"questions\":[],\"evidence_considered\":[]}",
    );
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{ActorRef, ReviewFindingSeverity};

    fn range() -> ReviewRange {
        ReviewRange {
            id: "range".to_owned(),
            task_id: "task".to_owned(),
            repo_id: "repo".to_owned(),
            repository_set_revision: 1,
            base_commit: "basesha".to_owned(),
            head_commit: "headsha".to_owned(),
            base_tree: "base-tree".to_owned(),
            head_tree: "head-tree".to_owned(),
            range_digest: "digest".to_owned(),
            integration_workspace_id: "workspace".to_owned(),
            head_integration_id: "integration".to_owned(),
            created_at: "2026-11-27T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn review_range_prompt_states_subject_findings_and_criteria() {
        let range = range();
        let contributors = vec![ReviewRangeContributor {
            id: "contributor".to_owned(),
            task_id: "task".to_owned(),
            range_id: "range".to_owned(),
            work_unit_id: Some("work".to_owned()),
            execution_id: Some("execution".to_owned()),
            attribution_id: None,
            actor: ActorRef::Agent("producer".to_owned()),
            created_at: "2026-11-27T00:00:00Z".to_owned(),
        }];
        let diffstat = vec![git::FileDiffStat {
            path: "src/a.rs".to_owned(),
            added_lines: 4,
            deleted_lines: 1,
            binary: false,
        }];
        let findings = vec![(
            ReviewFinding {
                id: "finding-1".to_owned(),
                task_id: "task".to_owned(),
                repo_id: "repo".to_owned(),
                range_id: "earlier".to_owned(),
                report_artifact_id: "report".to_owned(),
                ordinal: 0,
                label: "F-1".to_owned(),
                severity: ReviewFindingSeverity::P1,
                title: "Missing scope check".to_owned(),
                evidence: "Cross-tenant read".to_owned(),
                invariant: None,
                anchor_commit: "oldsha".to_owned(),
                digest: "digest".to_owned(),
                created_at: "2026-11-27T00:00:00Z".to_owned(),
            },
            Vec::new(),
        )];
        let mut input = RangePromptInput {
            range: &range,
            repository_path: "/checkout",
            recheck: false,
            external_provider_contrast: true,
            contributors: &contributors,
            diffstat: &diffstat,
            findings: &findings,
        };
        let prompt = render(&input);
        for expected in [
            "git diff basesha..headsha",
            "1 files, 4 lines added, 1 lines deleted",
            "agent `producer` (WorkUnit work)",
            "finding_id `finding-1` [P1]",
            "recorded real provider responses",
            "MUST (and MUST NOT)",
            "\"schema_version\":2",
            "`not_reproduced` is not allowed",
        ] {
            assert!(prompt.contains(expected), "{expected}");
        }
        input.external_provider_contrast = false;
        input.recheck = true;
        let prompt = render(&input);
        assert!(!prompt.contains("recorded real provider responses"));
        assert!(prompt.contains("`not_reproduced` is allowed"));
    }

    #[test]
    fn review_range_prompt_declares_unlisted_files() {
        let range = range();
        let diffstat = (0..MAX_DIFFSTAT_FILES + 3)
            .map(|index| git::FileDiffStat {
                path: format!("file-{index}"),
                added_lines: 1,
                deleted_lines: 0,
                binary: false,
            })
            .collect::<Vec<_>>();
        let prompt = render(&RangePromptInput {
            range: &range,
            repository_path: "/checkout",
            recheck: false,
            external_provider_contrast: false,
            contributors: &[],
            diffstat: &diffstat,
            findings: &[],
        });
        assert!(prompt.contains("3 further files are NOT listed here"));
        assert!(prompt.contains(&format!("{} files", MAX_DIFFSTAT_FILES + 3)));
    }
}
