//! Real GitHub REST client for the PR loop (plan node P0).
//!
//! Contract (plan «Congelado 6»):
//! - The token only lives in memory inside a [`Secret`], is sent only in the
//!   `Authorization: Bearer` header, and never appears in URLs, logs, errors
//!   or `Debug` output. Error text is built from classification plus a
//!   truncated, token-scrubbed provider message.
//! - Redirects are never followed, so the bearer header cannot leak to
//!   another host.
//! - Pagination uses page numbers derived from the `Link` header; the client
//!   never follows provider supplied URLs.
//! - Rate limits (429, or 403 carrying `Retry-After` / exhausted
//!   `X-RateLimit-Remaining`) are a typed retryable error; the client never
//!   sleeps on its own.
//! - Production `base_url` validation uses the credential service SSRF policy
//!   ([`DefaultPrProviderFactory`]). Tests inject [`PrProviderFactory`]
//!   implementations that point at a local server; there is no configuration
//!   switch that relaxes the policy.

use crate::credential_service::{validate_provider_base_url, Secret};
use crate::pr_service::{
    PrCreateRequest, PrObservation, PrProvider, PrProviderError, PrRecord, RemotePrStatus,
};
use crate::ServiceError;
use async_trait::async_trait;
use db::{PrMetadata, RemotePrAdmission};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_GITHUB_API_BASE_URL: &str = "https://api.github.com";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PER_PAGE: u32 = 100;
const MAX_ERROR_MESSAGE_CHARS: usize = 200;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_RATE_LIMIT_BACKOFF_SECONDS: u64 = 60;
const MAX_RATE_LIMIT_BACKOFF_SECONDS: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubClientError {
    /// `base_url` or repository identity is unusable. Nothing was sent.
    InvalidConfig(String),
    /// 401, or 403 that is not a rate limit. The credential cannot be used.
    Unauthorized {
        status: u16,
    },
    NotFound,
    /// 4xx (typically 422/409) after which the provider guarantees no effect.
    Rejected {
        status: u16,
        message: String,
    },
    /// 403/429 rate limit. Retryable after the given backoff.
    RateLimited {
        retry_after_seconds: u64,
    },
    /// 5xx. The effect of a mutating request is unknown.
    Server {
        status: u16,
    },
    /// Connection could not be established; the request was never sent.
    Connect,
    /// Timeout or I/O failure after the request may have been sent.
    Transport {
        timeout: bool,
    },
    /// The response body was not the expected shape or was too large.
    Decode,
}

impl GitHubClientError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Server { .. } | Self::Connect | Self::Transport { .. }
        )
    }

    /// Classify for `find_or_create`. `mutating` requests that may have taken
    /// effect become `OutcomeUnknown`; reads and never-sent requests are
    /// `Unavailable`; definitive provider refusals are `DefinitiveRejection`.
    pub fn into_pr_error(self, mutating: bool) -> PrProviderError {
        let text = self.to_string();
        match self {
            Self::Rejected { .. } | Self::NotFound => PrProviderError::DefinitiveRejection(text),
            Self::Server { .. } | Self::Transport { .. } | Self::Decode if mutating => {
                PrProviderError::OutcomeUnknown(text)
            }
            // A rate-limited, unauthorized or never-sent request had no effect.
            _ => PrProviderError::Unavailable(text),
        }
    }
}

impl std::fmt::Display for GitHubClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(reason) => {
                write!(f, "invalid GitHub client configuration: {reason}")
            }
            Self::Unauthorized { status } => {
                write!(f, "GitHub rejected the credential (HTTP {status})")
            }
            Self::NotFound => write!(f, "GitHub resource not found"),
            Self::Rejected { status, message } => {
                write!(f, "GitHub rejected the request (HTTP {status}): {message}")
            }
            Self::RateLimited {
                retry_after_seconds,
            } => write!(f, "GitHub rate limit; retry after {retry_after_seconds}s"),
            Self::Server { status } => write!(f, "GitHub server error (HTTP {status})"),
            Self::Connect => write!(f, "could not connect to GitHub"),
            Self::Transport { timeout: true } => write!(f, "GitHub request timed out"),
            Self::Transport { timeout: false } => write!(f, "GitHub transport failure"),
            Self::Decode => write!(f, "GitHub response could not be decoded"),
        }
    }
}

impl std::error::Error for GitHubClientError {}

impl From<GitHubClientError> for ServiceError {
    fn from(error: GitHubClientError) -> Self {
        match error {
            GitHubClientError::RateLimited {
                retry_after_seconds,
            } => ServiceError::RateLimited {
                retry_after_seconds,
            },
            GitHubClientError::NotFound => ServiceError::not_found("GitHub resource", "remote"),
            GitHubClientError::Unauthorized { .. } => ServiceError::AuthorizationDenied {
                message: error.to_string(),
            },
            GitHubClientError::Rejected { .. } | GitHubClientError::InvalidConfig(_) => {
                ServiceError::invalid_operation(error.to_string())
            }
            other => ServiceError::Domain(other.to_string()),
        }
    }
}

type ClientResult<T> = std::result::Result<T, GitHubClientError>;

/// `owner/name` of a GitHub repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepoRef {
    pub owner: String,
    pub name: String,
}

impl GitHubRepoRef {
    /// Accepts `https://host/owner/name(.git)`, `git@host:owner/name(.git)`,
    /// `host/owner/name` and `owner/name`.
    pub fn parse(identity: &str) -> ClientResult<Self> {
        let trimmed = identity.trim().trim_end_matches('/');
        let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
        let path = if let Some((_, rest)) = trimmed.split_once("://") {
            rest.split_once('/').map(|(_, path)| path)
        } else if let Some((_, rest)) = trimmed.split_once(':').filter(|_| trimmed.contains('@')) {
            Some(rest)
        } else {
            Some(trimmed)
        };
        let segments: Vec<&str> = path
            .unwrap_or_default()
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect();
        let [.., owner, name] = segments.as_slice() else {
            return Err(GitHubClientError::InvalidConfig(
                "repository identity is not owner/name".to_owned(),
            ));
        };
        let valid = |part: &str| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        if !valid(owner) || !valid(name) {
            return Err(GitHubClientError::InvalidConfig(
                "repository identity has invalid characters".to_owned(),
            ));
        }
        Ok(Self {
            owner: (*owner).to_owned(),
            name: (*name).to_owned(),
        })
    }

    fn path(&self) -> String {
        format!("/repos/{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubPullRequest {
    pub number: u64,
    pub html_url: String,
    /// GitHub `open` or `closed`; see `merged` for the merged distinction.
    pub state: String,
    pub merged: bool,
    pub draft: bool,
    pub head_sha: String,
    pub head_ref: String,
    pub base_ref: String,
    pub merge_commit_sha: Option<String>,
    /// `None` while GitHub is still computing mergeability.
    pub mergeable: Option<bool>,
    pub mergeable_state: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePullRequest {
    pub title: String,
    pub body: Option<String>,
    pub head: String,
    pub base: String,
    pub draft: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubReviewComment {
    pub id: u64,
    pub author_login: Option<String>,
    pub author_association: Option<String>,
    pub body: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub commit_id: Option<String>,
    pub in_reply_to_id: Option<u64>,
    pub html_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubIssueComment {
    pub id: u64,
    pub author_login: Option<String>,
    pub author_association: Option<String>,
    pub body: String,
    pub html_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubReview {
    pub id: u64,
    pub author_login: Option<String>,
    pub author_association: Option<String>,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED`, `PENDING`.
    pub state: String,
    pub body: String,
    pub commit_id: Option<String>,
    pub html_url: Option<String>,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubCompare {
    /// `ahead`, `behind`, `identical` or `diverged`.
    pub status: String,
    pub ahead_by: u64,
    pub behind_by: u64,
}

/// One page of a paginated listing. `next_page` is the 1-based page to pass
/// back to continue, or `None` when the listing is exhausted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_page: Option<u32>,
}

#[derive(Deserialize)]
struct WireUser {
    login: Option<String>,
}

#[derive(Deserialize)]
struct WireRef {
    #[serde(rename = "ref")]
    name: String,
    sha: String,
}

#[derive(Deserialize)]
struct WirePull {
    number: u64,
    html_url: String,
    state: String,
    merged: Option<bool>,
    merged_at: Option<String>,
    draft: Option<bool>,
    head: WireRef,
    base: WireRef,
    merge_commit_sha: Option<String>,
    mergeable: Option<bool>,
    mergeable_state: Option<String>,
    updated_at: Option<String>,
}

impl From<WirePull> for GitHubPullRequest {
    fn from(wire: WirePull) -> Self {
        Self {
            number: wire.number,
            html_url: wire.html_url,
            state: wire.state,
            merged: wire.merged.unwrap_or(false) || wire.merged_at.is_some(),
            draft: wire.draft.unwrap_or(false),
            head_sha: wire.head.sha,
            head_ref: wire.head.name,
            base_ref: wire.base.name,
            merge_commit_sha: wire.merge_commit_sha,
            mergeable: wire.mergeable,
            mergeable_state: wire.mergeable_state,
            updated_at: wire.updated_at,
        }
    }
}

#[derive(Deserialize)]
struct WireReviewComment {
    id: u64,
    user: Option<WireUser>,
    author_association: Option<String>,
    body: Option<String>,
    path: Option<String>,
    line: Option<u64>,
    commit_id: Option<String>,
    in_reply_to_id: Option<u64>,
    html_url: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct WireIssueComment {
    id: u64,
    user: Option<WireUser>,
    author_association: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct WireReview {
    id: u64,
    user: Option<WireUser>,
    author_association: Option<String>,
    state: String,
    body: Option<String>,
    commit_id: Option<String>,
    html_url: Option<String>,
    submitted_at: Option<String>,
}

#[derive(Deserialize)]
struct WireCompare {
    status: String,
    ahead_by: u64,
    behind_by: u64,
}

#[derive(Deserialize)]
struct WireError {
    message: Option<String>,
}

impl From<WireIssueComment> for GitHubIssueComment {
    fn from(wire: WireIssueComment) -> Self {
        Self {
            id: wire.id,
            author_login: wire.user.and_then(|user| user.login),
            author_association: wire.author_association,
            body: wire.body.unwrap_or_default(),
            html_url: wire.html_url,
            created_at: wire.created_at,
            updated_at: wire.updated_at,
        }
    }
}

/// HTTP client bound to one base URL and one in-memory token.
#[derive(Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    base_url: String,
    token: Secret,
}

impl std::fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl GitHubClient {
    /// Build a client for an already validated base URL. Production code
    /// obtains it through [`DefaultPrProviderFactory`], which applies the
    /// SSRF policy; this constructor does not resolve or filter addresses and
    /// is `pub(crate)` so only factories and tests can bypass validation.
    pub(crate) fn new_unvalidated(base_url: &str, token: Secret) -> ClientResult<Self> {
        let base_url = base_url.trim().trim_end_matches('/').to_owned();
        if base_url.is_empty() {
            return Err(GitHubClientError::InvalidConfig(
                "base_url is empty".to_owned(),
            ));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| GitHubClientError::InvalidConfig("HTTP client".to_owned()))?;
        Ok(Self {
            http,
            base_url,
            token,
        })
    }

    fn headers(&self) -> ClientResult<HeaderMap> {
        let mut headers = HeaderMap::new();
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {}", self.token.expose()))
                .map_err(|_| GitHubClientError::InvalidConfig("credential format".to_owned()))?;
        authorization.set_sensitive(true);
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(USER_AGENT, HeaderValue::from_static("forge"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "x-github-api-version",
            HeaderValue::from_static("2022-11-28"),
        );
        Ok(headers)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> ClientResult<(Vec<u8>, HeaderMap)> {
        let mut request = self
            .http
            .request(method, format!("{}{}", self.base_url, path))
            .headers(self.headers()?)
            .query(query);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| {
            if error.is_connect() {
                GitHubClientError::Connect
            } else {
                GitHubClientError::Transport {
                    timeout: error.is_timeout(),
                }
            }
        })?;
        let status = response.status();
        let headers = response.headers().clone();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(GitHubClientError::Decode);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| GitHubClientError::Transport {
                timeout: error.is_timeout(),
            })?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(GitHubClientError::Decode);
        }
        if status.is_success() {
            return Ok((bytes.to_vec(), headers));
        }
        Err(self.classify_failure(status, &headers, &bytes))
    }

    fn classify_failure(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
    ) -> GitHubClientError {
        let message = serde_json::from_slice::<WireError>(body)
            .ok()
            .and_then(|wire| wire.message)
            .unwrap_or_default();
        let message = self.redact(&message);
        let code = status.as_u16();
        let header_u64 = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
        };
        let retry_after = header_u64("retry-after");
        let exhausted = header_u64("x-ratelimit-remaining") == Some(0);
        let rate_message = message.to_ascii_lowercase().contains("rate limit");
        if status == StatusCode::TOO_MANY_REQUESTS
            || (status == StatusCode::FORBIDDEN
                && (retry_after.is_some() || exhausted || rate_message))
        {
            let reset_wait = header_u64("x-ratelimit-reset")
                .map(|reset| reset.saturating_sub(chrono::Utc::now().timestamp().max(0) as u64));
            let seconds = retry_after
                .or(if exhausted { reset_wait } else { None })
                .unwrap_or(DEFAULT_RATE_LIMIT_BACKOFF_SECONDS)
                .clamp(1, MAX_RATE_LIMIT_BACKOFF_SECONDS);
            return GitHubClientError::RateLimited {
                retry_after_seconds: seconds,
            };
        }
        match code {
            401 | 403 => GitHubClientError::Unauthorized { status: code },
            404 => GitHubClientError::NotFound,
            500..=599 => GitHubClientError::Server { status: code },
            _ => GitHubClientError::Rejected {
                status: code,
                message,
            },
        }
    }

    /// Truncate provider text and scrub the token if it was echoed back.
    fn redact(&self, text: &str) -> String {
        let token = self.token.expose();
        let scrubbed = if token.is_empty() {
            text.to_owned()
        } else {
            text.replace(token, "[redacted]")
        };
        scrubbed.chars().take(MAX_ERROR_MESSAGE_CHARS).collect()
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> ClientResult<(T, HeaderMap)> {
        let (bytes, headers) = self.send(Method::GET, path, query, None).await?;
        let value = serde_json::from_slice(&bytes).map_err(|_| GitHubClientError::Decode)?;
        Ok((value, headers))
    }

    fn next_page(headers: &HeaderMap, current: u32) -> Option<u32> {
        let has_next = headers
            .get_all("link")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| value.contains("rel=\"next\""));
        has_next.then_some(current.saturating_add(1))
    }

    /// Find a PR by head branch (same-repository head) and optional base,
    /// across all states. When several match, an open PR wins, otherwise the
    /// most recently listed one.
    pub async fn find_pull_by_head(
        &self,
        repo: &GitHubRepoRef,
        head_branch: &str,
        base_branch: Option<&str>,
    ) -> ClientResult<Option<GitHubPullRequest>> {
        let mut query = vec![
            ("head", format!("{}:{}", repo.owner, head_branch)),
            ("state", "all".to_owned()),
            ("sort", "created".to_owned()),
            ("direction", "desc".to_owned()),
            ("per_page", PER_PAGE.to_string()),
        ];
        if let Some(base) = base_branch {
            query.push(("base", base.to_owned()));
        }
        let (pulls, _): (Vec<WirePull>, _) = self
            .get_json(&format!("{}/pulls", repo.path()), &query)
            .await?;
        let mut pulls: Vec<GitHubPullRequest> = pulls
            .into_iter()
            .map(GitHubPullRequest::from)
            .filter(|pull| pull.head_ref == head_branch)
            .filter(|pull| base_branch.map_or(true, |base| pull.base_ref == base))
            .collect();
        if let Some(position) = pulls.iter().position(|pull| pull.state == "open") {
            return Ok(Some(pulls.swap_remove(position)));
        }
        Ok(pulls.into_iter().next())
    }

    pub async fn create_pull(
        &self,
        repo: &GitHubRepoRef,
        request: &CreatePullRequest,
    ) -> ClientResult<GitHubPullRequest> {
        let mut payload = json!({
            "title": request.title,
            "head": request.head,
            "base": request.base,
            "draft": request.draft,
        });
        if let Some(body) = &request.body {
            payload["body"] = json!(body);
        }
        let (bytes, _) = self
            .send(
                Method::POST,
                &format!("{}/pulls", repo.path()),
                &[],
                Some(payload),
            )
            .await?;
        let wire: WirePull =
            serde_json::from_slice(&bytes).map_err(|_| GitHubClientError::Decode)?;
        Ok(wire.into())
    }

    pub async fn get_pull(
        &self,
        repo: &GitHubRepoRef,
        number: u64,
    ) -> ClientResult<GitHubPullRequest> {
        let (wire, _): (WirePull, _) = self
            .get_json(&format!("{}/pulls/{number}", repo.path()), &[])
            .await?;
        Ok(wire.into())
    }

    /// Inline review comments, oldest first. `since` is an ISO-8601 instant
    /// applied by GitHub to `updated_at`.
    pub async fn list_review_comments(
        &self,
        repo: &GitHubRepoRef,
        number: u64,
        since: Option<&str>,
        page: u32,
    ) -> ClientResult<Page<GitHubReviewComment>> {
        let mut query = Self::list_query(since, page);
        query.push(("sort", "created".to_owned()));
        query.push(("direction", "asc".to_owned()));
        let (items, headers): (Vec<WireReviewComment>, _) = self
            .get_json(&format!("{}/pulls/{number}/comments", repo.path()), &query)
            .await?;
        Ok(Page {
            items: items
                .into_iter()
                .map(|wire| GitHubReviewComment {
                    id: wire.id,
                    author_login: wire.user.and_then(|user| user.login),
                    author_association: wire.author_association,
                    body: wire.body.unwrap_or_default(),
                    path: wire.path,
                    line: wire.line,
                    commit_id: wire.commit_id,
                    in_reply_to_id: wire.in_reply_to_id,
                    html_url: wire.html_url,
                    created_at: wire.created_at,
                    updated_at: wire.updated_at,
                })
                .collect(),
            next_page: Self::next_page(&headers, page.max(1)),
        })
    }

    /// Conversation comments on the PR (issue comments API).
    pub async fn list_issue_comments(
        &self,
        repo: &GitHubRepoRef,
        number: u64,
        since: Option<&str>,
        page: u32,
    ) -> ClientResult<Page<GitHubIssueComment>> {
        let query = Self::list_query(since, page);
        let (items, headers): (Vec<WireIssueComment>, _) = self
            .get_json(&format!("{}/issues/{number}/comments", repo.path()), &query)
            .await?;
        Ok(Page {
            items: items.into_iter().map(Into::into).collect(),
            next_page: Self::next_page(&headers, page.max(1)),
        })
    }

    /// Reviews, oldest first. GitHub has no `since` filter for reviews, so
    /// entries submitted at or before `since` are dropped client-side (ISO-8601
    /// UTC strings compare lexicographically). Pending reviews have no
    /// `submitted_at` and are always kept.
    pub async fn list_reviews(
        &self,
        repo: &GitHubRepoRef,
        number: u64,
        since: Option<&str>,
        page: u32,
    ) -> ClientResult<Page<GitHubReview>> {
        let query = [
            ("per_page", PER_PAGE.to_string()),
            ("page", page.max(1).to_string()),
        ];
        let (items, headers): (Vec<WireReview>, _) = self
            .get_json(&format!("{}/pulls/{number}/reviews", repo.path()), &query)
            .await?;
        Ok(Page {
            items: items
                .into_iter()
                .filter(|wire| match (since, wire.submitted_at.as_deref()) {
                    (Some(since), Some(submitted)) => submitted > since,
                    _ => true,
                })
                .map(|wire| GitHubReview {
                    id: wire.id,
                    author_login: wire.user.and_then(|user| user.login),
                    author_association: wire.author_association,
                    state: wire.state,
                    body: wire.body.unwrap_or_default(),
                    commit_id: wire.commit_id,
                    html_url: wire.html_url,
                    submitted_at: wire.submitted_at,
                })
                .collect(),
            next_page: Self::next_page(&headers, page.max(1)),
        })
    }

    fn list_query(since: Option<&str>, page: u32) -> Vec<(&'static str, String)> {
        let mut query = vec![
            ("per_page", PER_PAGE.to_string()),
            ("page", page.max(1).to_string()),
        ];
        if let Some(since) = since {
            query.push(("since", since.to_owned()));
        }
        query
    }

    pub async fn create_issue_comment(
        &self,
        repo: &GitHubRepoRef,
        number: u64,
        body: &str,
    ) -> ClientResult<GitHubIssueComment> {
        let (bytes, _) = self
            .send(
                Method::POST,
                &format!("{}/issues/{number}/comments", repo.path()),
                &[],
                Some(json!({ "body": body })),
            )
            .await?;
        let wire: WireIssueComment =
            serde_json::from_slice(&bytes).map_err(|_| GitHubClientError::Decode)?;
        Ok(wire.into())
    }

    /// Compare two refs (`base...head`), e.g. to detect a stale head.
    pub async fn compare_commits(
        &self,
        repo: &GitHubRepoRef,
        base: &str,
        head: &str,
    ) -> ClientResult<GitHubCompare> {
        let (wire, _): (WireCompare, _) = self
            .get_json(
                &format!("{}/compare/{base}...{head}", repo.path()),
                &[("per_page", "1".to_owned())],
            )
            .await?;
        Ok(GitHubCompare {
            status: wire.status,
            ahead_by: wire.ahead_by,
            behind_by: wire.behind_by,
        })
    }
}

/// `PrProvider` backed by [`GitHubClient`].
#[derive(Debug, Clone)]
pub struct GitHubPrProvider {
    client: GitHubClient,
}

impl GitHubPrProvider {
    pub fn new(client: GitHubClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &GitHubClient {
        &self.client
    }

    fn record(&self, request_repo: &str, pull: GitHubPullRequest) -> PrRecord {
        let status = pull_status(request_repo, &pull);
        PrRecord {
            provider_pr_id: pull.number.to_string(),
            pr_url: Some(pull.html_url),
            status,
        }
    }
}

fn pull_status(remote_repo_identity: &str, pull: &GitHubPullRequest) -> RemotePrStatus {
    let state = if pull.merged {
        "merged"
    } else {
        pull.state.as_str()
    };
    let observation = PrObservation {
        provider_event_id: format!("github-pr:{}:{}:{}", pull.number, state, pull.head_sha),
        remote_repo_identity: remote_repo_identity.to_owned(),
        source_branch: pull.head_ref.clone(),
        target_branch: pull.base_ref.clone(),
        head_sha: pull.head_sha.clone(),
        merged_commit_sha: pull.merged.then(|| pull.merge_commit_sha.clone()).flatten(),
    };
    if pull.merged {
        RemotePrStatus::Merged(observation)
    } else if pull.state == "closed" {
        RemotePrStatus::Closed(observation)
    } else {
        RemotePrStatus::Open(observation)
    }
}

#[async_trait]
impl PrProvider for GitHubPrProvider {
    async fn find_pr(
        &self,
        request: &PrCreateRequest,
    ) -> std::result::Result<Option<PrRecord>, PrProviderError> {
        let repo = GitHubRepoRef::parse(&request.repo_remote_url)
            .map_err(|error| error.into_pr_error(false))?;
        let found = self
            .client
            .find_pull_by_head(&repo, &request.source_branch, Some(&request.target_branch))
            .await
            .map_err(|error| error.into_pr_error(false))?;
        // The admission freezes the source SHA; a PR on the same branches at a
        // different head is not the admitted subject.
        Ok(found
            .filter(|pull| pull.head_sha == request.source_sha)
            .map(|pull| self.record(&request.repo_remote_url, pull)))
    }

    async fn create_pr(
        &self,
        request: PrCreateRequest,
    ) -> std::result::Result<PrRecord, PrProviderError> {
        let repo = GitHubRepoRef::parse(&request.repo_remote_url)
            .map_err(|error| error.into_pr_error(true))?;
        let pull = self
            .client
            .create_pull(
                &repo,
                &CreatePullRequest {
                    title: request.title.clone(),
                    body: request.body.clone(),
                    head: request.source_branch.clone(),
                    base: request.target_branch.clone(),
                    draft: false,
                },
            )
            .await
            .map_err(|error| error.into_pr_error(true))?;
        Ok(self.record(&request.repo_remote_url, pull))
    }

    async fn get_pr_status(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
    ) -> std::result::Result<RemotePrStatus, PrProviderError> {
        let repo = GitHubRepoRef::parse(&admission.remote_repo_identity)
            .map_err(|error| error.into_pr_error(false))?;
        let number = metadata
            .provider_pr_id
            .as_deref()
            .and_then(|id| id.parse::<u64>().ok())
            .ok_or_else(|| {
                PrProviderError::Unavailable("PR metadata has no GitHub PR number".to_owned())
            })?;
        let pull = self
            .client
            .get_pull(&repo, number)
            .await
            .map_err(|error| error.into_pr_error(false))?;
        Ok(pull_status(&admission.remote_repo_identity, &pull))
    }
}

/// Seam that turns a frozen admission plus an in-memory token into a provider.
/// Production uses [`DefaultPrProviderFactory`]; tests inject a factory that
/// targets a local server. There is deliberately no configuration to relax
/// the SSRF policy.
#[async_trait]
pub trait PrProviderFactory: Send + Sync {
    async fn build(
        &self,
        provider_type: &str,
        base_url: Option<&str>,
        token: Secret,
    ) -> std::result::Result<Arc<dyn PrProvider>, PrProviderError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultPrProviderFactory;

#[async_trait]
impl PrProviderFactory for DefaultPrProviderFactory {
    async fn build(
        &self,
        provider_type: &str,
        base_url: Option<&str>,
        token: Secret,
    ) -> std::result::Result<Arc<dyn PrProvider>, PrProviderError> {
        if provider_type != "github" {
            return Err(PrProviderError::Unavailable(format!(
                "unsupported frozen provider type: {provider_type}"
            )));
        }
        let resolved = match base_url.map(str::trim).filter(|url| !url.is_empty()) {
            None => DEFAULT_GITHUB_API_BASE_URL.to_owned(),
            Some(url) => {
                let trimmed = url.trim_end_matches('/');
                // The web host is not the API host for github.com.
                if trimmed.eq_ignore_ascii_case("https://github.com") {
                    DEFAULT_GITHUB_API_BASE_URL.to_owned()
                } else {
                    validate_provider_base_url(trimmed)
                        .await
                        .map_err(|reason| {
                            PrProviderError::Unavailable(format!(
                                "provider base_url rejected: {reason}"
                            ))
                        })?;
                    trimmed.to_owned()
                }
            }
        };
        let client = GitHubClient::new_unvalidated(&resolved, token)
            .map_err(|error| error.into_pr_error(false))?;
        Ok(Arc::new(GitHubPrProvider::new(client)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const TOKEN: &str = "ghp_supersecret_token_value";

    #[derive(Clone, Debug)]
    struct Captured {
        request_line: String,
        headers: String,
        body: String,
    }

    struct MockResponse {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: String,
    }

    fn reply(status: u16, body: &str) -> MockResponse {
        MockResponse {
            status,
            headers: Vec::new(),
            body: body.to_owned(),
        }
    }

    struct MockServer {
        base_url: String,
        captured: Arc<Mutex<Vec<Captured>>>,
    }

    /// Minimal in-process HTTP/1.1 server answering queued responses in order.
    async fn mock_server(responses: Vec<MockResponse>) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let base_url = format!("http://{}", listener.local_addr().expect("addr"));
        let captured = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
        let captured_task = Arc::clone(&captured);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut data = Vec::new();
                let mut buffer = [0_u8; 4096];
                let (head_end, content_length) = loop {
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        break (data.len(), 0);
                    }
                    data.extend_from_slice(&buffer[..read]);
                    if let Some(position) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&data[..position]).to_lowercase();
                        let length = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (position + 4, length);
                    }
                };
                while data.len() < head_end + content_length {
                    let read = socket.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    data.extend_from_slice(&buffer[..read]);
                }
                let head = String::from_utf8_lossy(&data[..head_end.min(data.len())]).to_string();
                let mut lines = head.lines();
                let request_line = lines.next().unwrap_or_default().to_owned();
                captured_task.lock().expect("captured").push(Captured {
                    request_line,
                    headers: lines.collect::<Vec<_>>().join("\n"),
                    body: String::from_utf8_lossy(&data[head_end.min(data.len())..]).to_string(),
                });
                let response = queue
                    .lock()
                    .expect("queue")
                    .pop_front()
                    .unwrap_or_else(|| reply(500, "{}"));
                let mut text = format!(
                    "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                    response.status,
                    response.body.len()
                );
                for (name, value) in &response.headers {
                    text.push_str(&format!("{name}: {value}\r\n"));
                }
                text.push_str("\r\n");
                text.push_str(&response.body);
                let _ = socket.write_all(text.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        MockServer { base_url, captured }
    }

    fn client(server: &MockServer) -> GitHubClient {
        GitHubClient::new_unvalidated(&server.base_url, Secret::new(TOKEN.to_owned()))
            .expect("client")
    }

    fn repo() -> GitHubRepoRef {
        GitHubRepoRef::parse("https://github.com/acme/widgets.git").expect("repo")
    }

    fn pull_json(number: u64, state: &str, merged: bool, sha: &str) -> String {
        format!(
            r#"{{"number":{number},"html_url":"https://github.com/acme/widgets/pull/{number}","state":"{state}","merged":{merged},"draft":false,"head":{{"ref":"forge/task","sha":"{sha}"}},"base":{{"ref":"main","sha":"b"}},"merge_commit_sha":"mc1","mergeable":true,"mergeable_state":"clean","updated_at":"2026-01-01T00:00:00Z"}}"#
        )
    }

    #[test]
    fn pr_github_client_parses_repo_identities() {
        for identity in [
            "https://github.com/acme/widgets.git",
            "https://github.com/acme/widgets/",
            "git@github.com:acme/widgets.git",
            "github.com/acme/widgets",
            "acme/widgets",
        ] {
            let parsed = GitHubRepoRef::parse(identity).expect(identity);
            assert_eq!(parsed.owner, "acme");
            assert_eq!(parsed.name, "widgets");
        }
        assert!(GitHubRepoRef::parse("widgets").is_err());
        assert!(GitHubRepoRef::parse("https://github.com/acme/../x y").is_err());
    }

    #[tokio::test]
    async fn pr_github_client_sends_bearer_header_never_in_url() {
        let server = mock_server(vec![reply(200, &pull_json(7, "open", false, "abc"))]).await;
        let pull = client(&server).get_pull(&repo(), 7).await.expect("pull");
        assert_eq!(pull.number, 7);
        assert_eq!(pull.mergeable, Some(true));
        assert!(!pull.merged);
        let captured = server.captured.lock().expect("captured");
        let request = &captured[0];
        assert!(request
            .request_line
            .starts_with("GET /repos/acme/widgets/pulls/7 "));
        assert!(!request.request_line.contains(TOKEN));
        let headers = request.headers.to_lowercase();
        assert!(headers.contains(&format!("authorization: bearer {}", TOKEN.to_lowercase())));
        assert!(headers.contains("user-agent: forge"));
    }

    #[tokio::test]
    async fn pr_github_client_find_by_head_prefers_open_and_filters_branches() {
        let body = format!(
            "[{},{}]",
            pull_json(3, "closed", false, "old"),
            pull_json(4, "open", false, "abc")
        );
        let server = mock_server(vec![reply(200, &body)]).await;
        let found = client(&server)
            .find_pull_by_head(&repo(), "forge/task", Some("main"))
            .await
            .expect("find")
            .expect("some");
        assert_eq!(found.number, 4);
        let request = server.captured.lock().expect("captured")[0]
            .request_line
            .clone();
        assert!(request.contains("head=acme%3Aforge%2Ftask"), "{request}");
        assert!(request.contains("state=all"));
    }

    #[tokio::test]
    async fn pr_github_client_provider_find_requires_admitted_sha() {
        let server = mock_server(vec![
            reply(200, &format!("[{}]", pull_json(4, "open", false, "other"))),
            reply(200, &format!("[{}]", pull_json(4, "open", false, "abc"))),
        ])
        .await;
        let provider = GitHubPrProvider::new(client(&server));
        let request = PrCreateRequest {
            repo_remote_url: "https://github.com/acme/widgets.git".to_owned(),
            source_branch: "forge/task".to_owned(),
            target_branch: "main".to_owned(),
            source_sha: "abc".to_owned(),
            idempotency_key: "op1".to_owned(),
            title: "t".to_owned(),
            body: None,
        };
        assert!(provider.find_pr(&request).await.expect("find").is_none());
        let record = provider
            .find_pr(&request)
            .await
            .expect("find")
            .expect("record");
        assert_eq!(record.provider_pr_id, "4");
        assert!(matches!(record.status, RemotePrStatus::Open(_)));
    }

    #[tokio::test]
    async fn pr_github_client_create_posts_json_payload() {
        let server = mock_server(vec![reply(201, &pull_json(9, "open", false, "abc"))]).await;
        let pull = client(&server)
            .create_pull(
                &repo(),
                &CreatePullRequest {
                    title: "Title".to_owned(),
                    body: Some("Body".to_owned()),
                    head: "forge/task".to_owned(),
                    base: "main".to_owned(),
                    draft: true,
                },
            )
            .await
            .expect("create");
        assert_eq!(pull.number, 9);
        let captured = server.captured.lock().expect("captured");
        assert!(captured[0]
            .request_line
            .starts_with("POST /repos/acme/widgets/pulls "));
        let payload: serde_json::Value = serde_json::from_str(&captured[0].body).expect("json");
        assert_eq!(payload["head"], "forge/task");
        assert_eq!(payload["base"], "main");
        assert_eq!(payload["draft"], true);
        assert_eq!(payload["title"], "Title");
        assert_eq!(payload["body"], "Body");
    }

    #[tokio::test]
    async fn pr_github_client_status_maps_merged_and_closed() {
        let server = mock_server(vec![
            reply(200, &pull_json(1, "closed", true, "abc")),
            reply(200, &pull_json(1, "closed", false, "abc")),
        ])
        .await;
        let merged = pull_status("r", &client(&server).get_pull(&repo(), 1).await.expect("a"));
        let RemotePrStatus::Merged(observation) = merged else {
            panic!("expected merged");
        };
        assert_eq!(observation.merged_commit_sha.as_deref(), Some("mc1"));
        let closed = pull_status("r", &client(&server).get_pull(&repo(), 1).await.expect("b"));
        assert!(matches!(closed, RemotePrStatus::Closed(_)));
    }

    #[tokio::test]
    async fn pr_github_client_lists_paginate_and_filter_since() {
        let comments = r#"[{"id":1,"user":{"login":"alice"},"author_association":"MEMBER","body":"fix","path":"a.rs","line":3,"commit_id":"abc","in_reply_to_id":null,"html_url":"u","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}]"#;
        let issue = r#"[{"id":2,"user":null,"author_association":"NONE","body":null,"html_url":"u","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}]"#;
        let reviews = r#"[{"id":5,"user":{"login":"bob"},"author_association":"OWNER","state":"APPROVED","body":"ok","commit_id":"abc","html_url":"u","submitted_at":"2026-01-01T00:00:00Z"},{"id":6,"user":{"login":"bob"},"author_association":"OWNER","state":"CHANGES_REQUESTED","body":"no","commit_id":"abc","html_url":"u","submitted_at":"2026-03-01T00:00:00Z"}]"#;
        let mut first = reply(200, comments);
        first.headers.push((
            "link",
            "<https://api.github.com/x?page=2>; rel=\"next\"".to_owned(),
        ));
        let server = mock_server(vec![first, reply(200, issue), reply(200, reviews)]).await;
        let client = client(&server);
        let page = client
            .list_review_comments(&repo(), 7, Some("2025-12-01T00:00:00Z"), 1)
            .await
            .expect("review comments");
        assert_eq!(page.items[0].author_login.as_deref(), Some("alice"));
        assert_eq!(page.next_page, Some(2));
        let page = client
            .list_issue_comments(&repo(), 7, None, 1)
            .await
            .expect("issue comments");
        assert_eq!(page.items[0].body, "");
        assert_eq!(page.next_page, None);
        let page = client
            .list_reviews(&repo(), 7, Some("2026-02-01T00:00:00Z"), 1)
            .await
            .expect("reviews");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].state, "CHANGES_REQUESTED");
        let captured = server.captured.lock().expect("captured");
        assert!(captured[0].request_line.contains("/pulls/7/comments"));
        assert!(captured[0]
            .request_line
            .contains("since=2025-12-01T00%3A00%3A00Z"));
        assert!(captured[1].request_line.contains("/issues/7/comments"));
        assert!(captured[2].request_line.contains("/pulls/7/reviews"));
    }

    #[tokio::test]
    async fn pr_github_client_creates_issue_comment_and_compares() {
        let comment = r#"{"id":11,"user":{"login":"forge-bot"},"author_association":"OWNER","body":"hi","html_url":"u","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}"#;
        let compare = r#"{"status":"ahead","ahead_by":2,"behind_by":0}"#;
        let server = mock_server(vec![reply(201, comment), reply(200, compare)]).await;
        let client = client(&server);
        let created = client
            .create_issue_comment(&repo(), 7, "hi")
            .await
            .expect("comment");
        assert_eq!(created.id, 11);
        let diff = client
            .compare_commits(&repo(), "aaa", "bbb")
            .await
            .expect("compare");
        assert_eq!((diff.status.as_str(), diff.ahead_by), ("ahead", 2));
        let captured = server.captured.lock().expect("captured");
        assert!(captured[0]
            .request_line
            .starts_with("POST /repos/acme/widgets/issues/7/comments"));
        assert_eq!(captured[0].body, r#"{"body":"hi"}"#);
        assert!(captured[1].request_line.contains("/compare/aaa...bbb"));
    }

    #[tokio::test]
    async fn pr_github_client_rate_limits_are_typed_and_retryable() {
        let mut limited = reply(
            403,
            r#"{"message":"You have exceeded a secondary rate limit"}"#,
        );
        limited.headers.push(("retry-after", "17".to_owned()));
        let mut too_many = reply(429, "{}");
        too_many.headers.push(("retry-after", "5".to_owned()));
        let forbidden = reply(403, r#"{"message":"Resource not accessible"}"#);
        let server = mock_server(vec![limited, too_many, forbidden]).await;
        let client = client(&server);
        let error = client.get_pull(&repo(), 1).await.expect_err("limited");
        assert_eq!(
            error,
            GitHubClientError::RateLimited {
                retry_after_seconds: 17
            }
        );
        assert!(error.is_retryable());
        assert!(matches!(
            ServiceError::from(error),
            ServiceError::RateLimited {
                retry_after_seconds: 17
            }
        ));
        assert_eq!(
            client.get_pull(&repo(), 1).await.expect_err("429"),
            GitHubClientError::RateLimited {
                retry_after_seconds: 5
            }
        );
        let denied = client.get_pull(&repo(), 1).await.expect_err("403");
        assert_eq!(denied, GitHubClientError::Unauthorized { status: 403 });
        assert!(!denied.is_retryable());
    }

    #[tokio::test]
    async fn pr_github_client_classifies_errors_for_find_or_create() {
        let server = mock_server(vec![
            reply(422, r#"{"message":"Validation Failed"}"#),
            reply(502, "{}"),
            reply(502, "{}"),
        ])
        .await;
        let provider = GitHubPrProvider::new(client(&server));
        let request = PrCreateRequest {
            repo_remote_url: "acme/widgets".to_owned(),
            source_branch: "forge/task".to_owned(),
            target_branch: "main".to_owned(),
            source_sha: "abc".to_owned(),
            idempotency_key: "op1".to_owned(),
            title: "t".to_owned(),
            body: None,
        };
        assert!(matches!(
            provider.create_pr(request.clone()).await,
            Err(PrProviderError::DefinitiveRejection(_))
        ));
        assert!(matches!(
            provider.create_pr(request.clone()).await,
            Err(PrProviderError::OutcomeUnknown(_))
        ));
        assert!(matches!(
            provider.find_pr(&request).await,
            Err(PrProviderError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn pr_github_client_connect_failure_is_unavailable_not_unknown() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        drop(listener);
        let client = GitHubClient::new_unvalidated(
            &format!("http://{address}"),
            Secret::new(TOKEN.to_owned()),
        )
        .expect("client");
        let error = client.get_pull(&repo(), 1).await.expect_err("refused");
        assert_eq!(error, GitHubClientError::Connect);
        assert!(matches!(
            error.into_pr_error(true),
            PrProviderError::Unavailable(_)
        ));
    }

    #[tokio::test]
    async fn pr_github_client_errors_and_debug_never_contain_token() {
        let echoed = format!(
            r#"{{"message":"bad credentials {TOKEN} {}"}}"#,
            "x".repeat(500)
        );
        let server = mock_server(vec![reply(422, &echoed)]).await;
        let client = client(&server);
        let error = client
            .create_pull(
                &repo(),
                &CreatePullRequest {
                    title: "t".to_owned(),
                    body: None,
                    head: "h".to_owned(),
                    base: "b".to_owned(),
                    draft: false,
                },
            )
            .await
            .expect_err("rejected");
        let text = error.to_string();
        assert!(!text.contains(TOKEN), "{text}");
        assert!(text.chars().count() < 300, "message must be truncated");
        let pr_error = error.clone().into_pr_error(true).to_string();
        assert!(!pr_error.contains(TOKEN));
        assert!(!format!("{error:?}").contains(TOKEN));
        assert!(!format!("{client:?}").contains(TOKEN));
        assert!(!format!("{:?}", GitHubPrProvider::new(client.clone())).contains(TOKEN));
        assert!(!ServiceError::from(error).to_string().contains(TOKEN));
    }

    #[tokio::test]
    async fn pr_github_client_does_not_follow_redirects() {
        let mut redirect = reply(302, "{}");
        redirect
            .headers
            .push(("location", "http://127.0.0.1:1/steal".to_owned()));
        let server = mock_server(vec![redirect]).await;
        let error = client(&server)
            .get_pull(&repo(), 1)
            .await
            .expect_err("redirect");
        assert!(matches!(
            error,
            GitHubClientError::Rejected { status: 302, .. }
        ));
        assert_eq!(server.captured.lock().expect("captured").len(), 1);
    }

    #[tokio::test]
    async fn pr_github_client_default_factory_blocks_loopback_base_url() {
        let result = DefaultPrProviderFactory
            .build(
                "github",
                Some("https://127.0.0.1:9"),
                Secret::new(TOKEN.to_owned()),
            )
            .await;
        match result {
            Err(PrProviderError::Unavailable(reason)) => assert!(!reason.contains(TOKEN)),
            _ => panic!("loopback base_url must be rejected"),
        }
        let plain = DefaultPrProviderFactory
            .build(
                "github",
                Some("http://example.com"),
                Secret::new(TOKEN.to_owned()),
            )
            .await;
        assert!(matches!(plain, Err(PrProviderError::Unavailable(_))));
        let unsupported = DefaultPrProviderFactory
            .build("gitlab", None, Secret::new(TOKEN.to_owned()))
            .await;
        assert!(matches!(unsupported, Err(PrProviderError::Unavailable(_))));
    }
}
