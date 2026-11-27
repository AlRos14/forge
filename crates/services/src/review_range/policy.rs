//! Versioned Task review policy and the Project default copied into new Tasks.

use super::canonical_digest;
use crate::{
    project_authority::user_can_administer_project, task_service::workspace::resolve_repo_source,
    Result, ServiceError,
};
use db::{
    CreateDomainEvent, DbError, ProjectRepo, ProjectReviewPolicy, RepoRepo, ReviewBaseSource,
    ReviewLedgerRepo, ReviewPolicyDocument, ReviewPolicyRepo, ReviewPolicySource, ReviewRiskClass,
    SqliteDb, Task, TaskRepo, TaskRepositoryRole, TaskReviewBase, TaskReviewPolicyRevision,
    WorkUnitRepositoryIntegration, WorkUnitRepositoryIntegrationOutcome, WriteProjectReviewPolicy,
    WriteTaskReviewPolicy,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_REVIEWER_ROLE_BYTES: usize = 64;
const MAX_CHECKPOINTS_PER_REPO: u32 = 64;

pub struct ReviewPolicyService {
    db: Arc<SqliteDb>,
    workspace_root: PathBuf,
}

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::Coordination {
        code: "review.policy.invalid",
        message: message.into(),
    }
}

fn not_authorized() -> ServiceError {
    ServiceError::Coordination {
        code: "review.policy.not_authorized",
        message: "only a Project owner or admin may change the review policy".to_owned(),
    }
}

fn write_error(error: DbError) -> ServiceError {
    match error {
        DbError::VersionConflict => ServiceError::Coordination {
            code: "review.policy.version_conflict",
            message: "the review policy changed; reload it and retry".to_owned(),
        },
        DbError::ProjectAdminRequired => not_authorized(),
        error => error.into(),
    }
}

/// The earliest successful integration of a repository, which pins its base.
fn first_successful_integration(
    integrations: &[WorkUnitRepositoryIntegration],
) -> Option<&WorkUnitRepositoryIntegration> {
    integrations
        .iter()
        .filter(|row| row.outcome == WorkUnitRepositoryIntegrationOutcome::Success)
        .min_by(|left, right| (&left.started_at, &left.id).cmp(&(&right.started_at, &right.id)))
}

impl ReviewPolicyService {
    pub fn new(db: Arc<SqliteDb>, workspace_root: PathBuf) -> Self {
        Self { db, workspace_root }
    }

    pub fn validate(doc: &ReviewPolicyDocument) -> Result<()> {
        if doc.schema_version != 1 {
            return Err(invalid("schema_version must be 1"));
        }
        if doc.coverage_required != (doc.risk_class != ReviewRiskClass::R0) {
            return Err(invalid(
                "coverage_required must be false for r0 and true for r1 and r2",
            ));
        }
        if doc.required_reviewers != 1 {
            return Err(invalid("required_reviewers must be 1"));
        }
        if doc.reviewer_role.len() > MAX_REVIEWER_ROLE_BYTES
            || db::canonical_task_role_name(&doc.reviewer_role).as_deref()
                != Some(doc.reviewer_role.as_str())
        {
            return Err(invalid(
                "reviewer_role must be a canonical TaskRole name of at most 64 bytes",
            ));
        }
        if doc.size_threshold.changed_lines == 0 || doc.size_threshold.changed_files == 0 {
            return Err(invalid("size_threshold values must be at least 1"));
        }
        if !(1..=MAX_CHECKPOINTS_PER_REPO).contains(&doc.max_checkpoints_per_repo) {
            return Err(invalid("max_checkpoints_per_repo must be between 1 and 64"));
        }
        Ok(())
    }

    pub fn digest(doc: &ReviewPolicyDocument) -> Result<String> {
        canonical_digest(doc)
    }

    pub async fn current(&self, task_id: &str) -> Result<Option<TaskReviewPolicyRevision>> {
        Ok(self.db.get_current_task_review_policy(task_id).await?)
    }

    /// Writes the next policy revision. A Task that already has a
    /// `merge_readiness` Gate is rejected until the Gate can be revised to
    /// policy v4 in the same transaction.
    pub async fn set(
        &self,
        user_id: &str,
        task_id: &str,
        expected_revision: Option<i64>,
        policy: ReviewPolicyDocument,
    ) -> Result<TaskReviewPolicyRevision> {
        Self::validate(&policy)?;
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        self.require_admin(&task.project_id, user_id).await?;
        let has_merge_gate: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM gate
             WHERE task_id = ? AND gate_kind = 'merge_readiness')",
        )
        .bind(task_id)
        .fetch_one(self.db.pool())
        .await?;
        if has_merge_gate {
            return super::not_implemented(
                "revising a Task's merge_readiness Gate together with its review policy",
            );
        }
        let digest = Self::digest(&policy)?;
        let created_at = db::now_rfc3339();
        let initial_bases = self.initial_bases(&task, &created_at).await?;
        let revision = expected_revision.unwrap_or(0).saturating_add(1);
        let event = CreateDomainEvent {
            id: db::new_uuid_v4(),
            event_type: "task.review_policy_updated".to_owned(),
            entity_type: "task".to_owned(),
            entity_id: task_id.to_owned(),
            actor_type: "human".to_owned(),
            actor_id: Some(user_id.to_owned()),
            scope_type: "task".to_owned(),
            scope_id: task_id.to_owned(),
            correlation_id: task_id.to_owned(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("task-review-policy:{task_id}:{revision}")),
            payload_json: serde_json::json!({
                "task_id": task_id,
                "revision": revision,
                "risk_class": policy.risk_class.to_string(),
                "coverage_required": policy.coverage_required,
            })
            .to_string(),
            created_at: created_at.clone(),
        };
        self.db
            .write_task_review_policy(WriteTaskReviewPolicy {
                task_id: task_id.to_owned(),
                expected_revision,
                policy,
                digest,
                source: ReviewPolicySource::Owner,
                created_by_user_id: user_id.to_owned(),
                gate_revisions: Vec::new(),
                initial_bases,
                events: vec![event],
                created_at,
            })
            .await
            .map_err(write_error)
    }

    pub async fn project_default(&self, project_id: &str) -> Result<Option<ProjectReviewPolicy>> {
        Ok(self.db.get_project_review_policy(project_id).await?)
    }

    pub async fn set_project_default(
        &self,
        user_id: &str,
        project_id: &str,
        expected_version: Option<i64>,
        policy: Option<ReviewPolicyDocument>,
    ) -> Result<ProjectReviewPolicy> {
        let policy = policy
            .map(|policy| {
                Self::validate(&policy)?;
                let digest = Self::digest(&policy)?;
                Ok::<_, ServiceError>((policy, digest))
            })
            .transpose()?;
        self.require_admin(project_id, user_id).await?;
        self.db
            .write_project_review_policy(WriteProjectReviewPolicy {
                project_id: project_id.to_owned(),
                expected_version,
                policy,
                updated_by_user_id: user_id.to_owned(),
                updated_at: db::now_rfc3339(),
            })
            .await
            .map_err(write_error)
    }

    /// The pinned base of a repository, or `None` when none can be determined
    /// yet. Pinning outside a policy write needs a persistence writer that does
    /// not exist yet, so a determinable but unpinned base is an error.
    pub async fn pin_base(&self, task_id: &str, repo_id: &str) -> Result<Option<TaskReviewBase>> {
        let rows = self.db.load_review_coverage_rows(task_id, repo_id).await?;
        if rows.base.is_some() {
            return Ok(rows.base);
        }
        if first_successful_integration(&rows.integrations).is_none() {
            return Ok(None);
        }
        super::not_implemented("pinning a review base outside a policy write")
    }

    async fn require_admin(&self, project_id: &str, user_id: &str) -> Result<()> {
        let project = ProjectRepo::get_by_id(&*self.db, project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.to_owned()))?;
        if user_can_administer_project(&self.db, &project, user_id).await? {
            Ok(())
        } else {
            Err(not_authorized())
        }
    }

    /// Revision-1 bases for writable repositories that already have a
    /// successful integration: the target commit before the first one.
    async fn initial_bases(&self, task: &Task, pinned_at: &str) -> Result<Vec<TaskReviewBase>> {
        let mut bases = Vec::new();
        for member in TaskRepo::list_repositories(&*self.db, &task.id).await? {
            if !matches!(
                member.role,
                TaskRepositoryRole::Primary | TaskRepositoryRole::Affected
            ) {
                continue;
            }
            let rows = self
                .db
                .load_review_coverage_rows(&task.id, &member.repo_id)
                .await?;
            if rows.base.is_some() {
                continue;
            }
            let Some(first) = first_successful_integration(&rows.integrations) else {
                continue;
            };
            let repo = RepoRepo::get_by_id(&*self.db, &member.repo_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("repo", member.repo_id.clone()))?;
            let source = resolve_repo_source(&repo, &self.workspace_root).await?;
            let base_tree = git::resolve_tree(Path::new(&source), &first.target_before_sha).await?;
            bases.push(TaskReviewBase {
                task_id: task.id.clone(),
                repo_id: member.repo_id,
                revision: 1,
                base_commit: first.target_before_sha.clone(),
                base_tree,
                source: ReviewBaseSource::FirstIntegration,
                decision_id: None,
                pinned_at: pinned_at.to_owned(),
            });
        }
        Ok(bases)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{ReviewSizeThreshold, ReviewTriggerMode};

    fn policy() -> ReviewPolicyDocument {
        ReviewPolicyDocument {
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
        }
    }

    fn rejected(change: impl FnOnce(&mut ReviewPolicyDocument)) {
        let mut document = policy();
        change(&mut document);
        assert!(
            matches!(
                ReviewPolicyService::validate(&document),
                Err(ServiceError::Coordination {
                    code: "review.policy.invalid",
                    ..
                })
            ),
            "{document:?}"
        );
    }

    #[test]
    fn review_range_policy_accepts_each_risk_class() {
        ReviewPolicyService::validate(&policy()).unwrap();
        let mut r2 = policy();
        r2.risk_class = ReviewRiskClass::R2;
        ReviewPolicyService::validate(&r2).unwrap();
        let mut r0 = policy();
        r0.risk_class = ReviewRiskClass::R0;
        r0.coverage_required = false;
        ReviewPolicyService::validate(&r0).unwrap();
    }

    #[test]
    fn review_range_policy_rejects_out_of_contract_values() {
        rejected(|doc| doc.schema_version = 2);
        rejected(|doc| doc.coverage_required = false);
        rejected(|doc| doc.risk_class = ReviewRiskClass::R0);
        rejected(|doc| doc.required_reviewers = 0);
        rejected(|doc| doc.required_reviewers = 2);
        rejected(|doc| doc.reviewer_role = String::new());
        rejected(|doc| doc.reviewer_role = "Reviewer".to_owned());
        rejected(|doc| doc.reviewer_role = "r".repeat(65));
        rejected(|doc| doc.size_threshold.changed_lines = 0);
        rejected(|doc| doc.size_threshold.changed_files = 0);
        rejected(|doc| doc.max_checkpoints_per_repo = 0);
        rejected(|doc| doc.max_checkpoints_per_repo = 65);
    }

    #[test]
    fn review_range_policy_digest_is_canonical() {
        let digest = ReviewPolicyService::digest(&policy()).unwrap();
        let canonical = concat!(
            r#"{"schema_version":1,"risk_class":"r1","coverage_required":true,"#,
            r#""trigger_mode":"auto","reviewer_role":"reviewer","required_reviewers":1,"#,
            r#""require_human":false,"#,
            r#""size_threshold":{"changed_lines":800,"changed_files":25},"#,
            r#""coordinator_may_request":true,"max_checkpoints_per_repo":12}"#,
        );
        assert_eq!(serde_json::to_string(&policy()).unwrap(), canonical);
        assert_eq!(digest.len(), 64);
        let mut changed = policy();
        changed.require_human = true;
        assert_ne!(ReviewPolicyService::digest(&changed).unwrap(), digest);
    }
}
