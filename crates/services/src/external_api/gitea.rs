use super::{ExternalApiError, ExternalIssue, IssueFetcher, SyncFilter};
use async_trait::async_trait;
use reqwest::header::AUTHORIZATION;
use serde::Deserialize;

pub struct GiteaClient {
    pub base_url: String,
}

#[derive(Deserialize)]
struct GiteaIssue {
    number: i64,
    title: String,
    body: Option<String>,
    labels: Vec<GiteaLabel>,
    state: String,
    html_url: String,
    created_at: String,
}

#[derive(Deserialize)]
struct GiteaLabel {
    name: String,
}

#[async_trait]
impl IssueFetcher for GiteaClient {
    async fn fetch_issues(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
        since: Option<&str>,
        sync_filter: &SyncFilter,
    ) -> Result<Vec<ExternalIssue>, ExternalApiError> {
        let client = reqwest::Client::new();
        let url = format!(
            "{}/api/v1/repos/{owner}/{repo}/issues",
            self.base_url.trim_end_matches('/')
        );
        let mut page = 1;
        let mut issues = Vec::new();

        loop {
            let page_value = page.to_string();
            let mut request = client
                .get(&url)
                .header(AUTHORIZATION, format!("token {token}"))
                .query(&[
                    ("type", "issues"),
                    ("state", "open"),
                    ("limit", "50"),
                    ("page", page_value.as_str()),
                ]);

            if let Some(since) = since {
                request = request.query(&[("since", since)]);
            }

            let response = request.send().await?;
            let status = response.status();
            if !status.is_success() {
                return Err(ExternalApiError::ApiError {
                    status: status.as_u16(),
                    body: response.text().await?,
                });
            }

            let page_issues = response.json::<Vec<GiteaIssue>>().await?;
            if page_issues.is_empty() {
                break;
            }

            issues.extend(page_issues.into_iter().filter_map(|issue| {
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

            page += 1;
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
