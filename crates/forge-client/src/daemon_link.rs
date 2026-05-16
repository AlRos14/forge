use std::{
    error::Error,
    fmt,
    future::Future,
    io::{Error as IoError, ErrorKind},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use tokio::{net::TcpStream, sync::Mutex, time::sleep};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{
            header::{HeaderValue, AUTHORIZATION},
            Response as HttpResponse,
        },
        protocol::Message,
    },
    MaybeTlsStream, WebSocketStream,
};
use url::Url;

pub const DAEMON_HEARTBEAT_INTERVAL_SECS: u64 = 30;

const DAEMON_RETRY_MAX_ATTEMPTS: usize = 5;
const DAEMON_RETRY_INITIAL_BACKOFF_SECS: u64 = 1;
const DAEMON_RETRY_MAX_BACKOFF_SECS: u64 = 30;

type CommandSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type CommandSender = SplitSink<CommandSocket, Message>;
type CommandReceiver = SplitStream<CommandSocket>;
type SharedCommandSender = Arc<Mutex<CommandSender>>;

#[derive(Clone)]
pub struct DaemonClient {
    pub server_url: Url,
    pub http: reqwest::Client,
    pub daemon_id: Option<String>,
    pub token: Option<String>,
}

impl DaemonClient {
    pub fn new(server_url: impl Into<String>) -> Result<Self> {
        let server_url = server_url.into();
        let server_url = Url::parse(server_url.trim()).context("parse Forge server URL")?;

        Ok(Self {
            server_url,
            http: reqwest::Client::new(),
            daemon_id: None,
            token: None,
        })
    }

    pub async fn register(
        &mut self,
        request: api_types::DaemonRegisterRequest,
        user_access_token: Option<&str>,
    ) -> Result<api_types::DaemonRegisterResponse> {
        let url = self.endpoint_url("/api/v1/daemons/register")?;
        let mut builder = self.http.post(url).json(&request);
        if let Some(token) = user_access_token {
            builder = builder.bearer_auth(token);
        }

        let response = builder.send().await?;
        let response: api_types::DaemonRegisterResponse = decode_json(response).await?;
        self.daemon_id = Some(response.daemon_id.clone());
        self.token = Some(response.registration_token.clone());
        Ok(response)
    }

    pub fn set_credentials(&mut self, daemon_id: String, token: String) {
        self.daemon_id = Some(daemon_id);
        self.token = Some(token);
    }

    pub async fn report(
        &self,
        request: api_types::DaemonReportRequest,
    ) -> Result<api_types::DaemonResponse> {
        let (daemon_id, token) = self.credentials()?;
        let url = self.endpoint_url(&format!("/api/v1/daemons/{daemon_id}/report"))?;
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&request)
            .send()
            .await?;
        decode_json(response).await
    }

    pub async fn connect_command_stream(&self) -> Result<DaemonCommandStream> {
        let token = self.token.as_deref().ok_or_else(|| {
            anyhow!("daemon credentials are not set; register or restore credentials first")
        })?;
        let url = self.command_stream_url()?;
        let mut request = url
            .as_str()
            .into_client_request()
            .context("build daemon command WebSocket request")?;
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .context("build daemon authorization header")?,
        );

        let (stream, response) = connect_async(request)
            .await
            .map_err(map_websocket_connect_error)
            .context("connect daemon command WebSocket")?;
        ensure_websocket_status(&response)?;
        let (sender, receiver) = stream.split();
        Ok(DaemonCommandStream {
            sender: Arc::new(Mutex::new(sender)),
            receiver,
        })
    }

    fn credentials(&self) -> Result<(&str, &str)> {
        let daemon_id = self.daemon_id.as_deref().ok_or_else(|| {
            anyhow!("daemon credentials are not set; register or restore credentials first")
        })?;
        let token = self.token.as_deref().ok_or_else(|| {
            anyhow!("daemon credentials are not set; register or restore credentials first")
        })?;
        Ok((daemon_id, token))
    }

    fn endpoint_url(&self, path: &str) -> Result<Url> {
        let mut url = self.server_url.clone();
        url.set_query(None);
        url.set_fragment(None);

        let base_path = url.path().trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let next_path = if base_path.is_empty() || base_path == "/" {
            format!("/{path}")
        } else {
            format!("{base_path}/{path}")
        };
        url.set_path(&next_path);
        Ok(url)
    }

    fn command_stream_url(&self) -> Result<Url> {
        let daemon_id = self.daemon_id.as_deref().ok_or_else(|| {
            anyhow!("daemon credentials are not set; register or restore credentials first")
        })?;
        let mut url = self.endpoint_url(&format!("/api/v1/daemons/{daemon_id}/connect"))?;
        let next_scheme = match url.scheme() {
            "http" => "ws",
            "https" => "wss",
            scheme => bail!("unsupported Forge server URL scheme for WebSocket: {scheme}"),
        };
        url.set_scheme(next_scheme)
            .map_err(|_| anyhow!("rewrite daemon command URL scheme to {next_scheme}"))?;
        Ok(url)
    }
}

pub struct DaemonCommandStream {
    sender: SharedCommandSender,
    receiver: CommandReceiver,
}

impl DaemonCommandStream {
    pub async fn recv(&mut self) -> Result<api_types::DaemonFrame> {
        loop {
            let message = self
                .receiver
                .next()
                .await
                .ok_or_else(stream_closed_error)??;

            match message {
                Message::Text(text) => {
                    return serde_json::from_str(&text.to_string())
                        .context("deserialize daemon command frame");
                }
                Message::Close(_) => return Err(stream_closed_error()),
                Message::Ping(_) | Message::Pong(_) => continue,
                Message::Binary(_) => bail!("expected daemon command frame as WebSocket text"),
                Message::Frame(_) => continue,
            }
        }
    }

    pub async fn send(&mut self, frame: &api_types::DaemonFrame) -> Result<()> {
        send_frame(&self.sender, frame).await
    }

    pub async fn send_heartbeat(&mut self, seq: u64) -> Result<()> {
        self.send(&api_types::DaemonFrame::Heartbeat { seq }).await
    }

    pub async fn close(self) -> Result<()> {
        self.sender
            .lock()
            .await
            .close()
            .await
            .context("close daemon command WebSocket")
    }
}

pub async fn run_with_reconnect<F, Fut>(client: Arc<DaemonClient>, on_stream: F) -> !
where
    F: Fn(DaemonCommandStream) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>>,
{
    let mut backoff = Duration::from_secs(DAEMON_RETRY_INITIAL_BACKOFF_SECS);
    let mut reconnect_attempt: u64 = 0;

    loop {
        match client.connect_command_stream().await {
            Ok(stream) => {
                tracing::info!(
                    daemon_id = ?client.daemon_id,
                    "daemon command stream connected"
                );
                backoff = Duration::from_secs(DAEMON_RETRY_INITIAL_BACKOFF_SECS);

                let heartbeat_sender = Arc::clone(&stream.sender);
                let mut heartbeat =
                    tokio::spawn(async move { heartbeat_loop(heartbeat_sender).await });
                let stream_future = on_stream(stream);
                tokio::pin!(stream_future);

                let result = tokio::select! {
                    result = &mut stream_future => {
                        heartbeat.abort();
                        let _ = heartbeat.await;
                        result
                    }
                    result = &mut heartbeat => {
                        match result {
                            Ok(Ok(())) => Ok(()),
                            Ok(Err(error)) => Err(error).context("daemon heartbeat failed"),
                            Err(error) => Err(error).context("daemon heartbeat task failed"),
                        }
                    }
                };

                if let Err(error) = result {
                    tracing::warn!(
                        error = %error,
                        "daemon command stream ended with error; reconnecting"
                    );
                } else {
                    tracing::warn!("daemon command stream closed; reconnecting");
                }
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "daemon command stream connection failed; reconnecting"
                );
            }
        }

        reconnect_attempt = reconnect_attempt.saturating_add(1);
        tracing::warn!(
            attempt = reconnect_attempt,
            backoff_secs = backoff.as_secs(),
            "daemon command stream reconnect scheduled"
        );
        sleep(backoff).await;
        backoff = next_backoff(backoff);
    }
}

pub async fn register_with_retry(
    client: &mut DaemonClient,
    request: &api_types::DaemonRegisterRequest,
    user_access_token: Option<&str>,
) -> Result<api_types::DaemonRegisterResponse> {
    let mut attempt = 1;
    let mut backoff = Duration::from_secs(DAEMON_RETRY_INITIAL_BACKOFF_SECS);

    loop {
        match client.register(request.clone(), user_access_token).await {
            Ok(response) => return Ok(response),
            Err(error) if should_retry(&error, attempt) => {
                tracing::warn!(
                    attempt,
                    backoff_secs = backoff.as_secs(),
                    error = %error,
                    "daemon registration failed; retrying"
                );
                sleep(backoff).await;
                attempt += 1;
                backoff = next_backoff(backoff);
            }
            Err(error) => return Err(error),
        }
    }
}

pub async fn report_with_retry(
    client: &DaemonClient,
    request: &api_types::DaemonReportRequest,
) -> Result<api_types::DaemonResponse> {
    let mut attempt = 1;
    let mut backoff = Duration::from_secs(DAEMON_RETRY_INITIAL_BACKOFF_SECS);

    loop {
        match client.report(request.clone()).await {
            Ok(response) => return Ok(response),
            Err(error) if should_retry(&error, attempt) => {
                tracing::warn!(
                    attempt,
                    backoff_secs = backoff.as_secs(),
                    error = %error,
                    "daemon report failed; retrying"
                );
                sleep(backoff).await;
                attempt += 1;
                backoff = next_backoff(backoff);
            }
            Err(error) => return Err(error),
        }
    }
}

async fn heartbeat_loop(sender: SharedCommandSender) -> Result<()> {
    let mut ticker = tokio::time::interval(Duration::from_secs(DAEMON_HEARTBEAT_INTERVAL_SECS));
    ticker.tick().await;
    let mut seq = 0;

    loop {
        ticker.tick().await;
        seq += 1;
        send_frame(&sender, &api_types::DaemonFrame::Heartbeat { seq }).await?;
    }
}

async fn send_frame(sender: &SharedCommandSender, frame: &api_types::DaemonFrame) -> Result<()> {
    let payload = serde_json::to_string(frame).context("serialize daemon command frame")?;
    sender
        .lock()
        .await
        .send(Message::Text(payload))
        .await
        .context("send daemon command frame")
}

async fn decode_json<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let body = response.bytes().await?;

    if !status.is_success() {
        return Err(HttpStatusError {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        }
        .into());
    }

    serde_json::from_slice(&body).map_err(Into::into)
}

fn should_retry(error: &anyhow::Error, attempt: usize) -> bool {
    attempt < DAEMON_RETRY_MAX_ATTEMPTS && is_transient_error(error)
}

fn is_transient_error(error: &anyhow::Error) -> bool {
    if let Some(status) = error.downcast_ref::<HttpStatusError>() {
        return status.status.is_server_error();
    }

    if let Some(reqwest) = error.downcast_ref::<reqwest::Error>() {
        return reqwest.is_connect() || reqwest.is_timeout();
    }

    false
}

fn next_backoff(current: Duration) -> Duration {
    Duration::from_secs((current.as_secs() * 2).min(DAEMON_RETRY_MAX_BACKOFF_SECS))
}

fn stream_closed_error() -> anyhow::Error {
    IoError::new(ErrorKind::UnexpectedEof, "daemon command stream closed").into()
}

fn ensure_websocket_status(response: &HttpResponse<Option<Vec<u8>>>) -> Result<()> {
    let status = response.status();
    if status.as_u16() == 101 || status.is_success() {
        Ok(())
    } else {
        Err(HttpStatusError {
            status: StatusCode::from_u16(status.as_u16())
                .context("convert WebSocket response status")?,
            body: String::new(),
        }
        .into())
    }
}

fn map_websocket_connect_error(error: tokio_tungstenite::tungstenite::Error) -> anyhow::Error {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            let status = StatusCode::from_u16(response.status().as_u16())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            HttpStatusError {
                status,
                body: response
                    .body()
                    .as_ref()
                    .map(|body| String::from_utf8_lossy(body).into_owned())
                    .unwrap_or_default(),
            }
            .into()
        }
        other => other.into(),
    }
}

#[derive(Debug)]
struct HttpStatusError {
    status: StatusCode,
    body: String,
}

impl fmt::Display for HttpStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let body = self.body.trim();
        if body.is_empty() {
            write!(formatter, "request failed with status {}", self.status)
        } else {
            write!(
                formatter,
                "request failed with status {}: {body}",
                self.status
            )
        }
    }
}

impl Error for HttpStatusError {}

#[cfg(test)]
mod tests {
    use super::DaemonClient;

    #[test]
    fn parses_server_url_and_rewrites_command_stream_scheme() {
        let mut http = DaemonClient::new("http://forge.example.com/base/")
            .expect("construct HTTP daemon client");
        http.set_credentials("daemon-1".to_owned(), "token-1".to_owned());
        let ws_url = http.command_stream_url().expect("build ws URL");
        assert_eq!(http.server_url.scheme(), "http");
        assert_eq!(
            ws_url.as_str(),
            "ws://forge.example.com/base/api/v1/daemons/daemon-1/connect"
        );

        let mut https =
            DaemonClient::new("https://forge.example.com").expect("construct HTTPS daemon client");
        https.set_credentials("daemon-2".to_owned(), "token-2".to_owned());
        let wss_url = https.command_stream_url().expect("build wss URL");
        assert_eq!(
            wss_url.as_str(),
            "wss://forge.example.com/api/v1/daemons/daemon-2/connect"
        );
    }

    #[test]
    fn request_frame_round_trips_with_expected_json_shape() {
        let frame = api_types::DaemonFrame::Request {
            id: "cmd-1".to_owned(),
            method: api_types::METHOD_FS_LIST.to_owned(),
            params: serde_json::json!({ "path": "/tmp" }),
        };

        let json = serde_json::to_value(&frame).expect("serialize daemon request frame");
        assert_eq!(
            json,
            serde_json::json!({
                "type": "request",
                "id": "cmd-1",
                "method": "fs.list",
                "params": { "path": "/tmp" }
            })
        );

        let decoded: api_types::DaemonFrame =
            serde_json::from_value(json).expect("deserialize daemon request frame");
        assert!(matches!(
            decoded,
            api_types::DaemonFrame::Request { id, method, .. }
                if id == "cmd-1" && method == api_types::METHOD_FS_LIST
        ));
    }
}
