//! Pinned, one-hop HTTPS transport for the delegated network gateway.
//!
//! This module intentionally has no policy decisions. `NetworkGateway` resolves and
//! authorizes a logical hostname first; this transport receives that hostname together
//! with the approved IP addresses and refuses to manufacture a DNS fallback.

use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use async_trait::async_trait;
use reqwest::{redirect::Policy, Client, Method};
use serde::Deserialize;

use super::network_gateway::{
    NetworkTransport, TransportFetchRequest, TransportFetchResponse, TransportResponseBounds,
    TransportSearchRequest, TransportSearchResponse, TransportSearchResult,
};
use super::tool::ToolCancellation;

/// Safe, deliberately small request vocabulary exposed to the transport connector.
/// There is no header, cookie, credential, proxy, request-body, or redirect option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedHttpsRequest {
    pub logical_host: String,
    pub port: u16,
    pub resolved_endpoints: Vec<SocketAddr>,
    pub method: PinnedHttpsMethod,
    pub path_and_query: String,
    pub timeout: Duration,
    /// Policy-derived constraints, supplied only by the authorized gateway
    /// request.  The connector enforces the header/byte bounds before it
    /// reads a body into memory.
    pub response_bounds: TransportResponseBounds,
    /// These constraints are fixed by `GatewayNetworkTransport`, not supplied by a worker.
    pub(crate) redirects_disabled: bool,
    pub(crate) proxies_disabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedHttpsMethod {
    Get,
    Head,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedHttpsResponse {
    pub status: u16,
    pub headers: BTreeSet<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedHttpsError {
    Cancelled,
    InvalidRequest,
    TimedOut,
    ResponseTooLarge,
    ContentTypeDenied,
    Transport,
}

/// The lower-level connection seam. Its input does not include a hostname to resolve:
/// a connector must use `resolved_endpoints` and preserve TLS SNI/certificate checking
/// for `logical_host`.
#[async_trait]
pub trait PinnedHttpsConnector: Send + Sync {
    async fn execute(
        &self,
        request: PinnedHttpsRequest,
        cancellation: &ToolCancellation,
    ) -> Result<PinnedHttpsResponse, PinnedHttpsError>;
}

/// Production connector based on the already-present `reqwest` + `rustls` stack.
///
/// `ClientBuilder::resolve` pins each logical hostname to a gateway-approved socket
/// address while the request URL retains the logical hostname. That makes rustls use
/// the logical hostname for SNI and certificate verification, without client-side DNS
/// re-resolution. `no_proxy` prevents ambient proxy environment configuration from
/// becoming an alternate route; redirects are deliberately returned to the gateway.
#[derive(Debug, Default)]
pub struct ReqwestPinnedHttpsConnector;

#[async_trait]
impl PinnedHttpsConnector for ReqwestPinnedHttpsConnector {
    async fn execute(
        &self,
        request: PinnedHttpsRequest,
        cancellation: &ToolCancellation,
    ) -> Result<PinnedHttpsResponse, PinnedHttpsError> {
        if cancellation.is_cancelled() {
            return Err(PinnedHttpsError::Cancelled);
        }
        if request.logical_host.is_empty()
            || request.resolved_endpoints.is_empty()
            || request.path_and_query.is_empty()
            || !request.path_and_query.starts_with('/')
            || !request.redirects_disabled
            || !request.proxies_disabled
        {
            return Err(PinnedHttpsError::InvalidRequest);
        }

        let mut builder = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(request.timeout)
            .connect_timeout(request.timeout);
        for endpoint in &request.resolved_endpoints {
            if endpoint.port() != request.port {
                return Err(PinnedHttpsError::InvalidRequest);
            }
            builder = builder.resolve(&request.logical_host, *endpoint);
        }
        let client = builder.build().map_err(|_| PinnedHttpsError::Transport)?;
        let url = format!(
            "https://{}:{}{}",
            request.logical_host, request.port, request.path_and_query
        );
        let method = match request.method {
            PinnedHttpsMethod::Get => Method::GET,
            PinnedHttpsMethod::Head => Method::HEAD,
        };
        // Cancellation wins when both branches are ready. Dropping reqwest's
        // async future stops local connection/body processing; a GET/HEAD
        // that already reached a remote server cannot be recalled.
        let mut response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(PinnedHttpsError::Cancelled),
            response = client.request(method, url).send() => response.map_err(map_reqwest_error)?,
        };
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or("<non-text>").to_owned(),
                )
            })
            .collect();
        validate_response_metadata(status, &headers, &request.response_bounds)?;
        let body = read_limited(
            &mut response,
            request.response_bounds.max_response_bytes,
            cancellation,
        )
        .await?;
        Ok(PinnedHttpsResponse {
            status,
            headers,
            body,
        })
    }
}

fn map_reqwest_error(error: reqwest::Error) -> PinnedHttpsError {
    if error.is_timeout() {
        PinnedHttpsError::TimedOut
    } else {
        PinnedHttpsError::Transport
    }
}

async fn read_limited(
    response: &mut reqwest::Response,
    max_response_bytes: usize,
    cancellation: &ToolCancellation,
) -> Result<Vec<u8>, PinnedHttpsError> {
    let mut body = Vec::with_capacity(max_response_bytes.min(64 * 1024));
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(PinnedHttpsError::Cancelled),
            chunk = response.chunk() => chunk.map_err(map_reqwest_error)?,
        };
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        append_limited(&mut body, &chunk, max_response_bytes)?;
    }
}

fn append_limited(
    body: &mut Vec<u8>,
    chunk: &[u8],
    max_response_bytes: usize,
) -> Result<(), PinnedHttpsError> {
    if body
        .len()
        .checked_add(chunk.len())
        .is_none_or(|len| len > max_response_bytes)
    {
        return Err(PinnedHttpsError::ResponseTooLarge);
    }
    body.extend_from_slice(chunk);
    Ok(())
}

/// Reject a known-invalid response before its body is read.  Redirects do not
/// yet have evidence semantics, so their MIME type is intentionally deferred
/// to the gateway's redirect handling; their byte declaration is still bound.
fn validate_response_metadata(
    status: u16,
    headers: &BTreeSet<(String, String)>,
    bounds: &TransportResponseBounds,
) -> Result<(), PinnedHttpsError> {
    if bounds.max_response_bytes == 0
        || headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse::<u64>().ok())
            .is_some_and(|bytes| bytes > bounds.max_response_bytes as u64)
    {
        return Err(PinnedHttpsError::ResponseTooLarge);
    }
    if (200..300).contains(&status) {
        let mime = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| normalized_mime(value))
            .filter(|mime| !mime.is_empty())
            .ok_or(PinnedHttpsError::ContentTypeDenied)?;
        if !bounds.allowed_mime_types.contains(&mime) {
            return Err(PinnedHttpsError::ContentTypeDenied);
        }
    }
    Ok(())
}

fn normalized_mime(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

#[derive(Debug, Clone)]
pub struct NetworkTransportLimits {
    pub timeout: Duration,
    /// Global process ceiling. Per-request policy bounds can only reduce it.
    pub max_response_bytes: usize,
}

impl Default for NetworkTransportLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            max_response_bytes: 2 * 1024 * 1024,
        }
    }
}

/// The gateway-facing transport. It performs exactly one request hop; `NetworkGateway`
/// owns redirect validation and can issue a separately authorized next hop.
pub struct GatewayNetworkTransport<C> {
    connector: C,
    limits: NetworkTransportLimits,
}

impl<C> GatewayNetworkTransport<C> {
    pub fn new(connector: C, limits: NetworkTransportLimits) -> Self {
        Self { connector, limits }
    }
}

impl Default for GatewayNetworkTransport<ReqwestPinnedHttpsConnector> {
    fn default() -> Self {
        Self::new(
            ReqwestPinnedHttpsConnector,
            NetworkTransportLimits::default(),
        )
    }
}

#[async_trait]
impl<C: PinnedHttpsConnector> NetworkTransport for GatewayNetworkTransport<C> {
    async fn search(
        &self,
        request: TransportSearchRequest,
        cancellation: &ToolCancellation,
    ) -> Result<TransportSearchResponse, String> {
        let mut url = reqwest::Url::parse(&format!("https://{}/search", request.provider_host))
            .map_err(|_| "invalid pinned request")?;
        url.query_pairs_mut().append_pair("q", &request.query);
        let response = self
            .execute_url(
                &url,
                PinnedHttpsMethod::Get,
                request.resolved_addresses,
                request.response_bounds,
                cancellation,
            )
            .await?;
        let byte_count = response.body.len();
        let content_type = response_mime(&response.headers)?;
        parse_search_response(response.body, content_type, byte_count)
    }

    async fn fetch(
        &self,
        request: TransportFetchRequest,
        cancellation: &ToolCancellation,
    ) -> Result<TransportFetchResponse, String> {
        let url = reqwest::Url::parse(&request.url).map_err(|_| "invalid pinned request")?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || url.username() != ""
            || url.password().is_some()
        {
            return Err("invalid pinned request".into());
        }
        let method = match request.method.as_str() {
            "GET" => PinnedHttpsMethod::Get,
            "HEAD" => PinnedHttpsMethod::Head,
            _ => return Err("invalid pinned request".into()),
        };
        let response = self
            .execute_url(
                &url,
                method,
                request.resolved_addresses,
                request.response_bounds,
                cancellation,
            )
            .await?;
        Ok(TransportFetchResponse {
            status: response.status,
            headers: response.headers,
            body: response.body,
        })
    }
}

impl<C: PinnedHttpsConnector> GatewayNetworkTransport<C> {
    async fn execute_url(
        &self,
        url: &reqwest::Url,
        method: PinnedHttpsMethod,
        addresses: Vec<IpAddr>,
        response_bounds: TransportResponseBounds,
        cancellation: &ToolCancellation,
    ) -> Result<PinnedHttpsResponse, String> {
        if cancellation.is_cancelled() {
            return Err("network operation cancelled".into());
        }
        let logical_host = url.host_str().ok_or("invalid pinned request")?.to_owned();
        let port = url
            .port_or_known_default()
            .ok_or("invalid pinned request")?;
        if addresses.is_empty() {
            return Err("pinned endpoint required".into());
        }
        let path_and_query = match url.query() {
            Some(query) => format!("{}?{}", url.path(), query),
            None => url.path().to_owned(),
        };
        let endpoints = addresses
            .into_iter()
            .map(|ip| SocketAddr::new(ip, port))
            .collect();
        let response_bounds = TransportResponseBounds {
            max_response_bytes: response_bounds
                .max_response_bytes
                .min(self.limits.max_response_bytes),
            allowed_mime_types: response_bounds.allowed_mime_types,
        };
        let response = self
            .connector
            .execute(
                PinnedHttpsRequest {
                    logical_host,
                    port,
                    resolved_endpoints: endpoints,
                    method,
                    path_and_query,
                    timeout: self.limits.timeout,
                    response_bounds: response_bounds.clone(),
                    redirects_disabled: true,
                    proxies_disabled: true,
                },
                cancellation,
            )
            .await
            .map_err(|error| match error {
                PinnedHttpsError::Cancelled => String::from("network operation cancelled"),
                PinnedHttpsError::TimedOut => String::from("network timeout"),
                PinnedHttpsError::ResponseTooLarge => String::from("response exceeds byte limit"),
                PinnedHttpsError::ContentTypeDenied => String::from("response content type denied"),
                _ => String::from("network transport unavailable"),
            })?;
        if cancellation.is_cancelled() {
            return Err("network operation cancelled".into());
        }
        // Production connectors reject header/body violations while reading,
        // but revalidate here so alternate connector implementations cannot
        // hand malformed evidence to the parser.
        validate_response(&response, &response_bounds)?;
        Ok(response)
    }
}

fn validate_response(
    response: &PinnedHttpsResponse,
    bounds: &TransportResponseBounds,
) -> Result<(), String> {
    validate_response_metadata(response.status, &response.headers, bounds).map_err(|error| {
        match error {
            PinnedHttpsError::ResponseTooLarge => String::from("response exceeds byte limit"),
            PinnedHttpsError::ContentTypeDenied => String::from("response content type denied"),
            _ => String::from("network transport unavailable"),
        }
    })?;
    if response.body.len() > bounds.max_response_bytes {
        return Err("response exceeds byte limit".into());
    }
    Ok(())
}

fn response_mime(headers: &BTreeSet<(String, String)>) -> Result<String, String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| normalized_mime(value))
        .filter(|mime| !mime.is_empty())
        .ok_or_else(|| "response content type denied".into())
}

fn parse_search_response(
    body: Vec<u8>,
    content_type: String,
    byte_count: usize,
) -> Result<TransportSearchResponse, String> {
    #[derive(Deserialize)]
    struct SearchWire {
        results: Vec<SearchResultWire>,
    }
    #[derive(Deserialize)]
    struct SearchResultWire {
        title: String,
        url: String,
        snippet: String,
    }
    let parsed: SearchWire =
        serde_json::from_slice(&body).map_err(|_| "invalid search response")?;
    Ok(TransportSearchResponse {
        content_type,
        byte_count,
        results: parsed
            .results
            .into_iter()
            .map(|result| TransportSearchResult {
                title: result.title,
                url: result.url,
                snippet: result.snippet,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::{
        net::{IpAddr, Ipv4Addr},
        sync::{Arc, Mutex},
        time::Duration,
    };

    struct RecordingConnector {
        requests: Mutex<Vec<PinnedHttpsRequest>>,
        response: Mutex<Result<PinnedHttpsResponse, PinnedHttpsError>>,
    }
    impl RecordingConnector {
        fn success() -> Self {
            Self {
                requests: Mutex::new(vec![]),
                response: Mutex::new(Ok(PinnedHttpsResponse {
                    status: 200,
                    headers: BTreeSet::from([(
                        "content-type".to_owned(),
                        "text/html; charset=utf-8".to_owned(),
                    )]),
                    body: b"ok".to_vec(),
                })),
            }
        }
    }
    #[async_trait]
    impl PinnedHttpsConnector for RecordingConnector {
        async fn execute(
            &self,
            request: PinnedHttpsRequest,
            _: &ToolCancellation,
        ) -> Result<PinnedHttpsResponse, PinnedHttpsError> {
            self.requests.lock().unwrap().push(request);
            self.response.lock().unwrap().clone()
        }
    }
    fn fetch(url: &str, addresses: Vec<IpAddr>) -> TransportFetchRequest {
        TransportFetchRequest {
            url: url.into(),
            method: "GET".into(),
            resolved_addresses: addresses,
            response_bounds: response_bounds(4, &["text/html"]),
        }
    }
    fn response_bounds(max_response_bytes: usize, mimes: &[&str]) -> TransportResponseBounds {
        TransportResponseBounds {
            max_response_bytes,
            allowed_mime_types: mimes.iter().map(|mime| (*mime).to_owned()).collect(),
        }
    }
    fn transport(connector: RecordingConnector) -> GatewayNetworkTransport<RecordingConnector> {
        GatewayNetworkTransport::new(
            connector,
            NetworkTransportLimits {
                timeout: Duration::from_secs(7),
                max_response_bytes: 4,
            },
        )
    }

    #[tokio::test]
    async fn requires_pinned_ip_and_preserves_logical_tls_host() {
        let connector = RecordingConnector::success();
        let transport = transport(connector);
        let cancellation = ToolCancellation::new();
        assert_eq!(
            transport
                .fetch(fetch("https://docs.example/a?b=c", vec![]), &cancellation)
                .await,
            Err("pinned endpoint required".into())
        );
        transport
            .fetch(
                fetch(
                    "https://docs.example/a?b=c",
                    vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))],
                ),
                &cancellation,
            )
            .await
            .unwrap();
        let requests = transport.connector.requests.lock().unwrap();
        let request = &requests[0];
        assert_eq!(request.logical_host, "docs.example");
        assert_eq!(
            request.resolved_endpoints[0],
            SocketAddr::from(([203, 0, 113, 7], 443))
        );
        assert_eq!(request.path_and_query, "/a?b=c");
        assert_eq!(request.method, PinnedHttpsMethod::Get);
    }
    #[tokio::test]
    async fn constrained_request_has_no_redirect_proxy_headers_or_body_escape_hatch() {
        let connector = RecordingConnector::success();
        let transport = transport(connector);
        let cancellation = ToolCancellation::new();
        transport
            .fetch(
                fetch(
                    "https://docs.example/a",
                    vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))],
                ),
                &cancellation,
            )
            .await
            .unwrap();
        let requests = transport.connector.requests.lock().unwrap();
        let request = &requests[0];
        assert_eq!(request.timeout, Duration::from_secs(7));
        assert_eq!(request.response_bounds.max_response_bytes, 4);
        assert_eq!(
            request.response_bounds.allowed_mime_types,
            BTreeSet::from(["text/html".to_owned()])
        );
        assert!(request.redirects_disabled);
        assert!(request.proxies_disabled);
    }
    #[tokio::test]
    async fn rejects_non_https_credentials_and_unsafe_methods_before_connector() {
        let connector = RecordingConnector::success();
        let transport = transport(connector);
        let cancellation = ToolCancellation::new();
        assert_eq!(
            transport
                .fetch(
                    fetch(
                        "http://docs.example/a",
                        vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))]
                    ),
                    &cancellation
                )
                .await,
            Err("invalid pinned request".into())
        );
        assert_eq!(
            transport
                .fetch(
                    fetch(
                        "https://user@docs.example/a",
                        vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))]
                    ),
                    &cancellation
                )
                .await,
            Err("invalid pinned request".into())
        );
        assert_eq!(
            transport
                .fetch(
                    TransportFetchRequest {
                        url: "https://docs.example/a".into(),
                        method: "POST".into(),
                        resolved_addresses: vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))],
                        response_bounds: response_bounds(4, &["text/html"]),
                    },
                    &cancellation
                )
                .await,
            Err("invalid pinned request".into())
        );
        assert!(transport.connector.requests.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn maps_connector_errors_without_leaking_details() {
        let connector = RecordingConnector {
            requests: Mutex::new(vec![]),
            response: Mutex::new(Err(PinnedHttpsError::TimedOut)),
        };
        let transport = transport(connector);
        let cancellation = ToolCancellation::new();
        assert_eq!(
            transport
                .fetch(
                    fetch(
                        "https://docs.example/a",
                        vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))]
                    ),
                    &cancellation
                )
                .await,
            Err("network timeout".into())
        );
    }
    #[tokio::test]
    async fn policy_byte_limit_reaches_the_pinned_connector() {
        let connector = RecordingConnector::success();
        let transport = transport(connector);
        let mut request = fetch(
            "https://docs.example/a",
            vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))],
        );
        request.response_bounds.max_response_bytes = 2;
        let cancellation = ToolCancellation::new();

        transport.fetch(request, &cancellation).await.unwrap();

        let requests = transport.connector.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].response_bounds.max_response_bytes, 2);
    }
    #[tokio::test]
    async fn policy_mime_is_rejected_before_search_evidence_is_parsed() {
        let connector = RecordingConnector {
            requests: Mutex::new(vec![]),
            response: Mutex::new(Ok(PinnedHttpsResponse {
                status: 200,
                headers: BTreeSet::from([("content-type".into(), "text/html".into())]),
                // This would produce "invalid search response" if parsing ran.
                body: b"not json".to_vec(),
            })),
        };
        let transport = transport(connector);
        let cancellation = ToolCancellation::new();

        assert_eq!(
            transport
                .search(
                    TransportSearchRequest {
                        provider_host: "search.example".into(),
                        query: "bounded".into(),
                        resolved_addresses: vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))],
                        response_bounds: response_bounds(16, &["application/json"]),
                    },
                    &cancellation
                )
                .await,
            Err("response content type denied".into())
        );
    }
    #[tokio::test]
    async fn response_body_limit_is_rechecked_for_connector_test_doubles() {
        let connector = RecordingConnector {
            requests: Mutex::new(vec![]),
            response: Mutex::new(Ok(PinnedHttpsResponse {
                status: 200,
                headers: BTreeSet::from([("content-type".into(), "text/html".into())]),
                body: b"oversized".to_vec(),
            })),
        };
        let transport = transport(connector);
        let mut request = fetch(
            "https://docs.example/a",
            vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))],
        );
        request.response_bounds.max_response_bytes = 2;
        let cancellation = ToolCancellation::new();

        assert_eq!(
            transport.fetch(request, &cancellation).await,
            Err("response exceeds byte limit".into())
        );
    }
    #[test]
    fn metadata_validation_rejects_declared_oversize_before_body_read() {
        let headers = BTreeSet::from([
            ("content-type".to_owned(), "text/html".to_owned()),
            ("content-length".to_owned(), "3".to_owned()),
        ]);
        assert_eq!(
            validate_response_metadata(200, &headers, &response_bounds(2, &["text/html"])),
            Err(PinnedHttpsError::ResponseTooLarge)
        );
    }
    #[test]
    fn read_limit_rejects_overflow() {
        let mut body = b"12".to_vec();
        assert_eq!(
            append_limited(&mut body, b"345", 4),
            Err(PinnedHttpsError::ResponseTooLarge)
        );
    }

    struct StalledConnector {
        started: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl PinnedHttpsConnector for StalledConnector {
        async fn execute(
            &self,
            _: PinnedHttpsRequest,
            cancellation: &ToolCancellation,
        ) -> Result<PinnedHttpsResponse, PinnedHttpsError> {
            self.started.notify_one();
            cancellation.cancelled().await;
            Err(PinnedHttpsError::Cancelled)
        }
    }

    #[tokio::test]
    async fn cancellation_unblocks_an_in_flight_connector_without_returning_a_response() {
        let started = Arc::new(tokio::sync::Notify::new());
        let transport = Arc::new(GatewayNetworkTransport::new(
            StalledConnector {
                started: started.clone(),
            },
            NetworkTransportLimits::default(),
        ));
        let cancellation = ToolCancellation::new();
        let request = fetch(
            "https://docs.example/a",
            vec![IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))],
        );
        let operation = {
            let transport = transport.clone();
            let cancellation = cancellation.clone();
            tokio::spawn(async move { transport.fetch(request, &cancellation).await })
        };
        started.notified().await;
        cancellation.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(200), operation)
                .await
                .expect("cancellation must not wait for the transport timeout")
                .unwrap(),
            Err("network operation cancelled".into())
        );
    }
}
