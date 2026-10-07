//! Fail-closed DNS adapter for Explorer network requests.
//!
//! [`NetworkGateway`](super::network_gateway::NetworkGateway) deliberately
//! depends on the small [`HostResolver`](super::network_gateway::HostResolver)
//! seam instead of constructing a DNS client itself.  This module is the
//! production adapter for that seam.  It keeps the system resolver behind a
//! tiny, injectable lookup interface and returns only a bounded list of
//! validated public IP addresses.  It does not cache answers, expose the raw
//! resolver output, or accept an alternate port/proxy/redirect route.

use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
};

use super::network_gateway::{is_denied_ip, HostResolver};

/// The only port permitted by the Explorer HTTPS transport.
pub const EXPLORER_HTTPS_PORT: u16 = 443;
/// Bound the amount of address material that can cross the gateway seam.
pub const MAX_RESOLVED_ADDRESSES: usize = 8;

/// The internal DNS seam.  A production adapter uses [`SystemDnsLookup`];
/// tests can inject a deterministic lookup without ever exposing raw DNS
/// results to gateway callers.
pub(crate) trait DnsLookup: Send + Sync {
    fn lookup(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String>;
}

/// Uses the operating system resolver for one uncached lookup.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemDnsLookup;

impl DnsLookup for SystemDnsLookup {
    fn lookup(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        (host, port)
            .to_socket_addrs()
            .map(|addresses| addresses.collect())
            .map_err(|error| format!("DNS lookup failed: {error}"))
    }
}

/// Safe [`HostResolver`] implementation for the Explorer gateway.
///
/// The interface intentionally accepts only a bare hostname through
/// `HostResolver::resolve`; `resolve_at_port` is exposed for transport seams
/// that carry a port explicitly and accepts HTTPS 443 only.  Every call
/// invokes the injected lookup again, so answers are not cached between
/// requests.  Any unsafe answer in a multi-address result rejects the whole
/// result rather than returning a partially trusted set.
#[derive(Debug, Clone)]
pub struct SafeHostResolver<L = SystemDnsLookup> {
    lookup: L,
    max_addresses: usize,
}

impl SafeHostResolver<SystemDnsLookup> {
    /// Build the production resolver.  It has no process-wide state or cache.
    pub fn system() -> Self {
        Self::new(SystemDnsLookup)
    }
}

impl<L> SafeHostResolver<L> {
    /// Build a resolver with an injected lookup adapter.
    ///
    /// The address bound is fixed to [`MAX_RESOLVED_ADDRESSES`] and cannot be
    /// widened by a worker.  Keeping the bound here makes the module's seam
    /// deep: callers learn one constructor and one `HostResolver` method while
    /// policy, normalization and address filtering remain local.
    pub fn new(lookup: L) -> Self {
        Self {
            lookup,
            max_addresses: MAX_RESOLVED_ADDRESSES,
        }
    }

    /// Resolve a hostname for the only permitted Explorer port (443).
    pub fn resolve_at_port(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String>
    where
        L: DnsLookup,
    {
        if port != EXPLORER_HTTPS_PORT {
            return Err("only HTTPS port 443 is allowed".into());
        }
        let normalized = normalize_host(host)?;
        let raw = self.lookup.lookup(&normalized, port)?;
        validate_and_bound(raw, port, self.max_addresses)
    }
}

impl Default for SafeHostResolver<SystemDnsLookup> {
    fn default() -> Self {
        Self::system()
    }
}

impl<L> HostResolver for SafeHostResolver<L>
where
    L: DnsLookup,
{
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        self.resolve_at_port(host, EXPLORER_HTTPS_PORT)
    }
}

fn normalize_host(raw: &str) -> Result<String, String> {
    let host = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 || host.parse::<IpAddr>().is_ok() {
        return Err("host must be a non-empty bare DNS name".into());
    }
    // A port, URL, bracketed IPv6 literal, userinfo and wildcard are all
    // rejected before entering the system resolver.  Gateway validation also
    // performs this check, but keeping it at this seam prevents accidental
    // direct use from creating a DNS/transport bypass.
    if !host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
    {
        return Err("host must be a bare DNS name".into());
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-') {
            return Err("host contains an invalid DNS label".into());
        }
    }
    Ok(host)
}

fn validate_and_bound(
    raw: Vec<SocketAddr>,
    expected_port: u16,
    max_addresses: usize,
) -> Result<Vec<IpAddr>, String> {
    if max_addresses == 0 {
        return Err("address bound is invalid".into());
    }
    let mut addresses = Vec::with_capacity(raw.len().min(max_addresses));
    let mut unique = BTreeSet::new();
    for socket in raw {
        if socket.port() != expected_port {
            return Err("DNS answer used a non-standard port".into());
        }
        let ip = socket.ip();
        if is_denied_ip(&ip) {
            return Err("DNS answer contained an unsafe address".into());
        }
        if unique.insert(ip) {
            if addresses.len() == max_addresses {
                return Err("DNS answer exceeded the address bound".into());
            }
            addresses.push(ip);
        }
    }
    if addresses.is_empty() {
        return Err("DNS answer was empty".into());
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct FakeLookup {
        answers: Arc<Mutex<Vec<Vec<SocketAddr>>>>,
        calls: Arc<Mutex<Vec<(String, u16)>>>,
    }

    impl FakeLookup {
        fn new(answers: Vec<Vec<SocketAddr>>) -> Self {
            Self {
                answers: Arc::new(Mutex::new(answers)),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<(String, u16)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl DnsLookup for FakeLookup {
        fn lookup(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
            self.calls.lock().unwrap().push((host.into(), port));
            self.answers
                .lock()
                .unwrap()
                .drain(..1)
                .next()
                .ok_or_else(|| "no fake answer".into())
        }
    }

    fn socket(ip: &str) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), EXPLORER_HTTPS_PORT)
    }

    #[test]
    fn allows_public_addresses_and_bounds_duplicates() {
        let lookup = FakeLookup::new(vec![vec![
            socket("8.8.8.8"),
            socket("2001:4860:4860::8888"),
            socket("8.8.8.8"),
        ]]);
        let resolver = SafeHostResolver::new(lookup.clone());
        let addresses = resolver.resolve("Docs.Example.").unwrap();
        assert_eq!(addresses.len(), 2);
        assert_eq!(lookup.calls(), vec![("docs.example".into(), 443)]);
    }

    #[test]
    fn denies_private_loopback_link_local_multicast_unspecified_reserved_and_mapped() {
        for ip in [
            "10.0.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "224.0.0.1",
            "0.0.0.0",
            "192.0.2.1",
            "240.0.0.1",
            "::",
            "::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            let resolver = SafeHostResolver::new(FakeLookup::new(vec![vec![socket(ip)]]));
            assert!(
                resolver.resolve("example.test").is_err(),
                "{ip} must be denied"
            );
        }
    }

    #[test]
    fn re_resolves_every_request_without_cache() {
        let lookup = FakeLookup::new(vec![vec![socket("8.8.8.8")], vec![socket("1.1.1.1")]]);
        let resolver = SafeHostResolver::new(lookup.clone());
        assert_eq!(
            resolver.resolve("example.test").unwrap(),
            vec!["8.8.8.8".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            resolver.resolve("example.test").unwrap(),
            vec!["1.1.1.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(lookup.calls().len(), 2);
    }

    #[test]
    fn rejects_empty_host_ports_and_non_standard_answer_ports() {
        let resolver = SafeHostResolver::new(FakeLookup::new(vec![vec![socket("8.8.8.8")]]));
        assert!(resolver.resolve("").is_err());
        assert!(resolver.resolve("example.test:443").is_err());
        assert!(resolver.resolve_at_port("example.test", 80).is_err());

        let wrong_port = FakeLookup::new(vec![vec![SocketAddr::new(
            "8.8.8.8".parse().unwrap(),
            8443,
        )]]);
        assert!(SafeHostResolver::new(wrong_port)
            .resolve("example.test")
            .is_err());
    }

    #[test]
    fn rejects_more_than_the_bounded_number_of_addresses() {
        let addresses = (1..=MAX_RESOLVED_ADDRESSES + 1)
            .map(|octet| socket(&format!("8.8.8.{octet}")))
            .collect();
        let resolver = SafeHostResolver::new(FakeLookup::new(vec![addresses]));
        assert!(resolver.resolve("example.test").is_err());
    }
}
