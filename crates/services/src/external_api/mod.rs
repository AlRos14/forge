pub mod gitea;
pub mod github;

use async_trait::async_trait;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIssue {
    pub number: i64,
    pub title: String,
    pub body: Option<String>,
    pub labels: Vec<String>,
    pub state: String,
    pub html_url: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct SyncFilter {
    pub labels: Option<Vec<String>>,
    pub milestones: Option<Vec<String>>,
    pub state: Option<Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalApiError {
    #[error("missing token: env var {0} is unset or empty")]
    MissingToken(String),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("api error: {status} {body}")]
    ApiError { status: u16, body: String },
}

#[async_trait]
pub trait IssueFetcher: Send + Sync {
    async fn fetch_issues(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
        since: Option<&str>,
        sync_filter: &SyncFilter,
    ) -> Result<Vec<ExternalIssue>, ExternalApiError>;
}

pub fn resolve_token(token_secret_ref: &str) -> Result<String, ExternalApiError> {
    let token = std::env::var(token_secret_ref)
        .map_err(|_| ExternalApiError::MissingToken(token_secret_ref.to_owned()))?;
    if token.trim().is_empty() {
        return Err(ExternalApiError::MissingToken(token_secret_ref.to_owned()));
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    #[test]
    fn sync_filter_deserializes_from_json() {
        let sync_filter: super::SyncFilter = serde_json::from_str(
            r#"{"labels":["forge","ai-task"],"milestones":["v1"],"state":["open"]}"#,
        )
        .unwrap();

        assert_eq!(
            sync_filter,
            super::SyncFilter {
                labels: Some(vec!["forge".to_owned(), "ai-task".to_owned()]),
                milestones: Some(vec!["v1".to_owned()]),
                state: Some(vec!["open".to_owned()]),
            }
        );
    }

    #[test]
    fn resolve_token_returns_missing_token_for_unset_env_var() {
        let key = "FORGE_TEST_TOKEN_UNSET_74";
        std::env::remove_var(key);

        let error = super::resolve_token(key).unwrap_err();

        assert!(matches!(error, super::ExternalApiError::MissingToken(value) if value == key));
    }

    #[test]
    fn resolve_token_returns_missing_token_for_empty_env_var() {
        let key = "FORGE_TEST_TOKEN_EMPTY_74";
        std::env::set_var(key, "");

        let error = super::resolve_token(key).unwrap_err();

        assert!(matches!(error, super::ExternalApiError::MissingToken(value) if value == key));
        std::env::remove_var(key);
    }

    #[test]
    fn resolve_token_returns_valid_env_var() {
        let key = "FORGE_TEST_TOKEN_VALID_74";
        std::env::set_var(key, "token-value");

        let token = super::resolve_token(key).unwrap();

        assert_eq!(token, "token-value");
        std::env::remove_var(key);
    }
}
