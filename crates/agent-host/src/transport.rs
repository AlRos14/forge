use std::{
    fmt,
    net::{IpAddr, SocketAddr},
};

use agent_runtime::{
    core::provider::{ProviderError, ProviderErrorKind},
    provider::transport::{ByteStream, HttpRequest, HttpResponse, HttpTransport},
};
use async_trait::async_trait;
use futures_util::StreamExt;

#[derive(Clone)]
pub struct ReqwestTransport {}

impl ReqwestTransport {
    pub fn new() -> Result<Self, ProviderError> {
        Ok(Self {})
    }

    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ProviderError> {
        let (client, url) = provider_client_for_url(&request.url).await?;
        let mut builder = client.post(url).body(request.body);
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        let response = builder.send().await.map_err(|error| {
            let kind = if error.is_timeout() {
                ProviderErrorKind::Timeout
            } else {
                ProviderErrorKind::Network
            };
            ProviderError::new(kind, "provider HTTP request failed").retryable()
        })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            // Providers in agent-runtime never see raw HTTP statuses — this
            // transport owns turning them into typed errors. Auth in
            // particular must be surfaced as `ProviderErrorKind::Auth` or the
            // provider's credential-refresh path can never trigger; anything
            // else would otherwise be parsed as an SSE stream and fail with
            // an empty-stream error that hides the real cause.
            let body_excerpt = response
                .bytes()
                .await
                .ok()
                .map(|bytes| {
                    String::from_utf8_lossy(&bytes)
                        .chars()
                        .take(300)
                        .collect::<String>()
                })
                .unwrap_or_default();
            tracing::warn!(status, body = %body_excerpt, "provider request rejected");
            let (kind, retryable) = match status {
                401 | 403 => (ProviderErrorKind::Auth, false),
                429 => (ProviderErrorKind::RateLimited, true),
                400..=499 => (ProviderErrorKind::BadRequest, false),
                _ => (ProviderErrorKind::Server, true),
            };
            let error = ProviderError::new(kind, format!("provider returned HTTP {status}"));
            return Err(if retryable { error.retryable() } else { error });
        }
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
            })
            .collect();
        let body: ByteStream = Box::pin(response.bytes_stream().map(|chunk| {
            chunk.map(|bytes| bytes.to_vec()).map_err(|_| {
                ProviderError::new(
                    ProviderErrorKind::Network,
                    "provider response stream failed",
                )
                .retryable()
            })
        }));
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

async fn provider_client_for_url(url: &str) -> Result<(reqwest::Client, String), ProviderError> {
    let parsed = url::Url::parse(url)
        .map_err(|_| ProviderError::new(ProviderErrorKind::Network, "provider URL rejected"))?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ProviderError::new(
            ProviderErrorKind::Network,
            "provider URL rejected",
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| ProviderError::new(ProviderErrorKind::Network, "provider URL rejected"))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| ProviderError::new(ProviderErrorKind::Network, "provider URL rejected"))?;
    let addresses = match parsed.host() {
        Some(url::Host::Domain(domain)) => {
            if restricted_provider_hostname(domain) {
                return Err(ProviderError::new(
                    ProviderErrorKind::Network,
                    "provider URL rejected",
                ));
            }
            tokio::net::lookup_host((domain, port))
                .await
                .map_err(|_| {
                    ProviderError::new(ProviderErrorKind::Network, "provider URL rejected")
                })?
                .collect::<Vec<_>>()
        }
        Some(url::Host::Ipv4(address)) => vec![SocketAddr::new(IpAddr::V4(address), port)],
        Some(url::Host::Ipv6(address)) => vec![SocketAddr::new(IpAddr::V6(address), port)],
        None => Vec::new(),
    };
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| restricted_provider_ip(address.ip()))
    {
        return Err(ProviderError::new(
            ProviderErrorKind::Network,
            "provider URL rejected",
        ));
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|_| ProviderError::new(ProviderErrorKind::Network, "HTTP client setup failed"))?;
    Ok((client, url.to_owned()))
}

fn restricted_provider_hostname(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.ends_with(".home.arpa")
}

fn restricted_provider_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(address) => {
            let octets = address.octets();
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_broadcast()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                || (octets[0] == 198 && (18..=19).contains(&octets[1]))
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
                || octets[0] >= 224
        }
        IpAddr::V6(address) => {
            let segments = address.segments();
            address.is_loopback()
                || address.is_unspecified()
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] & 0xffc0) == 0xfec0
                || (segments[0] & 0xfe00) == 0xfc00
                || segments[0] >= 0xff00
                || address
                    .to_ipv4()
                    .is_some_and(|mapped| restricted_provider_ip(IpAddr::V4(mapped)))
        }
    }
}

impl fmt::Debug for ReqwestTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReqwestTransport")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn post_stream(&self, request: HttpRequest) -> Result<ByteStream, ProviderError> {
        Ok(self.send(request).await?.body)
    }

    async fn post_response(&self, request: HttpRequest) -> Result<HttpResponse, ProviderError> {
        self.send(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn provider_transport_rejects_private_urls_before_network_io() {
        for url in [
            "http://127.0.0.1/v1/chat/completions",
            "https://127.0.0.1/v1/chat/completions",
            "https://[::1]/v1/chat/completions",
            "https://169.254.169.254/latest/meta-data",
            "https://user:pass@example.com/v1/chat/completions",
            "https://example.com/v1/chat/completions#fragment",
        ] {
            assert!(provider_client_for_url(url).await.is_err(), "{url}");
        }
    }
}
