//! Deterministic attribution of range commits and finding locations to recorded
//! integrations. Git author metadata is informational and never grants Actor
//! authority.

use crate::{task_service::workspace::resolve_repo_source, Result, ServiceError};
use db::{
    new_uuid_v4, ActorKind, ActorRef, RepoRepo, ReviewAttributionKind, ReviewCoverageRows,
    ReviewLedgerRepo, ReviewLocationAttribution, ReviewRange, ReviewRangeContributor, SqliteDb,
    WorkUnitRepositoryIntegration, WorkUnitRepositoryIntegrationOutcome,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingLocation {
    pub path: String,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeContributors {
    /// `range_id` is empty until the caller freezes the generated range ID.
    pub contributors: Vec<ReviewRangeContributor>,
    pub unrecorded_commits: Vec<String>,
    pub upstream_commits: Vec<String>,
}

#[derive(Clone)]
struct CommitSource {
    location: ReviewLocationAttribution,
    integration_order: Option<(String, String)>,
}

struct AttributionFacts {
    sources: BTreeMap<String, CommitSource>,
    contributors: Vec<ReviewRangeContributor>,
    unrecorded_commits: Vec<String>,
    upstream_commits: Vec<String>,
    repo_path: PathBuf,
    default_branch: String,
}

pub struct ReviewAttribution {
    db: Arc<SqliteDb>,
    workspace_root: PathBuf,
}

impl ReviewAttribution {
    pub fn new(db: Arc<SqliteDb>, workspace_root: PathBuf) -> Self {
        Self { db, workspace_root }
    }

    /// Attribute every requested location from its lines at the frozen range
    /// head. Deleted paths and locations without line ranges remain explicitly
    /// unattributed.
    pub async fn attribute(
        &self,
        range: &ReviewRange,
        locations: &[FindingLocation],
    ) -> Result<Vec<ReviewLocationAttribution>> {
        let facts = self
            .attribution_facts(
                &range.task_id,
                &range.repo_id,
                &range.base_commit,
                &range.head_commit,
            )
            .await?;
        let mut attributed = Vec::with_capacity(locations.len());
        for location in locations {
            let (Some(start), Some(end)) = (location.line_start, location.line_end) else {
                attributed.push(unattributed(None, 0));
                continue;
            };
            let Ok(start) = u32::try_from(start) else {
                attributed.push(unattributed(None, 0));
                continue;
            };
            let Ok(end) = u32::try_from(end) else {
                attributed.push(unattributed(None, 0));
                continue;
            };
            if !git::path_exists_at(
                Path::new(&facts.repo_path),
                &range.head_commit,
                &location.path,
            )
            .await?
            {
                attributed.push(unattributed(None, 0));
                continue;
            }
            let blamed = git::blame_lines(
                Path::new(&facts.repo_path),
                &range.head_commit,
                &location.path,
                start,
                end,
            )
            .await?;
            let mut line_counts = BTreeMap::<String, i64>::new();
            for line in blamed {
                *line_counts.entry(line.commit).or_default() += 1;
            }
            if line_counts.is_empty() {
                attributed.push(unattributed(None, 0));
                continue;
            }
            let mut choices = Vec::with_capacity(line_counts.len());
            for (commit, line_count) in line_counts {
                let source = if let Some(source) = facts.sources.get(&commit) {
                    source.clone()
                } else if self
                    .is_ancestor_if_present(&facts, &commit, &facts.default_branch)
                    .await?
                {
                    CommitSource {
                        location: ReviewLocationAttribution {
                            kind: ReviewAttributionKind::Upstream,
                            work_unit_id: None,
                            execution_id: None,
                            actor: None,
                            commit_sha: Some(commit.clone()),
                            integration_id: None,
                            line_count,
                        },
                        integration_order: None,
                    }
                } else if git::commit_exists(Path::new(&facts.repo_path), &commit).await?
                    && git::is_ancestor(Path::new(&facts.repo_path), &commit, &range.base_commit)
                        .await?
                {
                    CommitSource {
                        location: ReviewLocationAttribution {
                            kind: ReviewAttributionKind::Preexisting,
                            work_unit_id: None,
                            execution_id: None,
                            actor: None,
                            commit_sha: Some(commit.clone()),
                            integration_id: None,
                            line_count,
                        },
                        integration_order: None,
                    }
                } else {
                    CommitSource {
                        location: unattributed(Some(commit.clone()), line_count),
                        integration_order: None,
                    }
                };
                choices.push((line_count, commit, source));
            }
            let selected = choices
                .into_iter()
                .max_by(|left, right| {
                    left.0
                        .cmp(&right.0)
                        .then_with(|| left.2.integration_order.cmp(&right.2.integration_order))
                        .then_with(|| left.1.cmp(&right.1))
                })
                .map(|(line_count, _, mut source)| {
                    source.location.line_count = line_count;
                    source.location
                })
                .unwrap_or_else(|| unattributed(None, 0));
            attributed.push(selected);
        }
        Ok(attributed)
    }

    /// Resolve every commit in `base..head` to its first introducing
    /// integration, explicit Human attribution, upstream status, or an
    /// unrecorded block. Git author/committer strings are never consulted.
    pub async fn contributors(
        &self,
        task_id: &str,
        repo_id: &str,
        base: &str,
        head: &str,
    ) -> Result<RangeContributors> {
        let facts = self.attribution_facts(task_id, repo_id, base, head).await?;
        Ok(RangeContributors {
            contributors: facts.contributors,
            unrecorded_commits: facts.unrecorded_commits,
            upstream_commits: facts.upstream_commits,
        })
    }

    /// Sum the lines attributed to each Actor and return the largest author.
    /// Equal totals prefer the Actor with the latest recorded integration in
    /// `integration_started_at`; equal timestamps use descending integration
    /// ID, then the stable Actor key. The returned location is selected by the
    /// same order, with commit, Execution, and WorkUnit IDs as final fallbacks.
    pub fn primary_author(
        locations: &[ReviewLocationAttribution],
        integration_started_at: &BTreeMap<String, String>,
    ) -> Option<ReviewLocationAttribution> {
        let mut candidates = BTreeMap::<
            (String, String),
            (
                i64,
                (
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                ),
                ReviewLocationAttribution,
            ),
        >::new();
        for location in locations {
            let Some(actor) = location.actor.as_ref() else {
                continue;
            };
            if location.line_count <= 0 {
                continue;
            }
            let key = (actor.kind().to_string(), actor.id().to_owned());
            let started_at = location
                .integration_id
                .as_ref()
                .and_then(|id| integration_started_at.get(id).cloned());
            let order = (
                started_at,
                location.integration_id.clone(),
                location.commit_sha.clone(),
                location.execution_id.clone(),
                location.work_unit_id.clone(),
            );
            match candidates.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert((location.line_count, order, location.clone()));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let candidate = entry.get_mut();
                    candidate.0 += location.line_count;
                    if order > candidate.1 {
                        candidate.1 = order;
                        candidate.2 = location.clone();
                    }
                }
            }
        }
        candidates
            .into_iter()
            .max_by(|(left_key, left), (right_key, right)| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| left.1.cmp(&right.1))
                    .then_with(|| left_key.cmp(right_key))
            })
            .map(|(_, (total, _, mut attribution))| {
                attribution.line_count = total;
                attribution
            })
    }

    async fn attribution_facts(
        &self,
        task_id: &str,
        repo_id: &str,
        base: &str,
        head: &str,
    ) -> Result<AttributionFacts> {
        let rows = self.db.load_review_coverage_rows(task_id, repo_id).await?;
        self.attribution_facts_from_rows(task_id, repo_id, base, head, &rows)
            .await
    }

    async fn attribution_facts_from_rows(
        &self,
        task_id: &str,
        repo_id: &str,
        base: &str,
        head: &str,
        rows: &ReviewCoverageRows,
    ) -> Result<AttributionFacts> {
        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
        let repo_path = resolve_repo_source(&repo, &self.workspace_root).await?;
        if !git::commit_exists(Path::new(&repo_path), base).await?
            || !git::commit_exists(Path::new(&repo_path), head).await?
        {
            return Err(ServiceError::Coordination {
                code: "review.range.unrecorded_commits",
                message: "the review range includes a commit that is unavailable locally"
                    .to_owned(),
            });
        }
        let commits = git::rev_list(Path::new(&repo_path), head, base).await?;
        let mut integrations = rows
            .integrations
            .iter()
            .filter(|integration| {
                integration.outcome == WorkUnitRepositoryIntegrationOutcome::Success
                    && integration.target_after_sha.is_some()
            })
            .cloned()
            .collect::<Vec<_>>();
        integrations.sort_by(|left, right| {
            (&left.started_at, &left.id).cmp(&(&right.started_at, &right.id))
        });
        let mut commit_integrations = BTreeMap::<String, WorkUnitRepositoryIntegration>::new();
        for integration in &integrations {
            let Some(after) = integration.target_after_sha.as_deref() else {
                continue;
            };
            for commit in
                git::rev_list(Path::new(&repo_path), after, &integration.target_before_sha).await?
            {
                commit_integrations
                    .entry(commit)
                    .or_insert_with(|| integration.clone());
            }
        }
        let manual_attributions = rows
            .attributions
            .iter()
            .map(|attribution| (attribution.commit_sha.clone(), attribution))
            .collect::<BTreeMap<_, _>>();
        let default_branch_exists = git::commit_exists(Path::new(&repo_path), &repo.default_branch)
            .await
            .unwrap_or(false);
        let mut sources = BTreeMap::new();
        let mut contributors = BTreeMap::<(String, String), ReviewRangeContributor>::new();
        let mut unrecorded_commits = Vec::new();
        let mut upstream_commits = Vec::new();
        for commit in commits {
            if let Some(integration) = commit_integrations.get(&commit) {
                let execution = db::ExecutionRepo::get_by_id(&*self.db, &integration.execution_id)
                    .await?
                    .ok_or_else(|| {
                        ServiceError::invalid_operation("integrated commit Execution is missing")
                    })?;
                let actor =
                    actor_from_execution(&execution.actor_kind, execution.actor_id.as_deref())
                        .ok_or_else(|| {
                            ServiceError::invalid_operation(
                                "integrated commit has no recorded Actor",
                            )
                        })?;
                let contributor = ReviewRangeContributor {
                    id: new_uuid_v4(),
                    task_id: task_id.to_owned(),
                    range_id: String::new(),
                    work_unit_id: Some(integration.work_unit_id.clone()),
                    execution_id: Some(integration.execution_id.clone()),
                    attribution_id: None,
                    actor: actor.clone(),
                    created_at: integration.created_at.clone(),
                };
                contributors
                    .entry(("execution".to_owned(), integration.execution_id.clone()))
                    .or_insert_with(|| contributor.clone());
                sources.insert(
                    commit.clone(),
                    CommitSource {
                        location: ReviewLocationAttribution {
                            kind: ReviewAttributionKind::WorkUnit,
                            work_unit_id: Some(integration.work_unit_id.clone()),
                            execution_id: Some(integration.execution_id.clone()),
                            actor: Some(actor),
                            commit_sha: Some(commit.clone()),
                            integration_id: Some(integration.id.clone()),
                            line_count: 0,
                        },
                        integration_order: Some((
                            integration.started_at.clone(),
                            integration.id.clone(),
                        )),
                    },
                );
            } else if let Some(attribution) = manual_attributions.get(&commit) {
                let contributor = ReviewRangeContributor {
                    id: new_uuid_v4(),
                    task_id: task_id.to_owned(),
                    range_id: String::new(),
                    work_unit_id: None,
                    execution_id: None,
                    attribution_id: Some(attribution.id.clone()),
                    actor: attribution.actor.clone(),
                    created_at: attribution.created_at.clone(),
                };
                contributors
                    .entry(("attribution".to_owned(), attribution.id.clone()))
                    .or_insert_with(|| contributor.clone());
                sources.insert(
                    commit.clone(),
                    CommitSource {
                        location: ReviewLocationAttribution {
                            // The frozen location enum has no separate manual
                            // Actor value; retain the Actor while marking the
                            // source as an externally attributed commit.
                            kind: ReviewAttributionKind::Unattributed,
                            work_unit_id: None,
                            execution_id: None,
                            actor: Some(attribution.actor.clone()),
                            commit_sha: Some(commit.clone()),
                            integration_id: None,
                            line_count: 0,
                        },
                        integration_order: Some((
                            attribution.created_at.clone(),
                            attribution.id.clone(),
                        )),
                    },
                );
            } else if default_branch_exists
                && git::is_ancestor(Path::new(&repo_path), &commit, &repo.default_branch).await?
            {
                upstream_commits.push(commit.clone());
                sources.insert(
                    commit.clone(),
                    CommitSource {
                        location: ReviewLocationAttribution {
                            kind: ReviewAttributionKind::Upstream,
                            work_unit_id: None,
                            execution_id: None,
                            actor: None,
                            commit_sha: Some(commit),
                            integration_id: None,
                            line_count: 0,
                        },
                        integration_order: None,
                    },
                );
            } else {
                unrecorded_commits.push(commit);
            }
        }
        let mut contributors = contributors.into_values().collect::<Vec<_>>();
        contributors.sort_by(|left, right| {
            (left.actor.kind().to_string(), left.actor.id(), &left.id).cmp(&(
                right.actor.kind().to_string(),
                right.actor.id(),
                &right.id,
            ))
        });
        unrecorded_commits.sort();
        upstream_commits.sort();
        Ok(AttributionFacts {
            sources,
            contributors,
            unrecorded_commits,
            upstream_commits,
            repo_path: PathBuf::from(repo_path),
            default_branch: repo.default_branch,
        })
    }

    async fn is_ancestor_if_present(
        &self,
        facts: &AttributionFacts,
        commit: &str,
        branch: &str,
    ) -> Result<bool> {
        if !git::commit_exists(Path::new(&facts.repo_path), commit).await?
            || !git::commit_exists(Path::new(&facts.repo_path), branch).await?
        {
            return Ok(false);
        }
        Ok(git::is_ancestor(Path::new(&facts.repo_path), commit, branch).await?)
    }
}

fn actor_from_execution(
    actor_kind: &Option<ActorKind>,
    actor_id: Option<&str>,
) -> Option<ActorRef> {
    match (*actor_kind.as_ref()?, actor_id?) {
        (ActorKind::Human, id) => Some(ActorRef::Human(id.to_owned())),
        (ActorKind::Agent, id) => Some(ActorRef::Agent(id.to_owned())),
    }
}

fn unattributed(commit_sha: Option<String>, line_count: i64) -> ReviewLocationAttribution {
    ReviewLocationAttribution {
        kind: ReviewAttributionKind::Unattributed,
        work_unit_id: None,
        execution_id: None,
        actor: None,
        commit_sha,
        integration_id: None,
        line_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(actor: &str, lines: i64, integration: &str) -> ReviewLocationAttribution {
        ReviewLocationAttribution {
            kind: ReviewAttributionKind::WorkUnit,
            work_unit_id: Some(format!("wu-{actor}")),
            execution_id: Some(format!("exec-{actor}")),
            actor: Some(ActorRef::Human(actor.to_owned())),
            commit_sha: Some(format!("commit-{actor}")),
            integration_id: Some(integration.to_owned()),
            line_count: lines,
        }
    }

    #[test]
    fn review_range_primary_author_sums_actor_lines_across_work_units() {
        let mut second_unit = source("one", 2, "i-3");
        second_unit.work_unit_id = Some("another-work-unit".to_owned());
        second_unit.execution_id = Some("another-execution".to_owned());
        let attributed = vec![
            source("one", 3, "i-1"),
            source("two", 5, "i-2"),
            second_unit,
        ];
        let integration_started_at = BTreeMap::from([
            ("i-1".to_owned(), "2026-11-27T10:00:00Z".to_owned()),
            ("i-2".to_owned(), "2026-11-27T10:01:00Z".to_owned()),
            ("i-3".to_owned(), "2026-11-27T10:02:00Z".to_owned()),
        ]);
        let author = ReviewAttribution::primary_author(&attributed, &integration_started_at)
            .expect("primary author");
        assert_eq!(author.actor, Some(ActorRef::Human("one".to_owned())));
        assert_eq!(author.line_count, 5);
        assert_eq!(author.integration_id.as_deref(), Some("i-3"));
    }

    #[test]
    fn review_range_primary_author_breaks_line_ties_by_latest_integration_timestamp() {
        let attributed = vec![source("older", 5, "i-old"), source("newer", 5, "i-new")];
        let integration_started_at = BTreeMap::from([
            ("i-old".to_owned(), "2026-11-27T09:00:00Z".to_owned()),
            ("i-new".to_owned(), "2026-11-27T10:00:00Z".to_owned()),
        ]);

        let author = ReviewAttribution::primary_author(&attributed, &integration_started_at)
            .expect("primary author");
        assert_eq!(author.actor, Some(ActorRef::Human("newer".to_owned())));
        assert_eq!(author.integration_id.as_deref(), Some("i-new"));
        assert_eq!(author.line_count, 5);

        let same_time = vec![source("a", 5, "i-a"), source("z", 5, "i-z")];
        let equal_timestamps = BTreeMap::from([
            ("i-a".to_owned(), "2026-11-27T10:00:00Z".to_owned()),
            ("i-z".to_owned(), "2026-11-27T10:00:00Z".to_owned()),
        ]);
        let deterministic = ReviewAttribution::primary_author(&same_time, &equal_timestamps)
            .expect("stable integration-ID tie break");
        assert_eq!(deterministic.actor, Some(ActorRef::Human("z".to_owned())));
        assert_eq!(deterministic.integration_id.as_deref(), Some("i-z"));
    }
}
