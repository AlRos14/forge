use std::sync::Arc;

use db::{now_rfc3339, Project, ProjectRepo, SqliteDb, UpdateProject};

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
}
