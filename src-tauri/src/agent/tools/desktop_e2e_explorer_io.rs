//! Deterministic physical I/O for the desktop Explorer regression harness.
//!
//! This adapter is compiled only into the explicit desktop-E2E binary.  It
//! replaces DNS and HTTPS sockets below the production gateway, so the test
//! still exercises the normal plan, approval, scope, lease, parser, worker,
//! and delivery paths without a credential or network dependency.

use std::{
    collections::{BTreeSet, VecDeque},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Mutex,
};

use async_trait::async_trait;
use serde::Deserialize;

use super::{
    network_gateway::TransportResponseBounds,
    network_transport::{
        GatewayNetworkTransport, NetworkTransportLimits, PinnedHttpsConnector, PinnedHttpsError,
        PinnedHttpsMethod, PinnedHttpsRequest, PinnedHttpsResponse,
    },
    safe_host_resolver::{DnsLookup, SafeHostResolver},
    tool::ToolCancellation,
};

const HARNESS_ENV: &str = "ANGELBOT_DESKTOP_E2E";
const SCRIPT_ENV: &str = "ANGELBOT_DESKTOP_E2E_EXPLORER_SCRIPT";
const FIXTURE_IP: Ipv4Addr = Ipv4Addr::new(93, 184, 216, 34);

/// The one test-only constructor exposed to application composition.
pub(crate) struct DesktopE2eExplorerIo {
    connector: ScriptedPinnedHttpsConnector,
    resolver: SafeHostResolver<ScriptedDnsLookup>,
}

impl DesktopE2eExplorerIo {
    /// Absence means a non-harness desktop-E2E build should retain production
    /// I/O.  A real harness marker without a valid script always fails closed;
    /// it must never silently contact the network.
    pub(crate) fn from_environment() -> Result<Option<Self>, String> {
        let harness = std::env::var(HARNESS_ENV).ok().as_deref() == Some("1");
        let script = std::env::var(SCRIPT_ENV).ok();
        match (harness, script) {
            (false, None) => Ok(None),
            (false, Some(_)) => {
                Err("desktop E2E Explorer script requires the harness marker".into())
            }
            (true, None) => Err("desktop E2E Explorer script is required by the harness".into()),
            (true, Some(script)) => Self::from_script(&script).map(Some),
        }
    }

    fn from_script(script: &str) -> Result<Self, String> {
        let script: ExplorerIoScript = serde_json::from_str(script)
            .map_err(|_| "desktop E2E Explorer script is invalid".to_owned())?;
        if script.exchanges.is_empty() {
            return Err("desktop E2E Explorer script has no exchanges".into());
        }
        let hosts = script
            .exchanges
            .iter()
            .map(ExplorerIoExchange::validate)
            .collect::<Result<BTreeSet<_>, _>>()?;
        let connector = ScriptedPinnedHttpsConnector::new(script.exchanges)?;
        Ok(Self {
            connector,
            resolver: SafeHostResolver::new(ScriptedDnsLookup { hosts }),
        })
    }

    pub(crate) fn into_network_io(
        self,
    ) -> (
        GatewayNetworkTransport<ScriptedPinnedHttpsConnector>,
        SafeHostResolver<ScriptedDnsLookup>,
    ) {
        (
            GatewayNetworkTransport::new(self.connector, NetworkTransportLimits::default()),
            self.resolver,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplorerIoScript {
    exchanges: Vec<ExplorerIoExchange>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplorerIoExchange {
    host: String,
    method: String,
    path_and_query: String,
    status: u16,
    content_type: String,
    body: String,
}

impl ExplorerIoExchange {
    fn validate(&self) -> Result<String, String> {
        let host = self.host.trim().to_ascii_lowercase();
        if host.is_empty()
            || host != self.host
            || host.contains('/')
            || host.contains("://")
            || host.chars().any(char::is_whitespace)
            || !matches!(self.method.as_str(), "GET" | "HEAD")
            || !self.path_and_query.starts_with('/')
            || self.status < 100
            || self.content_type.trim().is_empty()
        {
            return Err("desktop E2E Explorer exchange is invalid".into());
        }
        Ok(host)
    }
}

pub(crate) struct ScriptedPinnedHttpsConnector {
    exchanges: Mutex<VecDeque<ExplorerIoExchange>>,
}

impl ScriptedPinnedHttpsConnector {
    fn new(exchanges: Vec<ExplorerIoExchange>) -> Result<Self, String> {
        for exchange in &exchanges {
            exchange.validate()?;
        }
        Ok(Self {
            exchanges: Mutex::new(exchanges.into()),
        })
    }
}

#[async_trait]
impl PinnedHttpsConnector for ScriptedPinnedHttpsConnector {
    async fn execute(
        &self,
        request: PinnedHttpsRequest,
        cancellation: &ToolCancellation,
    ) -> Result<PinnedHttpsResponse, PinnedHttpsError> {
        if cancellation.is_cancelled() {
            return Err(PinnedHttpsError::Cancelled);
        }
        if request.port != 443
            || request.resolved_endpoints.is_empty()
            || request
                .resolved_endpoints
                .iter()
                .any(|endpoint| endpoint.port() != 443)
            || !request.redirects_disabled
            || !request.proxies_disabled
        {
            return Err(PinnedHttpsError::InvalidRequest);
        }

        let mut exchanges = self
            .exchanges
            .lock()
            .map_err(|_| PinnedHttpsError::Transport)?;
        let exchange = exchanges.front().ok_or(PinnedHttpsError::Transport)?;
        let method = match request.method {
            PinnedHttpsMethod::Get => "GET",
            PinnedHttpsMethod::Head => "HEAD",
        };
        if exchange.host != request.logical_host
            || exchange.method != method
            || exchange.path_and_query != request.path_and_query
        {
            return Err(PinnedHttpsError::InvalidRequest);
        }
        if cancellation.is_cancelled() {
            return Err(PinnedHttpsError::Cancelled);
        }
        let exchange = exchanges
            .pop_front()
            .expect("the scripted exchange was present while locked");
        drop(exchanges);

        let body = exchange.body.into_bytes();
        validate_fixture_response(&body, &exchange.content_type, &request.response_bounds)?;
        Ok(PinnedHttpsResponse {
            status: exchange.status,
            headers: BTreeSet::from([("content-type".into(), exchange.content_type)]),
            body,
        })
    }
}

fn validate_fixture_response(
    body: &[u8],
    content_type: &str,
    bounds: &TransportResponseBounds,
) -> Result<(), PinnedHttpsError> {
    if body.len() > bounds.max_response_bytes {
        return Err(PinnedHttpsError::ResponseTooLarge);
    }
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if mime.is_empty() || !bounds.allowed_mime_types.contains(&mime) {
        return Err(PinnedHttpsError::ContentTypeDenied);
    }
    Ok(())
}

pub(crate) struct ScriptedDnsLookup {
    hosts: BTreeSet<String>,
}

impl DnsLookup for ScriptedDnsLookup {
    fn lookup(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        if port != 443 || !self.hosts.contains(host) {
            return Err("desktop E2E Explorer host is not scripted".into());
        }
        Ok(vec![SocketAddr::new(IpAddr::V4(FIXTURE_IP), port)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> TransportResponseBounds {
        TransportResponseBounds {
            max_response_bytes: 100,
            allowed_mime_types: BTreeSet::from(["application/json".into()]),
        }
    }

    fn request(path_and_query: &str) -> PinnedHttpsRequest {
        PinnedHttpsRequest {
            logical_host: "docs.example.test".into(),
            port: 443,
            resolved_endpoints: vec![SocketAddr::new(IpAddr::V4(FIXTURE_IP), 443)],
            method: PinnedHttpsMethod::Get,
            path_and_query: path_and_query.into(),
            timeout: std::time::Duration::from_secs(1),
            response_bounds: bounds(),
            redirects_disabled: true,
            proxies_disabled: true,
        }
    }

    #[tokio::test]
    async fn scripted_connector_requires_the_next_exact_authorized_request() {
        let connector = ScriptedPinnedHttpsConnector::new(vec![ExplorerIoExchange {
            host: "docs.example.test".into(),
            method: "GET".into(),
            path_and_query: "/search?q=angelbot-e2e".into(),
            status: 200,
            content_type: "application/json".into(),
            body: r#"{\"results\":[]}"#.into(),
        }])
        .unwrap();
        let cancellation = ToolCancellation::new();

        let mismatch = connector
            .execute(request("/search?q=unexpected"), &cancellation)
            .await;
        assert_eq!(mismatch, Err(PinnedHttpsError::InvalidRequest));
        let response = connector
            .execute(request("/search?q=angelbot-e2e"), &cancellation)
            .await;
        assert!(response.is_ok());
    }
}
