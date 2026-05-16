use anyhow::{anyhow, Result};
use reqwest::StatusCode;
use serde::{de::DeserializeOwned, Serialize};

#[derive(Clone)]
pub struct ForgeClient {
    base_url: String,
    http: reqwest::Client,
}

impl ForgeClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.http.get(self.url(path)).send().await?;
        decode_json(response).await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let response = self.http.post(self.url(path)).json(body).send().await?;
        decode_json(response).await
    }

    pub async fn post_bearer<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        token: &str,
        body: &B,
    ) -> Result<T> {
        let response = self
            .http
            .post(self.url(path))
            .bearer_auth(token)
            .json(body)
            .send()
            .await?;
        decode_json(response).await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let response = self.http.delete(self.url(path)).send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }

        Err(request_error(
            status,
            response.text().await.unwrap_or_default(),
        ))
    }

    pub fn url(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("{}{}", self.base_url, path)
        } else {
            format!("{}/{}", self.base_url, path)
        }
    }
}

async fn decode_json<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        return Err(request_error(status, String::from_utf8_lossy(&body).into()));
    }

    serde_json::from_slice(&body).map_err(Into::into)
}

fn request_error(status: StatusCode, body: String) -> anyhow::Error {
    if body.trim().is_empty() {
        anyhow!("request failed with status {status}")
    } else {
        anyhow!("request failed with status {status}: {body}")
    }
}
