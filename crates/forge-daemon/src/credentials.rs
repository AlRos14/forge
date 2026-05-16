use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use url::Url;

const CREDENTIALS_FILE: &str = "credentials.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonCredentials {
    pub daemon_id: String,
    pub token: String,
}

pub fn default_path(server: &str) -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".config"));
    base.join("forge-daemon")
        .join(server_directory(server))
        .join(CREDENTIALS_FILE)
}

pub async fn load(path: &Path) -> Result<Option<DaemonCredentials>> {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read daemon credentials from {}", path.display()));
        }
    };

    serde_json::from_str(&contents)
        .with_context(|| format!("parse daemon credentials at {}", path.display()))
        .map(Some)
}

pub async fn save(path: &Path, credentials: &DaemonCredentials) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create daemon credentials directory {}", parent.display()))?;
    }

    let contents = serde_json::to_string_pretty(credentials)?;
    tokio::fs::write(path, contents)
        .await
        .with_context(|| format!("write daemon credentials to {}", path.display()))
}

fn server_directory(server: &str) -> String {
    Url::parse(server.trim())
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_owned();
            Some(match url.port() {
                Some(port) => format!("{host}_{port}"),
                None => host,
            })
        })
        .filter(|value| !value.is_empty())
        .map(|value| sanitize_path_segment(&value))
        .unwrap_or_else(|| sanitize_path_segment(server))
}

fn sanitize_path_segment(value: &str) -> String {
    let segment: String = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if segment.is_empty() {
        "default".to_owned()
    } else {
        segment
    }
}

#[cfg(test)]
mod tests {
    use super::server_directory;

    #[test]
    fn server_directory_uses_url_host_and_port() {
        assert_eq!(
            server_directory("https://forge.example.com:8443/base"),
            "forge.example.com_8443"
        );
    }

    #[test]
    fn server_directory_sanitizes_non_url_values() {
        assert_eq!(server_directory("local/server test"), "local_server_test");
    }
}
