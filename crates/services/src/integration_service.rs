use crate::{
    external_api::{
        gitea::GiteaClient, github::GitHubClient, resolve_token, IssueFetcher, SyncFilter,
    },
    Result, ServiceError, TaskService,
};
use api_types::ActorRef;
use db::{
    new_uuid_v4, now_rfc3339, CoordinationMode, CreateProjectIntegration, CreateTaskExternalLink,
    ExternalLinkRepo, IntegrationPlatform, IntegrationRepo, ProjectIntegration, ProjectRepo,
    SqliteDb, UpdateProjectIntegration,
};
use events::EventBus;
use std::sync::Arc;

pub struct IntegrationService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    task_service: Arc<TaskService>,
}

pub struct SyncResult {
    pub imported: u32,
    pub skipped: u32,
    pub errors: u32,
}

impl IntegrationService {
    pub fn new(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        task_service: Arc<TaskService>,
    ) -> Self {
        Self {
            db,
            event_bus,
            task_service,
        }
    }

    pub async fn create_integration(
        &self,
        input: CreateProjectIntegration,
    ) -> Result<ProjectIntegration> {
        self.validate_create(&input).await?;
        Ok(IntegrationRepo::create_integration(&*self.db, input).await?)
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<ProjectIntegration>> {
        validate_required("id", id)?;
        Ok(IntegrationRepo::get_by_id(&*self.db, id).await?)
    }

    pub async fn get_by_project_id(&self, project_id: &str) -> Result<Option<ProjectIntegration>> {
        validate_required("project_id", project_id)?;
        Ok(IntegrationRepo::get_by_project_id(&*self.db, project_id).await?)
    }

    pub async fn update_integration(
        &self,
        input: UpdateProjectIntegration,
    ) -> Result<ProjectIntegration> {
        self.validate_update(&input).await?;
        Ok(IntegrationRepo::update_integration(&*self.db, input).await?)
    }

    pub async fn delete_integration(&self, id: &str) -> Result<()> {
        validate_required("id", id)?;
        Ok(IntegrationRepo::delete_integration(&*self.db, id).await?)
    }

    pub async fn sync_integration(&self, integration: &ProjectIntegration) -> Result<SyncResult> {
        let _ = &self.event_bus;
        let default_implementer = integration_default_implementer(integration)?;
        if let Some(actor) = default_implementer.as_ref() {
            let project = ProjectRepo::get_by_id(&*self.db, &integration.project_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("project", integration.project_id.clone())
                })?;
            self.task_service
                .validate_actor_for_project(&project, actor)
                .await?;
        }
        let token = resolve_token(&integration.token_secret_ref)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let sync_filter = parse_sync_filter(&integration.sync_filter);
        let fetcher = issue_fetcher(integration);
        let issues = fetcher
            .fetch_issues(
                &integration.owner,
                &integration.repo,
                &token,
                integration.last_polled_at.as_deref(),
                &sync_filter,
            )
            .await
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;

        let mut imported = 0;
        let mut skipped = 0;
        for issue in issues {
            if integration.platform == IntegrationPlatform::Github
                && issue.html_url.contains("/pull/")
            {
                skipped += 1;
                continue;
            }

            let platform = integration.platform.to_string();
            let global_id = compute_global_id(
                &platform,
                &integration.base_url,
                &integration.owner,
                &integration.repo,
                issue.number,
            );
            if ExternalLinkRepo::get_by_global_id(&*self.db, &global_id)
                .await?
                .is_some()
            {
                skipped += 1;
                continue;
            }

            let task = self
                .task_service
                .create_task(
                    integration.project_id.clone(),
                    issue.title.clone(),
                    issue.body.clone(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;

            if let Some(actor) = default_implementer.as_ref() {
                self.task_service
                    .create_task_role(
                        &task.id,
                        "implementer",
                        CoordinationMode::Independent,
                        "{}".to_owned(),
                    )
                    .await?;
                self.task_service
                    .add_task_role_member(&task.id, "implementer", actor.clone())
                    .await?;
            }

            let now = now_rfc3339();
            ExternalLinkRepo::create_link(
                &*self.db,
                CreateTaskExternalLink {
                    id: new_uuid_v4(),
                    task_id: task.id.clone(),
                    integration_id: integration.id.clone(),
                    platform,
                    remote_owner: integration.owner.clone(),
                    remote_repo: integration.repo.clone(),
                    remote_issue_number: issue.number,
                    remote_url: compute_remote_url(
                        &integration.platform,
                        &integration.base_url,
                        &integration.owner,
                        &integration.repo,
                        issue.number,
                    ),
                    global_id,
                    synced_at: now.clone(),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
            )
            .await?;

            imported += 1;
        }

        Ok(SyncResult {
            imported,
            skipped,
            errors: 0,
        })
    }

    async fn validate_create(&self, input: &CreateProjectIntegration) -> Result<()> {
        validate_required("id", &input.id)?;
        validate_required("project_id", &input.project_id)?;
        validate_required("base_url", &input.base_url)?;
        validate_required("owner", &input.owner)?;
        validate_required("repo", &input.repo)?;
        validate_required("token_secret_ref", &input.token_secret_ref)?;
        validate_poll_interval(input.poll_interval_secs)?;
        validate_sync_filter(&input.sync_filter)?;
        ProjectRepo::get_by_id(&*self.db, &input.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", input.project_id.clone()))?;
        Ok(())
    }

    async fn validate_update(&self, input: &UpdateProjectIntegration) -> Result<()> {
        validate_required("id", &input.id)?;
        IntegrationRepo::get_by_id(&*self.db, &input.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("integration", input.id.clone()))?;
        if let Some(project_id) = &input.project_id {
            validate_required("project_id", project_id)?;
            ProjectRepo::get_by_id(&*self.db, project_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("project", project_id.clone()))?;
        }
        if let Some(base_url) = &input.base_url {
            validate_required("base_url", base_url)?;
        }
        if let Some(owner) = &input.owner {
            validate_required("owner", owner)?;
        }
        if let Some(repo) = &input.repo {
            validate_required("repo", repo)?;
        }
        if let Some(token_secret_ref) = &input.token_secret_ref {
            validate_required("token_secret_ref", token_secret_ref)?;
        }
        if let Some(poll_interval_secs) = input.poll_interval_secs {
            validate_poll_interval(poll_interval_secs)?;
        }
        if let Some(sync_filter) = &input.sync_filter {
            validate_sync_filter(sync_filter)?;
        }
        Ok(())
    }
}

fn integration_default_implementer(integration: &ProjectIntegration) -> Result<Option<ActorRef>> {
    match (
        integration.default_assignee_type.as_deref(),
        integration.default_assignee_id.as_deref(),
    ) {
        (None, None) => Ok(None),
        (Some("agent"), Some(id)) if !id.is_empty() => Ok(Some(ActorRef::Agent(id.to_owned()))),
        (Some("user"), Some(id)) if !id.is_empty() => Ok(Some(ActorRef::Human(id.to_owned()))),
        _ => Err(ServiceError::invalid_operation(
            "integration default implementer must be an exact Agent or Human Actor reference",
        )),
    }
}

fn issue_fetcher(integration: &ProjectIntegration) -> Box<dyn IssueFetcher> {
    match integration.platform {
        IntegrationPlatform::Github => Box::new(GitHubClient),
        IntegrationPlatform::Gitea => Box::new(GiteaClient {
            base_url: integration.base_url.clone(),
        }),
    }
}

fn parse_sync_filter(sync_filter: &str) -> SyncFilter {
    if sync_filter.trim().is_empty() {
        return SyncFilter::default();
    }
    serde_json::from_str(sync_filter).unwrap_or_default()
}

fn validate_required(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(ServiceError::invalid_operation(format!(
            "{field} is required"
        )));
    }
    Ok(())
}

fn validate_poll_interval(poll_interval_secs: i64) -> Result<()> {
    if poll_interval_secs <= 0 {
        return Err(ServiceError::invalid_operation(
            "poll_interval_secs must be greater than 0",
        ));
    }
    Ok(())
}

fn validate_sync_filter(sync_filter: &str) -> Result<()> {
    if sync_filter.trim().is_empty() {
        return Ok(());
    }
    serde_json::from_str::<serde_json::Value>(sync_filter)
        .map(|_| ())
        .map_err(|error| ServiceError::invalid_operation(format!("invalid sync_filter: {error}")))
}

fn compute_global_id(
    platform: &str,
    base_url: &str,
    owner: &str,
    repo: &str,
    number: i64,
) -> String {
    let host = url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();
    let is_well_known = matches!(host.as_str(), "api.github.com" | "github.com");
    if is_well_known {
        format!("{platform}:{owner}/{repo}#{number}")
    } else {
        format!("{platform}:{host}:{owner}/{repo}#{number}")
    }
}

fn compute_remote_url(
    platform: &IntegrationPlatform,
    base_url: &str,
    owner: &str,
    repo: &str,
    number: i64,
) -> String {
    match platform {
        IntegrationPlatform::Github => {
            format!("https://github.com/{owner}/{repo}/issues/{number}")
        }
        IntegrationPlatform::Gitea => {
            let base = base_url.trim_end_matches('/');
            format!("{base}/{owner}/{repo}/issues/{number}")
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn compute_global_id_omits_well_known_github_host() {
        let global_id =
            super::compute_global_id("github", "https://api.github.com", "owner", "repo", 7);

        assert_eq!(global_id, "github:owner/repo#7");
    }

    #[test]
    fn compute_global_id_includes_self_hosted_gitea_host() {
        let global_id =
            super::compute_global_id("gitea", "https://gitea.example.com", "owner", "repo", 42);

        assert_eq!(global_id, "gitea:gitea.example.com:owner/repo#42");
    }

    #[test]
    fn compute_global_id_distinguishes_different_hosts() {
        let first = super::compute_global_id("gitea", "https://gitea.a.com", "owner", "repo", 1);
        let second = super::compute_global_id("gitea", "https://gitea.b.com", "owner", "repo", 1);

        assert_ne!(first, second);
    }
}
