use std::sync::Arc;

use db::{
    now_rfc3339, Project, ProjectRepo, ReviewPolicyDocument, ReviewPolicyRepo, SqliteDb,
    UpdateProject, WriteProjectReviewPolicy,
};

use crate::{Result, ServiceError};

pub struct ProjectRepositoryService {
    db: Arc<SqliteDb>,
}

impl ProjectRepositoryService {
    pub fn new(db: Arc<SqliteDb>) -> Self {
        Self { db }
    }

    pub async fn update_primary_repo(
        &self,
        project_id: &str,
        expected_version: i64,
        primary_repo_id: Option<String>,
        caller_user_id: &str,
    ) -> Result<Project> {
        self.update_project_at_version(
            UpdateProject {
                id: project_id.to_owned(),
                name: None,
                settings: None,
                primary_repo_id: Some(primary_repo_id),
                paused_at: None,
                updated_at: now_rfc3339(),
            },
            expected_version,
            None,
            caller_user_id,
        )
        .await
    }

    pub async fn update_project_at_version(
        &self,
        input: UpdateProject,
        expected_version: i64,
        project_hooks_json: Option<String>,
        caller_user_id: &str,
    ) -> Result<Project> {
        ProjectRepo::update_at_version(
            &*self.db,
            input,
            expected_version,
            project_hooks_json,
            Some(caller_user_id),
        )
        .await
        .map_err(|error| match error {
            db::DbError::Check(message) if message == "project.primary_repo_not_in_project" => {
                ServiceError::ProjectPrimaryRepoNotInProject
            }
            other => ServiceError::from(other),
        })
    }

    /// Updates Project fields and, when supplied, its default review policy in
    /// one database transaction. The Project version remains the caller's
    /// concurrency fence; the policy version is read before the transaction
    /// and revalidated by the same atomic write.
    pub async fn update_project_at_version_with_review_policy(
        &self,
        input: UpdateProject,
        expected_version: i64,
        project_hooks_json: Option<String>,
        caller_user_id: &str,
        default_review_policy: Option<Option<ReviewPolicyDocument>>,
    ) -> Result<Project> {
        let policy_update = if let Some(policy) = default_review_policy {
            let current = ReviewPolicyRepo::get_project_review_policy(&*self.db, &input.id).await?;
            let policy = policy
                .map(|policy| {
                    crate::review_range::ReviewPolicyService::validate(&policy)?;
                    let digest = crate::review_range::ReviewPolicyService::digest(&policy)?;
                    Ok::<_, ServiceError>((policy, digest))
                })
                .transpose()?;
            Some(WriteProjectReviewPolicy {
                project_id: input.id.clone(),
                expected_version: current.as_ref().map(|record| record.version),
                policy,
                updated_by_user_id: caller_user_id.to_owned(),
                updated_at: input.updated_at.clone(),
            })
        } else {
            None
        };
        ProjectRepo::update_at_version_with_review_policy(
            &*self.db,
            input,
            expected_version,
            project_hooks_json,
            Some(caller_user_id),
            policy_update,
        )
        .await
        .map_err(|error| match error {
            db::DbError::Check(message) if message == "project.primary_repo_not_in_project" => {
                ServiceError::ProjectPrimaryRepoNotInProject
            }
            other => ServiceError::from(other),
        })
    }
}
