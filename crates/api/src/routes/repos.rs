use api_types::{
    CreateRepoRequest, PaginatedResponse, RepoResponse, RepoSyncResponse, UpdateRepoRequest,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use db::{
    new_uuid_v4, now_rfc3339, CreatePrProviderConfig, CreateRepo, PrProviderConfigRepo,
    ProjectRepo, Repo, RepoRepo, SqliteDb, UpdatePrProviderConfig, UpdateProject, UpdateRepo,
    WorkMode,
};

use crate::{
    errors::{ApiError, ApiResult},
    path_input::canonical_directory,
    routes::{page_request, repo_response, ListParams},
    state::AppState,
};

pub async fn create_repo(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(request): Json<CreateRepoRequest>,
) -> ApiResult<Json<RepoResponse>> {
    if request.pr_provider.is_none() && request.pr_provider_config.is_some() {
        return Err(ApiError::bad_request_with_code(
            "repo.pr_provider_required",
            "pr_provider is required when creating provider configuration",
        ));
    }
    if request
        .pr_provider
        .as_ref()
        .is_some_and(|provider| provider.trim().is_empty())
    {
        return Err(ApiError::bad_request_with_code(
            "repo.pr_provider_invalid",
            "pr_provider must not be empty",
        ));
    }
    let project = ProjectRepo::get_by_id(&*state.db, &project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("project", project_id.clone()))?;
    if project.primary_repo_id.is_some() {
        return Err(ApiError::conflict_with_code(
            "project_already_has_primary_repo",
            format!("project {project_id} already has a primary repo"),
        ));
    }
    let local_path = normalize_optional_local_path(request.local_path)?;
    let name = request
        .name
        .unwrap_or_else(|| repo_name_from_remote_url(&request.remote_url));
    let now = now_rfc3339();
    let repo = RepoRepo::create(
        &*state.db,
        CreateRepo {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            name,
            local_path,
            remote_url: request.remote_url,
            work_mode: request
                .work_mode
                .map(work_mode_domain)
                .unwrap_or(WorkMode::DirectMerge),
            default_branch: request.default_branch.unwrap_or_else(|| "main".to_owned()),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await?;
    if let Some(pr_provider) = &request.pr_provider {
        let pr_config = request.pr_provider_config.as_ref();
        let now = now_rfc3339();
        PrProviderConfigRepo::create(
            &*state.db,
            CreatePrProviderConfig {
                id: new_uuid_v4(),
                repo_id: repo.id.clone(),
                provider_type: pr_provider.clone(),
                base_url: pr_config.and_then(|c| c.base_url.clone()),
                polling_interval_seconds: pr_config
                    .and_then(|c| c.polling_interval_seconds)
                    .unwrap_or(300),
                token_secret_ref: pr_config.and_then(|c| c.token.clone()),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;
    }
    ProjectRepo::update(
        &*state.db,
        UpdateProject {
            id: project_id,
            name: None,
            settings: None,
            primary_repo_id: Some(Some(repo.id.clone())),
            paused_at: None,
            updated_at: now_rfc3339(),
        },
    )
    .await?;
    Ok(Json(repo_with_provider(&*state.db, repo).await?))
}

pub async fn list_repos(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<PaginatedResponse<RepoResponse>>> {
    let page = RepoRepo::list_by_project(&*state.db, &project_id, page_request(&params)?).await?;
    let mut items = Vec::with_capacity(page.items.len());
    for repo in page.items {
        items.push(repo_with_provider(&*state.db, repo).await?);
    }
    Ok(Json(PaginatedResponse {
        has_more: page.next_cursor.is_some(),
        items,
        next_cursor: page.next_cursor,
        total_count: page.total_count.and_then(|count| u64::try_from(count).ok()),
    }))
}

pub async fn get_repo(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<RepoResponse>> {
    let repo = RepoRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("repo", id))?;
    Ok(Json(repo_with_provider(&*state.db, repo).await?))
}

pub async fn update_repo(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRepoRequest>,
) -> ApiResult<Json<RepoResponse>> {
    let db = &*state.db;
    let UpdateRepoRequest {
        name,
        remote_url,
        local_path,
        default_branch,
        work_mode,
        pr_provider,
        pr_provider_config,
    } = request;
    if pr_provider
        .as_ref()
        .and_then(|provider| provider.as_ref())
        .is_some_and(|provider| provider.trim().is_empty())
    {
        return Err(ApiError::bad_request_with_code(
            "repo.pr_provider_invalid",
            "pr_provider must not be empty",
        ));
    }
    let local_path = normalize_update_local_path(local_path)?;
    let repo = RepoRepo::update(
        db,
        UpdateRepo {
            id,
            name,
            local_path,
            remote_url,
            work_mode: work_mode.map(work_mode_domain),
            default_branch,
            updated_at: now_rfc3339(),
        },
    )
    .await?;
    update_pr_provider_config(db, &repo.id, pr_provider, pr_provider_config).await?;
    Ok(Json(repo_with_provider(db, repo).await?))
}

async fn repo_with_provider(db: &SqliteDb, repo: Repo) -> ApiResult<RepoResponse> {
    let mut response = repo_response(repo);
    if let Some(config) = PrProviderConfigRepo::get_by_repo_id(db, &response.id).await? {
        response.pr_provider = Some(config.provider_type.clone());
        response.pr_provider_status = Some(api_types::PrProviderStatus {
            provider_type: config.provider_type,
            has_token: config.token_secret_ref.is_some(),
            polling_interval_seconds: config.polling_interval_seconds,
        });
    }
    Ok(response)
}

async fn update_pr_provider_config(
    db: &SqliteDb,
    repo_id: &str,
    provider: Option<Option<String>>,
    config: Option<Option<api_types::UpdatePrProviderConfigRequest>>,
) -> ApiResult<()> {
    let existing = PrProviderConfigRepo::get_by_repo_id(db, repo_id).await?;
    if provider.as_ref().is_some_and(Option::is_none) {
        if config.as_ref().is_some_and(Option::is_some) {
            return Err(ApiError::bad_request_with_code(
                "repo.pr_provider_config_conflict",
                "provider configuration cannot be supplied when pr_provider is cleared",
            ));
        }
        if let Some(existing) = existing {
            PrProviderConfigRepo::delete(db, &existing.id).await?;
        }
        return Ok(());
    }

    if config.as_ref().is_some_and(Option::is_none) {
        if provider.as_ref().is_some_and(Option::is_some) {
            return Err(ApiError::bad_request_with_code(
                "repo.pr_provider_config_conflict",
                "provider configuration cannot be cleared while pr_provider is set",
            ));
        }
        if let Some(existing) = existing {
            PrProviderConfigRepo::delete(db, &existing.id).await?;
        }
        return Ok(());
    }

    let provider_type = provider.flatten();
    let config = config.flatten();
    if provider_type.is_none() && config.is_none() {
        return Ok(());
    }

    if let Some(existing) = existing {
        PrProviderConfigRepo::update(
            db,
            UpdatePrProviderConfig {
                id: existing.id,
                provider_type,
                base_url: config.as_ref().and_then(|config| config.base_url.clone()),
                polling_interval_seconds: config
                    .as_ref()
                    .and_then(|config| config.polling_interval_seconds),
                token_secret_ref: config.as_ref().and_then(|config| config.token.clone()),
                updated_at: now_rfc3339(),
            },
        )
        .await?;
    } else {
        let Some(provider_type) = provider_type else {
            return Err(ApiError::bad_request_with_code(
                "repo.pr_provider_required",
                "pr_provider must be set before provider configuration can be updated",
            ));
        };
        PrProviderConfigRepo::create(
            db,
            CreatePrProviderConfig {
                id: new_uuid_v4(),
                repo_id: repo_id.to_owned(),
                provider_type,
                base_url: config
                    .as_ref()
                    .and_then(|config| config.base_url.clone().flatten()),
                polling_interval_seconds: config
                    .as_ref()
                    .and_then(|config| config.polling_interval_seconds)
                    .unwrap_or(300),
                token_secret_ref: config
                    .as_ref()
                    .and_then(|config| config.token.clone().flatten()),
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await?;
    }
    Ok(())
}

pub async fn delete_repo(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    RepoRepo::delete(&*state.db, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn sync_repo(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<RepoSyncResponse>> {
    let repo = RepoRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("repo", id.clone()))?;
    let local_path = repo.local_path.ok_or_else(|| {
        ApiError::bad_request_with_code(
            "repo.no_local_path",
            "repository has no local path to sync",
        )
    })?;
    let path = std::path::PathBuf::from(&local_path);
    if !git::is_git_repo(&path).await {
        return Err(ApiError::bad_request_with_code(
            "repo.not_a_git_repo",
            format!("{local_path} is not a git repository"),
        ));
    }
    let pull_output = git::pull_ff_only(&path).await.map_err(git_sync_error)?;
    let push_output = git::push(&path).await.map_err(git_sync_error)?;
    Ok(Json(RepoSyncResponse {
        pull_output,
        push_output,
    }))
}

fn git_sync_error(error: git::GitError) -> ApiError {
    match error {
        git::GitError::CommandFailed { stderr, stdout, .. } => {
            let detail = if !stderr.trim().is_empty() {
                stderr
            } else {
                stdout
            };
            ApiError::bad_request_with_code("repo.sync_failed", detail.trim().to_string())
        }
        other => ApiError::internal(format!("git sync failed: {other}")),
    }
}

fn normalize_optional_local_path(local_path: Option<String>) -> ApiResult<Option<String>> {
    local_path
        .map(|path| canonical_directory(&path).map(|path| path.to_string_lossy().into_owned()))
        .transpose()
}

fn normalize_update_local_path(
    local_path: Option<Option<String>>,
) -> ApiResult<Option<Option<String>>> {
    local_path.map(normalize_optional_local_path).transpose()
}

fn repo_name_from_remote_url(remote_url: &str) -> String {
    let segment = remote_url
        .trim()
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(remote_url);
    segment.strip_suffix(".git").unwrap_or(segment).to_owned()
}

fn work_mode_domain(work_mode: api_types::WorkMode) -> WorkMode {
    match work_mode {
        api_types::WorkMode::DirectMerge => WorkMode::DirectMerge,
        api_types::WorkMode::PullRequest => WorkMode::PullRequest,
    }
}
