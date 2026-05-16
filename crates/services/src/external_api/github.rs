use super::{ExternalApiError, ExternalIssue, IssueFetcher, SyncFilter};
use async_trait::async_trait;
use reqwest::header::{ACCEPT, AUTHORIZATION, LINK, USER_AGENT};
use serde::Deserialize;

pub struct GitHubClient;

#[derive(Deserialize)]
struct GitHubIssue {
    number: i64,
    title: String,
    body: Option<String>,
    labels: Vec<GitHubLabel>,
    state: String,
    html_url: String,
    created_at: String,
    pull_request: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GitHubLabel {
    name: String,
}

#[async_trait]
impl IssueFetcher for GitHubClient {
    async fn fetch_issues(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
        since: Option<&str>,
        sync_filter: &SyncFilter,
    ) -> Result<Vec<ExternalIssue>, ExternalApiError> {
        let client = reqwest::Client::new();
        let mut next_url = Some(format!(
            "https://api.github.com/repos/{owner}/{repo}/issues"
        ));
        let mut issues = Vec::new();
        let mut first_page = true;

        while let Some(url) = next_url.take() {
            let mut request = client
                .get(&url)
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header(USER_AGENT, "forge-agent")
                .header(ACCEPT, "application/vnd.github+json");

            if first_page {
                request = request.query(&[("state", "open"), ("per_page", "100")]);
                if let Some(since) = since {
                    request = request.query(&[("since", since)]);
                }
                first_page = false;
            }

            let response = request.send().await?;
            let status = response.status();
            if !status.is_success() {
                return Err(ExternalApiError::ApiError {
                    status: status.as_u16(),
                    body: response.text().await?,
                });
            }

            let headers = response.headers().clone();
            let page_issues = response.json::<Vec<GitHubIssue>>().await?;
            issues.extend(page_issues.into_iter().filter_map(|issue| {
                if issue.pull_request.is_some() {
                    return None;
                }

                let labels = issue
                    .labels
                    .into_iter()
                    .map(|label| label.name)
                    .collect::<Vec<_>>();
                if !matches_label_filter(&labels, sync_filter) {
                    return None;
                }

                Some(ExternalIssue {
                    number: issue.number,
                    title: issue.title,
                    body: issue.body,
                    labels,
                    state: issue.state,
                    html_url: issue.html_url,
                    created_at: issue.created_at,
                })
            }));

            next_url = headers
                .get(LINK)
                .and_then(|value| value.to_str().ok())
                .and_then(next_link_url);
        }

        Ok(issues)
    }
}

fn matches_label_filter(labels: &[String], sync_filter: &SyncFilter) -> bool {
    sync_filter.labels.as_ref().is_none_or(|filter_labels| {
        filter_labels
            .iter()
            .any(|filter_label| labels.iter().any(|label| label == filter_label))
    })
}

fn next_link_url(link_header: &str) -> Option<String> {
    link_header.split(',').find_map(|link| {
        let link = link.trim();
        if !link.contains("rel=\"next\"") {
            return None;
        }

        let start = link.find('<')? + 1;
        let end = link[start..].find('>')? + start;
        Some(link[start..end].to_owned())
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn matches_label_filter_matches_when_no_filter_set() {
        let labels = vec!["bug".to_owned()];
        let sync_filter = super::SyncFilter::default();

        assert!(super::matches_label_filter(&labels, &sync_filter));
    }

    #[test]
    fn matches_label_filter_matches_matching_label() {
        let labels = vec!["bug".to_owned(), "forge".to_owned()];
        let sync_filter = super::SyncFilter {
            labels: Some(vec!["forge".to_owned()]),
            ..super::SyncFilter::default()
        };

        assert!(super::matches_label_filter(&labels, &sync_filter));
    }

    #[test]
    fn matches_label_filter_rejects_non_matching_label() {
        let labels = vec!["bug".to_owned()];
        let sync_filter = super::SyncFilter {
            labels: Some(vec!["forge".to_owned()]),
            ..super::SyncFilter::default()
        };

        assert!(!super::matches_label_filter(&labels, &sync_filter));
    }

    #[test]
    fn matches_label_filter_rejects_empty_issue_labels_with_filter() {
        let labels = Vec::new();
        let sync_filter = super::SyncFilter {
            labels: Some(vec!["forge".to_owned()]),
            ..super::SyncFilter::default()
        };

        assert!(!super::matches_label_filter(&labels, &sync_filter));
    }

    #[test]
    fn github_issue_with_pull_request_field_is_skipped() {
        let issue = super::GitHubIssue {
            number: 1,
            title: "Fix bug".to_owned(),
            body: None,
            labels: Vec::new(),
            state: "open".to_owned(),
            html_url: "https://github.com/owner/repo/pull/1".to_owned(),
            created_at: "2026-04-21T00:00:00Z".to_owned(),
            pull_request: Some(serde_json::json!({ "url": "https://api.github.com/pulls/1" })),
        };

        assert!(issue.pull_request.is_some());
    }
}
