//! Provider OAuth login driven from the machine the browser runs on.
//!
//! Providers whose OAuth client only whitelists a localhost callback (OpenAI's
//! Codex client) need a listener on the browser's machine. When Forge is
//! remote it cannot provide one, so `forge-ctl` binds the port here and relays
//! the authorization code back to the server. The PKCE verifier and the tokens
//! stay on the server throughout: this process only ever sees `code` and
//! `state`.

use std::net::Ipv4Addr;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use api_types::{
    AgentProviderId, LoopbackOwner, ProviderAuthorizationOperationResponse,
    ProviderAuthorizationState, ProviderCredentialMethod, StartProviderAuthorizationRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::client::ForgeClient;

/// Ports OpenAI's Codex OAuth client whitelists for its localhost callback.
const LOOPBACK_CALLBACK_PORTS: [u16; 2] = [1455, 1457];
const LOOPBACK_CALLBACK_PATH: &str = "/auth/callback";
const MAX_CALLBACK_BYTES: usize = 8 * 1024;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Runs a browser OAuth ceremony with the callback socket owned by this
/// process, then polls until the server reaches a terminal state.
pub async fn browser_login(
    client: &ForgeClient,
    provider: AgentProviderId,
    label: &str,
    no_open: bool,
) -> Result<ProviderAuthorizationOperationResponse> {
    let (listener, port) = bind_callback().await?;
    let started: ProviderAuthorizationOperationResponse = client
        .post(
            "/api/v1/provider-authorizations",
            &StartProviderAuthorizationRequest {
                provider,
                method: ProviderCredentialMethod::BrowserOauth,
                // Only used for the post-login bounce; the OAuth redirect_uri
                // is the loopback socket bound above.
                redirect_origin: client.url("/").trim_end_matches('/').to_owned(),
                credential_label: label.to_owned(),
                loopback_owner: LoopbackOwner::Client,
                loopback_port: Some(port),
            },
        )
        .await
        .context("starting the provider authorization")?;
    let authorization_url = started
        .authorization_url
        .clone()
        .context("server did not return an authorization URL")?;

    eprintln!("Open this URL to continue:\n  {authorization_url}");
    if !no_open && open_browser(&authorization_url) {
        eprintln!("Opened your browser. Waiting for the callback on localhost:{port}…");
    } else {
        eprintln!("Waiting for the callback on localhost:{port}…");
    }

    let query = tokio::time::timeout(LOGIN_TIMEOUT, wait_for_callback(listener))
        .await
        .context("provider login timed out")??;
    client
        .relay_provider_callback(&format!(
            "/api/v1/provider-authorizations/{}/callback?{query}",
            provider_path(provider)
        ))
        .await
        .context("relaying the authorization code to Forge")?;
    poll_until_terminal(client, &started.id).await
}

/// Runs a device-code ceremony. No socket is needed, so this works against a
/// remote Forge unchanged.
pub async fn device_login(
    client: &ForgeClient,
    provider: AgentProviderId,
    label: &str,
) -> Result<ProviderAuthorizationOperationResponse> {
    let started: ProviderAuthorizationOperationResponse = client
        .post(
            "/api/v1/provider-authorizations",
            &StartProviderAuthorizationRequest {
                provider,
                method: ProviderCredentialMethod::DeviceOauth,
                redirect_origin: client.url("/").trim_end_matches('/').to_owned(),
                credential_label: label.to_owned(),
                loopback_owner: LoopbackOwner::Server,
                loopback_port: None,
            },
        )
        .await
        .context("starting the provider authorization")?;
    if let Some(url) = started.authorization_url.as_deref() {
        eprintln!("Open this URL to continue:\n  {url}");
    }
    if let Some(code) = started.user_code.as_deref() {
        eprintln!("Enter this code: {code}");
    }
    poll_until_terminal(client, &started.id).await
}

async fn poll_until_terminal(
    client: &ForgeClient,
    id: &str,
) -> Result<ProviderAuthorizationOperationResponse> {
    let path = format!("/api/v1/provider-authorizations/{id}");
    loop {
        let operation: ProviderAuthorizationOperationResponse = client
            .get(&path)
            .await
            .context("reading the provider authorization")?;
        if operation.state.is_terminal() {
            return Ok(operation);
        }
        tokio::time::sleep(Duration::from_secs(u64::from(
            operation.poll_interval_seconds.max(1),
        )))
        .await;
    }
}

async fn bind_callback() -> Result<(TcpListener, u16)> {
    for port in LOOPBACK_CALLBACK_PORTS {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
            Ok(listener) => return Ok((listener, port)),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {}
            Err(error) => {
                return Err(error).context("binding the localhost OAuth callback");
            }
        }
    }
    bail!("provider login needs localhost port 1455 or 1457; both are already in use")
}

/// Accepts the provider's redirect and returns its raw query string. The code
/// is forwarded verbatim rather than parsed: the server owns validation.
async fn wait_for_callback(listener: TcpListener) -> Result<String> {
    let (mut stream, peer) = listener
        .accept()
        .await
        .context("accepting the localhost callback")?;
    if !peer.ip().is_loopback() {
        respond(&mut stream, 403, "Callback rejected.").await;
        bail!("provider login callback came from a non-loopback address")
    }
    let head = read_request_head(&mut stream).await?;
    let Some(query) = callback_query(&head) else {
        respond(&mut stream, 404, "Not found.").await;
        bail!("provider login callback target was rejected")
    };
    if query.is_empty() {
        respond(&mut stream, 400, "Callback was malformed.").await;
        bail!("provider login callback carried no parameters")
    }
    respond(
        &mut stream,
        200,
        "Sign-in received. You can close this window and return to Forge.",
    )
    .await;
    Ok(query)
}

fn callback_query(head: &str) -> Option<String> {
    let mut parts = head.lines().next()?.split_whitespace();
    if parts.next() != Some("GET") {
        return None;
    }
    let target = parts.next()?;
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        return None;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    (path == LOOPBACK_CALLBACK_PATH && !query.contains('#')).then(|| query.to_owned())
}

async fn read_request_head(stream: &mut TcpStream) -> Result<String> {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0_u8; 1_024];
        let count = stream
            .read(&mut chunk)
            .await
            .context("reading the localhost callback")?;
        if count == 0 {
            bail!("provider login callback ended before its headers")
        }
        if bytes.len().saturating_add(count) > MAX_CALLBACK_BYTES {
            bail!("provider login callback exceeded the size limit")
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(bytes).context("provider login callback was not valid UTF-8")
}

async fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn provider_path(provider: AgentProviderId) -> &'static str {
    match provider {
        AgentProviderId::OpenAi => "openai",
        AgentProviderId::XAi => "xai",
        AgentProviderId::Gemini => "gemini",
        AgentProviderId::OpenRouter => "openrouter",
        AgentProviderId::OpenAiCompatible => "openai_compatible",
    }
}

fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        return false;
    }
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Terminal-state helper mirrored from the API type so callers can report a
/// non-zero exit without re-deriving the state list.
pub fn failed_reason(operation: &ProviderAuthorizationOperationResponse) -> Option<String> {
    match operation.state {
        ProviderAuthorizationState::Succeeded => None,
        _ => Some(
            operation
                .error_message
                .clone()
                .unwrap_or_else(|| format!("provider login ended in state {:?}", operation.state)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_query_accepts_only_the_registered_target() {
        assert_eq!(
            callback_query("GET /auth/callback?code=a&state=b HTTP/1.1\r\n\r\n").as_deref(),
            Some("code=a&state=b")
        );
        assert_eq!(
            callback_query("GET /auth/callback HTTP/1.1\r\n\r\n").as_deref(),
            Some("")
        );
        assert!(callback_query("POST /auth/callback?code=a HTTP/1.1\r\n\r\n").is_none());
        assert!(callback_query("GET /elsewhere?code=a HTTP/1.1\r\n\r\n").is_none());
        assert!(callback_query("GET /auth/callback?code=a#x HTTP/1.1\r\n\r\n").is_none());
    }
}
