//! Lease-bound, fail-closed delegated network mediation.
//!
//! This is deliberately a small standalone port.  Workers receive neither a
//! browser nor an HTTP client: they submit an intent here, and an injected
//! transport can only receive a URL after policy, lease, DNS and redirect
//! checks have succeeded. Explorer's production worker host injects this port
//! through a deliberately narrowed evidence-only adapter.

use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use async_trait::async_trait;
use chrono::Utc;
use reqwest::Url;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    execution_gateway::{
        CapabilityLease, ExecutionAction, ExecutionDecision, ExecutionGateway, ExecutionIntent,
    },
    shared_db::SharedDb,
    tool::ToolCancellation,
    work_package::{WorkPackage, WorkPackageScope, WorkPackageStatus},
    worker_policy::NetworkAction,
};

pub const SOURCE_POLICY_VERSION: u16 = 1;
pub const GLOBAL_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub const GLOBAL_MAX_REDIRECTS: u8 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePolicy {
    pub id: String,
    pub owner_profile_id: i64,
    pub workspace_key: String,
    pub capability_scope_ref: String,
    pub policy_version: u16,
    pub actions: BTreeSet<NetworkAction>,
    pub allowed_hosts: BTreeSet<String>,
    pub allowed_mime_types: BTreeSet<String>,
    pub max_response_bytes: usize,
    pub max_redirects: u8,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSourcePolicy {
    pub owner_profile_id: i64,
    pub workspace_key: String,
    pub capability_scope_ref: String,
    pub actions: BTreeSet<NetworkAction>,
    pub allowed_hosts: BTreeSet<String>,
    pub allowed_mime_types: BTreeSet<String>,
    pub max_response_bytes: usize,
    pub max_redirects: u8,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SourcePolicyError {
    #[error("invalid source policy: {0}")]
    Invalid(String),
    #[error("source policy not found")]
    NotFound,
    #[error("source policy storage failed: {0}")]
    Storage(String),
}

/// The persistence adapter is intentionally separate from network execution.
pub struct SourcePolicyRepository;
impl SourcePolicyRepository {
    /// Build and validate a policy before a caller chooses how to persist it.
    ///
    /// Foreground confirmation can compose this with a delegation/lease
    /// transaction, while existing administration callers retain `create`.
    pub fn new_policy(new: NewSourcePolicy) -> Result<SourcePolicy, SourcePolicyError> {
        let policy = SourcePolicy {
            id: format!("nsp_{}", Uuid::new_v4().simple()),
            owner_profile_id: new.owner_profile_id,
            workspace_key: new.workspace_key,
            capability_scope_ref: new.capability_scope_ref,
            policy_version: SOURCE_POLICY_VERSION,
            actions: new.actions,
            allowed_hosts: new.allowed_hosts,
            allowed_mime_types: new.allowed_mime_types,
            max_response_bytes: new.max_response_bytes,
            max_redirects: new.max_redirects,
            enabled: true,
        };
        validate_policy(&policy)?;
        Ok(policy)
    }

    pub fn create(
        conn: &Connection,
        new: NewSourcePolicy,
    ) -> Result<SourcePolicy, SourcePolicyError> {
        let policy = Self::new_policy(new)?;
        Self::insert(conn, &policy, Utc::now().timestamp())?;
        Ok(policy)
    }

    /// Persist a previously validated policy within the caller's transaction.
    /// This is the only way a one-time foreground confirmation may create a
    /// policy: the policy cannot outlive a failed delegation issuance.
    pub fn insert_in_tx(
        tx: &Transaction<'_>,
        policy: &SourcePolicy,
        now: i64,
    ) -> Result<(), SourcePolicyError> {
        Self::insert(tx, policy, now)
    }

    fn insert(conn: &Connection, policy: &SourcePolicy, now: i64) -> Result<(), SourcePolicyError> {
        validate_policy(policy)?;
        conn.execute("INSERT INTO delegated_network_source_policies (id,owner_profile_id,workspace_key,capability_scope_ref,policy_version,actions_json,allowed_hosts_json,allowed_mime_types_json,max_response_bytes,max_redirects,enabled,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,?11,?11)", params![policy.id,policy.owner_profile_id,policy.workspace_key,policy.capability_scope_ref,policy.policy_version,encode_actions(&policy.actions)?,encode_set(&policy.allowed_hosts)?,encode_set(&policy.allowed_mime_types)?,policy.max_response_bytes as i64,policy.max_redirects as i64,now]).map_err(storage)?;
        Ok(())
    }
    pub fn load(
        conn: &Connection,
        owner_profile_id: i64,
        workspace_key: &str,
        capability_scope_ref: &str,
    ) -> Result<SourcePolicy, SourcePolicyError> {
        conn.query_row("SELECT id,owner_profile_id,workspace_key,capability_scope_ref,policy_version,actions_json,allowed_hosts_json,allowed_mime_types_json,max_response_bytes,max_redirects,enabled FROM delegated_network_source_policies WHERE owner_profile_id=?1 AND workspace_key=?2 AND capability_scope_ref=?3 AND enabled=1 ORDER BY policy_version DESC LIMIT 1", params![owner_profile_id,workspace_key,capability_scope_ref], |r| {
            Ok(SourcePolicy { id:r.get(0)?, owner_profile_id:r.get(1)?, workspace_key:r.get(2)?, capability_scope_ref:r.get(3)?, policy_version:r.get::<_,i64>(4)? as u16, actions:decode_actions(&r.get::<_,String>(5)?).map_err(to_sql_error)?, allowed_hosts:decode_set(&r.get::<_,String>(6)?).map_err(to_sql_error)?, allowed_mime_types:decode_set(&r.get::<_,String>(7)?).map_err(to_sql_error)?, max_response_bytes:r.get::<_,i64>(8)? as usize, max_redirects:r.get::<_,i64>(9)? as u8, enabled:r.get::<_,i64>(10)? != 0 })
        }).optional().map_err(storage)?.ok_or(SourcePolicyError::NotFound).and_then(|p| { validate_policy(&p)?; Ok(p) })
    }

    /// Load one exact enabled policy by its durable id.  Administration uses
    /// this only after a trusted approval record selected that id; callers do
    /// not receive the id or use it as a user-controlled capability selector.
    pub(crate) fn load_enabled_by_id(
        conn: &Connection,
        policy_id: &str,
    ) -> Result<SourcePolicy, SourcePolicyError> {
        conn.query_row("SELECT id,owner_profile_id,workspace_key,capability_scope_ref,policy_version,actions_json,allowed_hosts_json,allowed_mime_types_json,max_response_bytes,max_redirects,enabled FROM delegated_network_source_policies WHERE id=?1 AND enabled=1", params![policy_id], |r| {
            Ok(SourcePolicy { id:r.get(0)?, owner_profile_id:r.get(1)?, workspace_key:r.get(2)?, capability_scope_ref:r.get(3)?, policy_version:r.get::<_,i64>(4)? as u16, actions:decode_actions(&r.get::<_,String>(5)?).map_err(to_sql_error)?, allowed_hosts:decode_set(&r.get::<_,String>(6)?).map_err(to_sql_error)?, allowed_mime_types:decode_set(&r.get::<_,String>(7)?).map_err(to_sql_error)?, max_response_bytes:r.get::<_,i64>(8)? as usize, max_redirects:r.get::<_,i64>(9)? as u8, enabled:r.get::<_,i64>(10)? != 0 })
        }).optional().map_err(storage)?.ok_or(SourcePolicyError::NotFound).and_then(|p| { validate_policy(&p)?; Ok(p) })
    }

    /// Load all currently enabled policies for one trusted workspace. Approval
    /// provenance is deliberately checked by the delegation-scope module,
    /// whose production lookup adapter is injected into the transport gateway.
    pub fn enabled_for_workspace(
        conn: &Connection,
        owner_profile_id: i64,
        workspace_key: &str,
    ) -> Result<Vec<SourcePolicy>, SourcePolicyError> {
        let mut statement = conn
            .prepare(
                "SELECT id,owner_profile_id,workspace_key,capability_scope_ref,policy_version,actions_json,allowed_hosts_json,allowed_mime_types_json,max_response_bytes,max_redirects,enabled
                 FROM delegated_network_source_policies
                 WHERE owner_profile_id=?1 AND workspace_key=?2 AND enabled=1
                 ORDER BY policy_version DESC, created_at DESC",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map(params![owner_profile_id, workspace_key], |r| {
                Ok(SourcePolicy {
                    id: r.get(0)?,
                    owner_profile_id: r.get(1)?,
                    workspace_key: r.get(2)?,
                    capability_scope_ref: r.get(3)?,
                    policy_version: r.get::<_, i64>(4)? as u16,
                    actions: decode_actions(&r.get::<_, String>(5)?).map_err(to_sql_error)?,
                    allowed_hosts: decode_set(&r.get::<_, String>(6)?).map_err(to_sql_error)?,
                    allowed_mime_types: decode_set(&r.get::<_, String>(7)?)
                        .map_err(to_sql_error)?,
                    max_response_bytes: r.get::<_, i64>(8)? as usize,
                    max_redirects: r.get::<_, i64>(9)? as u8,
                    enabled: r.get::<_, i64>(10)? != 0,
                })
            })
            .map_err(storage)?;
        let mut policies = Vec::new();
        for row in rows {
            let policy = row.map_err(storage)?;
            validate_policy(&policy)?;
            policies.push(policy);
        }
        Ok(policies)
    }

    /// Select an enabled policy which covers every requested public host and
    /// action. Delegation issuance additionally requires a durable
    /// confirmation record before treating this policy as user-approved.
    pub fn find_covering(
        conn: &Connection,
        owner_profile_id: i64,
        workspace_key: &str,
        actions: &BTreeSet<NetworkAction>,
        hosts: &BTreeSet<String>,
    ) -> Result<SourcePolicy, SourcePolicyError> {
        for policy in Self::enabled_for_workspace(conn, owner_profile_id, workspace_key)? {
            if actions.is_subset(&policy.actions) && hosts.is_subset(&policy.allowed_hosts) {
                return Ok(policy);
            }
        }
        Err(SourcePolicyError::NotFound)
    }
}

pub trait SourcePolicyLookup {
    fn load(
        &self,
        scope: &WorkPackageScope,
        scope_ref: &str,
    ) -> Result<SourcePolicy, SourcePolicyError>;
}
impl<'a> SourcePolicyLookup for &'a Connection {
    fn load(
        &self,
        scope: &WorkPackageScope,
        scope_ref: &str,
    ) -> Result<SourcePolicy, SourcePolicyError> {
        SourcePolicyRepository::load(
            self,
            scope.owner_profile_id,
            &scope.workspace_key,
            scope_ref,
        )
    }
}

/// Shared-database adapter for the gateway policy lookup.
///
/// The gateway is synchronous, while the application owns one SQLite
/// connection behind [`SharedDb`].  This adapter keeps that ownership detail
/// outside the gateway: every lookup holds the mutex only for the duration of
/// the repository call and never gives a connection guard to a worker or
/// stores it between requests.  Storage failures are intentionally mapped to
/// `SourcePolicyError::Storage`, so a missing/poisoned database fails closed
/// through the gateway's existing policy denial path.
#[derive(Clone)]
pub struct SharedDbSourcePolicyLookup {
    db: SharedDb,
}

impl SharedDbSourcePolicyLookup {
    pub fn new(db: SharedDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &SharedDb {
        &self.db
    }
}

impl SourcePolicyLookup for SharedDbSourcePolicyLookup {
    fn load(
        &self,
        scope: &WorkPackageScope,
        scope_ref: &str,
    ) -> Result<SourcePolicy, SourcePolicyError> {
        self.db
            .with_conn(|connection| {
                SourcePolicyRepository::load(
                    connection,
                    scope.owner_profile_id,
                    &scope.workspace_key,
                    scope_ref,
                )
            })
            .map_err(|error| SourcePolicyError::Storage(error.to_string()))?
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIntent {
    pub provider_host: String,
    pub query: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchIntent {
    pub url: String,
    pub method: String,
}
/// Immutable response constraints derived from the source policy after the
/// gateway has authorized an operation.  A worker never supplies these
/// values: transports receive them only with an already-authorized request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportResponseBounds {
    pub max_response_bytes: usize,
    pub allowed_mime_types: BTreeSet<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportSearchRequest {
    pub provider_host: String,
    pub query: String,
    pub resolved_addresses: Vec<IpAddr>,
    pub response_bounds: TransportResponseBounds,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportFetchRequest {
    pub url: String,
    pub method: String,
    pub resolved_addresses: Vec<IpAddr>,
    pub response_bounds: TransportResponseBounds,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportSearchResponse {
    /// Normalized MIME type of the parsed response body.
    pub content_type: String,
    /// Number of bytes accepted before the transport parsed the evidence.
    pub byte_count: usize,
    pub results: Vec<TransportSearchResult>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportFetchResponse {
    pub status: u16,
    pub headers: BTreeSet<(String, String)>,
    pub body: Vec<u8>,
}

pub trait HostResolver {
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String>;
}
#[async_trait]
pub trait NetworkTransport: Send + Sync {
    /// Implementations must only connect to `resolved_addresses`, preserve TLS
    /// SNI for the validated host, and never attach ambient credentials.
    async fn search(
        &self,
        request: TransportSearchRequest,
        cancellation: &ToolCancellation,
    ) -> Result<TransportSearchResponse, String>;
    async fn fetch(
        &self,
        request: TransportFetchRequest,
        cancellation: &ToolCancellation,
    ) -> Result<TransportFetchResponse, String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTrust {
    Untrusted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkAuditMetadata {
    pub policy_id: String,
    pub policy_version: u16,
    pub action: String,
    pub request_digest: String,
    pub final_host: Option<String>,
    pub final_url_digest: Option<String>,
    pub byte_count: usize,
    pub redirect_count: u8,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedSearchEvidence {
    pub trust: EvidenceTrust,
    pub results: Vec<TransportSearchResult>,
    pub audit: NetworkAuditMetadata,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedFetchEvidence {
    pub trust: EvidenceTrust,
    pub body: Vec<u8>,
    pub content_type: String,
    pub audit: NetworkAuditMetadata,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NetworkDenied {
    #[error("network operation cancelled")]
    Cancelled,
    #[error("gateway authorization denied: {0:?}")]
    Gateway(super::execution_gateway::ExecutionDenied),
    #[error("work package scope or state denied")]
    WorkPackageDenied,
    #[error("source policy denied: {0}")]
    Policy(String),
    #[error("invalid network intent: {0}")]
    Intent(String),
    #[error("DNS denied: {0}")]
    Dns(String),
    #[error("transport failed")]
    Transport,
    #[error("response denied: {0}")]
    Response(String),
}

pub struct NetworkGateway<P, T, R> {
    policies: P,
    transport: T,
    resolver: R,
}
impl<P, T, R> NetworkGateway<P, T, R>
where
    P: SourcePolicyLookup,
    T: NetworkTransport,
    R: HostResolver,
{
    pub fn new(policies: P, transport: T, resolver: R) -> Self {
        Self {
            policies,
            transport,
            resolver,
        }
    }

    /// Load the persistent source policy used to decide which read-only
    /// network schemas may be offered.  This is introspection only; every
    /// request must still pass `search` or `fetch` authorization.
    pub fn source_policy_for(
        &self,
        package: &WorkPackage,
        scope: &WorkPackageScope,
    ) -> Result<SourcePolicy, NetworkDenied> {
        if package.scope != *scope
            || package.status != WorkPackageStatus::Active
            || package.capability_scope_ref.trim().is_empty()
        {
            return Err(NetworkDenied::WorkPackageDenied);
        }
        let policy = self
            .policies
            .load(scope, &package.capability_scope_ref)
            .map_err(|e| NetworkDenied::Policy(e.to_string()))?;
        if policy.policy_version != SOURCE_POLICY_VERSION || !policy.enabled {
            return Err(NetworkDenied::Policy("policy is not enabled".into()));
        }
        Ok(policy)
    }
    pub async fn search(
        &self,
        lease: &CapabilityLease,
        package: &WorkPackage,
        scope: &WorkPackageScope,
        intent: SearchIntent,
        now_ms: i64,
        cancellation: &ToolCancellation,
    ) -> Result<UntrustedSearchEvidence, NetworkDenied> {
        ensure_not_cancelled(cancellation)?;
        if intent.query.trim().is_empty() {
            return Err(NetworkDenied::Intent("query is required".into()));
        }
        let policy = self.authorize(
            lease,
            package,
            scope,
            NetworkAction::Search,
            "network.search",
            &intent.provider_host,
            &format!("search:{}", digest(&intent.query)),
            now_ms,
        )?;
        let host = validated_host(&intent.provider_host)?;
        let addresses = self.resolve(&host)?;
        ensure_not_cancelled(cancellation)?;
        let response_bounds = response_bounds(&policy);
        let response = self
            .transport
            .search(
                TransportSearchRequest {
                    provider_host: host,
                    query: intent.query.clone(),
                    resolved_addresses: addresses,
                    response_bounds,
                },
                cancellation,
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                ensure_not_cancelled(cancellation)?;
                return Err(NetworkDenied::Transport);
            }
        };
        ensure_not_cancelled(cancellation)?;
        let policy = self.recheck_response_policy(package, scope, &policy)?;
        validate_search_response(&policy, &response)?;
        Ok(UntrustedSearchEvidence {
            trust: EvidenceTrust::Untrusted,
            results: response.results,
            audit: audit(
                &policy,
                "search",
                &intent.query,
                None,
                response.byte_count,
                0,
            ),
        })
    }
    pub async fn fetch(
        &self,
        lease: &CapabilityLease,
        package: &WorkPackage,
        scope: &WorkPackageScope,
        intent: FetchIntent,
        now_ms: i64,
        cancellation: &ToolCancellation,
    ) -> Result<UntrustedFetchEvidence, NetworkDenied> {
        ensure_not_cancelled(cancellation)?;
        let method = intent.method.trim().to_ascii_uppercase();
        if method != "GET" && method != "HEAD" {
            return Err(NetworkDenied::Intent("only GET or HEAD is allowed".into()));
        }
        let first = parse_url(&intent.url)?;
        let mut policy = self.authorize(
            lease,
            package,
            scope,
            NetworkAction::Fetch,
            "network.fetch",
            first.host_str().unwrap(),
            &format!("fetch:{method}:{}", digest(first.as_str())),
            now_ms,
        )?;
        let mut url = first;
        let mut redirects = 0;
        loop {
            ensure_not_cancelled(cancellation)?;
            let host = validated_host(url.host_str().unwrap())?;
            ensure_policy_host(&policy, &host)?;
            let addresses = self.resolve(&host)?;
            ensure_not_cancelled(cancellation)?;
            // Every redirect is a new outbound destination.  Do not let an
            // allowlisted source policy widen the concrete attempt lease.
            authorize_lease(
                lease,
                "network.fetch",
                &host,
                &format!("fetch-hop:{}", digest(url.as_str())),
                now_ms,
            )?;
            let response = self
                .transport
                .fetch(
                    TransportFetchRequest {
                        url: url.to_string(),
                        method: method.clone(),
                        resolved_addresses: addresses,
                        response_bounds: response_bounds(&policy),
                    },
                    cancellation,
                )
                .await;
            let response = match response {
                Ok(response) => response,
                Err(_) => {
                    ensure_not_cancelled(cancellation)?;
                    return Err(NetworkDenied::Transport);
                }
            };
            ensure_not_cancelled(cancellation)?;
            policy = self.recheck_response_policy(package, scope, &policy)?;
            if (300..400).contains(&response.status) {
                if redirects >= policy.max_redirects {
                    return Err(NetworkDenied::Response("redirect limit exceeded".into()));
                }
                let location = header(&response.headers, "location")
                    .ok_or_else(|| NetworkDenied::Response("redirect without location".into()))?;
                url = url
                    .join(location)
                    .map_err(|_| NetworkDenied::Response("invalid redirect location".into()))?;
                parse_checked_url(&url)?;
                redirects += 1;
                continue;
            }
            if !(200..300).contains(&response.status) {
                return Err(NetworkDenied::Response("non-success response".into()));
            }
            // The production transport applies these bounds before it parses
            // or returns evidence.  Keep this validation at the gateway too:
            // alternate transports used by tests or future connectors must
            // not be able to widen a policy-derived response envelope.
            let mime = validate_fetch_response(&policy, &response)?;
            let bytes = response.body.len();
            return Ok(UntrustedFetchEvidence {
                trust: EvidenceTrust::Untrusted,
                body: response.body,
                content_type: mime,
                audit: audit(&policy, "fetch", url.as_str(), Some(&url), bytes, redirects),
            });
        }
    }
    fn authorize(
        &self,
        lease: &CapabilityLease,
        package: &WorkPackage,
        scope: &WorkPackageScope,
        action: NetworkAction,
        tool: &str,
        host: &str,
        fingerprint: &str,
        now_ms: i64,
    ) -> Result<SourcePolicy, NetworkDenied> {
        if package.scope != *scope
            || package.status != WorkPackageStatus::Active
            || package.capability_scope_ref.trim().is_empty()
        {
            return Err(NetworkDenied::WorkPackageDenied);
        }
        let policy = self
            .policies
            .load(scope, &package.capability_scope_ref)
            .map_err(|e| NetworkDenied::Policy(e.to_string()))?;
        if policy.policy_version != SOURCE_POLICY_VERSION
            || !policy.enabled
            || !policy.actions.contains(&action)
        {
            return Err(NetworkDenied::Policy("action is not enabled".into()));
        }
        ensure_policy_host(&policy, &validated_host(host)?)?;
        match ExecutionGateway::authorize(
            lease,
            &ExecutionIntent {
                tool_id: tool.into(),
                action: ExecutionAction::Network {
                    host: host.into(),
                    request_fingerprint: fingerprint.into(),
                },
            },
            now_ms,
        ) {
            ExecutionDecision::Allow(_) => Ok(policy),
            ExecutionDecision::Deny(reason) => Err(NetworkDenied::Gateway(reason)),
        }
    }
    /// A request can block in the transport while its source approval is
    /// revoked or its policy changes. Re-read the approval-aware policy before
    /// interpreting the returned bytes, following a redirect, or exposing any
    /// evidence. The concrete worker host performs the companion live-lease
    /// snapshot check before it projects this evidence to the worker.
    fn recheck_response_policy(
        &self,
        package: &WorkPackage,
        scope: &WorkPackageScope,
        expected: &SourcePolicy,
    ) -> Result<SourcePolicy, NetworkDenied> {
        let current = self.source_policy_for(package, scope)?;
        if current != *expected {
            return Err(NetworkDenied::Policy(
                "network policy changed while request was in flight".into(),
            ));
        }
        Ok(current)
    }
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, NetworkDenied> {
        let addresses = self
            .resolver
            .resolve(host)
            .map_err(|_| NetworkDenied::Dns("resolution failed".into()))?;
        if addresses.is_empty() || addresses.iter().any(is_denied_ip) {
            return Err(NetworkDenied::Dns("unsafe or empty DNS answer".into()));
        }
        Ok(addresses)
    }
}

fn ensure_not_cancelled(cancellation: &ToolCancellation) -> Result<(), NetworkDenied> {
    if cancellation.is_cancelled() {
        Err(NetworkDenied::Cancelled)
    } else {
        Ok(())
    }
}

fn parse_url(raw: &str) -> Result<Url, NetworkDenied> {
    Url::parse(raw)
        .map_err(|_| NetworkDenied::Intent("invalid URL".into()))
        .and_then(|url| {
            parse_checked_url(&url)?;
            Ok(url)
        })
}
pub(crate) fn parse_checked_url(url: &Url) -> Result<(), NetworkDenied> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(NetworkDenied::Intent(
            "URL must be credential-free default-port HTTPS".into(),
        ));
    }
    validated_host(url.host_str().unwrap())?;
    Ok(())
}
pub(crate) fn validated_host(raw: &str) -> Result<String, NetworkDenied> {
    let host = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        || host.parse::<IpAddr>().is_ok()
    {
        Err(NetworkDenied::Intent("host must be a bare DNS name".into()))
    } else {
        Ok(host)
    }
}
fn ensure_policy_host(policy: &SourcePolicy, host: &str) -> Result<(), NetworkDenied> {
    if policy.allowed_hosts.contains(host) {
        Ok(())
    } else {
        Err(NetworkDenied::Policy("host not allowlisted".into()))
    }
}
fn response_bounds(policy: &SourcePolicy) -> TransportResponseBounds {
    TransportResponseBounds {
        max_response_bytes: policy.max_response_bytes,
        allowed_mime_types: policy.allowed_mime_types.clone(),
    }
}
fn validate_search_response(
    policy: &SourcePolicy,
    response: &TransportSearchResponse,
) -> Result<(), NetworkDenied> {
    if response.byte_count > policy.max_response_bytes {
        return Err(NetworkDenied::Response("response too large".into()));
    }
    if !policy
        .allowed_mime_types
        .contains(&normalized_mime(&response.content_type))
    {
        return Err(NetworkDenied::Response("content type denied".into()));
    }
    Ok(())
}
fn validate_fetch_response(
    policy: &SourcePolicy,
    response: &TransportFetchResponse,
) -> Result<String, NetworkDenied> {
    if header(&response.headers, "content-disposition")
        .is_some_and(|value| value.to_ascii_lowercase().contains("attachment"))
    {
        return Err(NetworkDenied::Response("downloads are denied".into()));
    }
    let content_type = header(&response.headers, "content-type")
        .ok_or_else(|| NetworkDenied::Response("missing content type".into()))?;
    let mime = normalized_mime(content_type);
    if !policy.allowed_mime_types.contains(&mime) {
        return Err(NetworkDenied::Response("content type denied".into()));
    }
    if response.body.len() > policy.max_response_bytes
        || header(&response.headers, "content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|bytes| bytes > policy.max_response_bytes)
    {
        return Err(NetworkDenied::Response("response too large".into()));
    }
    Ok(mime)
}
fn normalized_mime(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}
fn header<'a>(headers: &'a BTreeSet<(String, String)>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}
fn authorize_lease(
    lease: &CapabilityLease,
    tool: &str,
    host: &str,
    fingerprint: &str,
    now_ms: i64,
) -> Result<(), NetworkDenied> {
    match ExecutionGateway::authorize(
        lease,
        &ExecutionIntent {
            tool_id: tool.into(),
            action: ExecutionAction::Network {
                host: host.into(),
                request_fingerprint: fingerprint.into(),
            },
        },
        now_ms,
    ) {
        ExecutionDecision::Allow(_) => Ok(()),
        ExecutionDecision::Deny(reason) => Err(NetworkDenied::Gateway(reason)),
    }
}
fn audit(
    policy: &SourcePolicy,
    action: &str,
    material: &str,
    final_url: Option<&Url>,
    byte_count: usize,
    redirect_count: u8,
) -> NetworkAuditMetadata {
    NetworkAuditMetadata {
        policy_id: policy.id.clone(),
        policy_version: policy.policy_version,
        action: action.into(),
        request_digest: digest(material),
        final_host: final_url.and_then(|u| u.host_str().map(str::to_owned)),
        final_url_digest: final_url.map(|u| digest(u.as_str())),
        byte_count,
        redirect_count,
    }
}
fn digest(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}
/// Shared concrete-address policy used by both the gateway and its production
/// DNS adapter.  A resolver answer is rejected as a whole when any address
/// matches this policy, preventing a mixed safe/private answer from becoming
/// a partial bypass.
pub(crate) fn is_denied_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => denied_v4(*v),
        IpAddr::V6(v) => denied_v6(*v),
    }
}
fn denied_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 0
        || o[0] == 10
        || o[0] == 127
        || (o[0] == 100 && (64..=127).contains(&o[1]))
        || (o[0] == 169 && o[1] == 254)
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && (o[1] == 0 || o[1] == 168 || (o[1] == 0 && o[2] == 2)))
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19 || o[1] == 51))
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        || o[0] >= 224
}
fn denied_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    ip.to_ipv4().is_some()
        || ip.is_loopback()
        || ip.is_unspecified()
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || s[0] == 0x2001 && s[1] == 0x0db8
        || (s[0] & 0xff00) == 0xff00
}
fn validate_policy(p: &SourcePolicy) -> Result<(), SourcePolicyError> {
    if p.id.trim().is_empty()
        || p.workspace_key.trim().is_empty()
        || p.capability_scope_ref.trim().is_empty()
        || p.policy_version != SOURCE_POLICY_VERSION
        || p.actions.is_empty()
        || p.allowed_hosts.is_empty()
        || p.allowed_mime_types.is_empty()
        || p.max_response_bytes == 0
        || p.max_response_bytes > GLOBAL_MAX_RESPONSE_BYTES
        || p.max_redirects > GLOBAL_MAX_REDIRECTS
    {
        return Err(SourcePolicyError::Invalid(
            "empty, unsupported, or over-global-limit policy".into(),
        ));
    }
    for h in &p.allowed_hosts {
        validated_host(h).map_err(|_| SourcePolicyError::Invalid("invalid allowed host".into()))?;
    }
    for m in &p.allowed_mime_types {
        if !m.contains('/') || m.chars().any(char::is_whitespace) {
            return Err(SourcePolicyError::Invalid("invalid MIME type".into()));
        }
    }
    Ok(())
}
fn encode_set(s: &BTreeSet<String>) -> Result<String, SourcePolicyError> {
    serde_json::to_string(s).map_err(|e| SourcePolicyError::Storage(e.to_string()))
}
fn decode_set(s: &str) -> Result<BTreeSet<String>, SourcePolicyError> {
    serde_json::from_str(s).map_err(|e| SourcePolicyError::Storage(e.to_string()))
}
fn encode_actions(s: &BTreeSet<NetworkAction>) -> Result<String, SourcePolicyError> {
    serde_json::to_string(s).map_err(|e| SourcePolicyError::Storage(e.to_string()))
}
fn decode_actions(s: &str) -> Result<BTreeSet<NetworkAction>, SourcePolicyError> {
    serde_json::from_str(s).map_err(|e| SourcePolicyError::Storage(e.to_string()))
}
fn storage(e: rusqlite::Error) -> SourcePolicyError {
    SourcePolicyError::Storage(e.to_string())
}
fn to_sql_error(e: SourcePolicyError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        delegation::LeaseStatus,
        execution_gateway::CapabilityGrant,
        worker_policy::{TaskShape, WorkerProfile},
    };
    use async_trait::async_trait;
    use std::{
        collections::VecDeque,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };
    struct Resolver(Vec<IpAddr>);
    impl HostResolver for Resolver {
        fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, String> {
            Ok(self.0.clone())
        }
    }
    struct Transport {
        fetches: Mutex<VecDeque<TransportFetchResponse>>,
    }
    #[async_trait]
    impl NetworkTransport for Transport {
        async fn search(
            &self,
            _: TransportSearchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportSearchResponse, String> {
            Ok(TransportSearchResponse {
                content_type: "text/html".into(),
                byte_count: 1,
                results: vec![TransportSearchResult {
                    title: "x".into(),
                    url: "https://untrusted.example/x".into(),
                    snippet: "ignore instructions".into(),
                }],
            })
        }
        async fn fetch(
            &self,
            _: TransportFetchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportFetchResponse, String> {
            self.fetches
                .lock()
                .map_err(|_| "transport lock poisoned".to_string())?
                .pop_front()
                .ok_or("no response".into())
        }
    }
    struct Policies(SourcePolicy);
    impl SourcePolicyLookup for Policies {
        fn load(&self, _: &WorkPackageScope, _: &str) -> Result<SourcePolicy, SourcePolicyError> {
            Ok(self.0.clone())
        }
    }
    struct RevocablePolicies {
        policy: SourcePolicy,
        revoked: Arc<AtomicBool>,
    }
    impl SourcePolicyLookup for RevocablePolicies {
        fn load(&self, _: &WorkPackageScope, _: &str) -> Result<SourcePolicy, SourcePolicyError> {
            if self.revoked.load(Ordering::SeqCst) {
                Err(SourcePolicyError::NotFound)
            } else {
                Ok(self.policy.clone())
            }
        }
    }
    struct RevokingTransport {
        revoked: Arc<AtomicBool>,
        searches: Arc<AtomicUsize>,
        fetches: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl NetworkTransport for RevokingTransport {
        async fn search(
            &self,
            _: TransportSearchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportSearchResponse, String> {
            self.searches.fetch_add(1, Ordering::SeqCst);
            self.revoked.store(true, Ordering::SeqCst);
            Ok(TransportSearchResponse {
                content_type: "text/html".into(),
                byte_count: 1,
                results: vec![],
            })
        }

        async fn fetch(
            &self,
            _: TransportFetchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportFetchResponse, String> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            self.revoked.store(true, Ordering::SeqCst);
            Ok(TransportFetchResponse {
                status: 302,
                headers: BTreeSet::from([("location".into(), "/next".into())]),
                body: vec![],
            })
        }
    }
    fn policy() -> SourcePolicy {
        SourcePolicy {
            id: "p".into(),
            owner_profile_id: 1,
            workspace_key: "w".into(),
            capability_scope_ref: "ref".into(),
            policy_version: 1,
            actions: BTreeSet::from([NetworkAction::Search, NetworkAction::Fetch]),
            allowed_hosts: BTreeSet::from(["search.example".into(), "docs.example".into()]),
            allowed_mime_types: BTreeSet::from(["text/html".into()]),
            max_response_bytes: 10,
            max_redirects: 1,
            enabled: true,
        }
    }
    fn package() -> WorkPackage {
        WorkPackage {
            id: "wp".into(),
            scope: WorkPackageScope {
                session_id: "s".into(),
                owner_profile_id: 1,
                workspace_key: "w".into(),
            },
            task_shape: TaskShape::Explore,
            worker_profile: WorkerProfile::Explorer,
            worker_policy_version: 1,
            scope_digest: "d".into(),
            capability_scope_ref: "ref".into(),
            capability_expires_at: 100,
            candidate_version: 0,
            status: WorkPackageStatus::Active,
        }
    }
    fn lease(status: LeaseStatus, expires: i64) -> CapabilityLease {
        CapabilityLease::from_grant(CapabilityGrant {
            id: "l".into(),
            attempt_id: "a".into(),
            status,
            expires_at_ms: expires,
            read_roots: vec![],
            write_roots: vec![],
            tool_allowlist: vec!["network.search".into(), "network.fetch".into()],
            network_hosts: vec!["search.example".into(), "docs.example".into()],
        })
        .unwrap()
    }
    fn response(status: u16, headers: &[(&str, &str)], body: &[u8]) -> TransportFetchResponse {
        TransportFetchResponse {
            status,
            headers: headers
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            body: body.to_vec(),
        }
    }
    fn gateway(
        responses: Vec<TransportFetchResponse>,
        ips: Vec<IpAddr>,
    ) -> NetworkGateway<Policies, Transport, Resolver> {
        NetworkGateway::new(
            Policies(policy()),
            Transport {
                fetches: Mutex::new(responses.into()),
            },
            Resolver(ips),
        )
    }
    fn public() -> Vec<IpAddr> {
        vec!["8.8.8.8".parse().unwrap()]
    }
    #[tokio::test]
    async fn search_and_fetch_are_untrusted_and_audited() {
        let p = package();
        let cancellation = ToolCancellation::new();
        let g = gateway(
            vec![response(200, &[("content-type", "text/html")], b"ok")],
            public(),
        );
        let s = g
            .search(
                &lease(LeaseStatus::Active, 10),
                &p,
                &p.scope,
                SearchIntent {
                    provider_host: "search.example".into(),
                    query: "q".into(),
                },
                1,
                &cancellation,
            )
            .await
            .unwrap();
        assert_eq!(s.trust, EvidenceTrust::Untrusted);
        assert_ne!(s.audit.request_digest, "q");
        let f = g
            .fetch(
                &lease(LeaseStatus::Active, 10),
                &p,
                &p.scope,
                FetchIntent {
                    url: "https://docs.example/a".into(),
                    method: "GET".into(),
                },
                1,
                &cancellation,
            )
            .await
            .unwrap();
        assert_eq!(f.trust, EvidenceTrust::Untrusted);
        assert_eq!(f.body, b"ok");
    }
    #[tokio::test]
    async fn post_transport_policy_recheck_discards_revoked_search_and_redirect_response() {
        let p = package();
        let cancellation = ToolCancellation::new();
        let revoked = Arc::new(AtomicBool::new(false));
        let searches = Arc::new(AtomicUsize::new(0));
        let fetches = Arc::new(AtomicUsize::new(0));
        let gateway = NetworkGateway::new(
            RevocablePolicies {
                policy: policy(),
                revoked: revoked.clone(),
            },
            RevokingTransport {
                revoked: revoked.clone(),
                searches: searches.clone(),
                fetches: fetches.clone(),
            },
            Resolver(public()),
        );

        assert!(matches!(
            gateway
                .search(
                    &lease(LeaseStatus::Active, 10),
                    &p,
                    &p.scope,
                    SearchIntent {
                        provider_host: "search.example".into(),
                        query: "revocation boundary".into(),
                    },
                    1,
                    &cancellation,
                )
                .await,
            Err(NetworkDenied::Policy(_))
        ));
        assert_eq!(searches.load(Ordering::SeqCst), 1);

        revoked.store(false, Ordering::SeqCst);
        assert!(matches!(
            gateway
                .fetch(
                    &lease(LeaseStatus::Active, 10),
                    &p,
                    &p.scope,
                    FetchIntent {
                        url: "https://docs.example/start".into(),
                        method: "GET".into(),
                    },
                    1,
                    &cancellation,
                )
                .await,
            Err(NetworkDenied::Policy(_))
        ));
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    struct CancellingRedirectTransport {
        fetches: AtomicUsize,
    }

    #[async_trait]
    impl NetworkTransport for CancellingRedirectTransport {
        async fn search(
            &self,
            _: TransportSearchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportSearchResponse, String> {
            Err("search is not used in this test".into())
        }

        async fn fetch(
            &self,
            _: TransportFetchRequest,
            cancellation: &ToolCancellation,
        ) -> Result<TransportFetchResponse, String> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            cancellation.cancel();
            Ok(response(302, &[("location", "/next")], b""))
        }
    }

    #[tokio::test]
    async fn cancellation_after_a_redirect_response_never_issues_the_next_hop() {
        let p = package();
        let transport = CancellingRedirectTransport {
            fetches: AtomicUsize::new(0),
        };
        let cancellation = ToolCancellation::new();
        let gateway = NetworkGateway::new(Policies(policy()), transport, Resolver(public()));

        assert_eq!(
            gateway
                .fetch(
                    &lease(LeaseStatus::Active, 10),
                    &p,
                    &p.scope,
                    FetchIntent {
                        url: "https://docs.example/start".into(),
                        method: "GET".into(),
                    },
                    1,
                    &cancellation,
                )
                .await,
            Err(NetworkDenied::Cancelled)
        );
        assert_eq!(
            gateway.transport.fetches.load(Ordering::SeqCst),
            1,
            "cancellation after a response must suppress redirect traversal"
        );
    }
    struct CapturingSearchTransport {
        response: TransportSearchResponse,
        requests: Mutex<Vec<TransportSearchRequest>>,
    }
    #[async_trait]
    impl NetworkTransport for CapturingSearchTransport {
        async fn search(
            &self,
            request: TransportSearchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportSearchResponse, String> {
            self.requests.lock().unwrap().push(request);
            Ok(self.response.clone())
        }

        async fn fetch(
            &self,
            _: TransportFetchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportFetchResponse, String> {
            Err("fetch is not used in this test".into())
        }
    }
    #[tokio::test]
    async fn search_carries_policy_bounds_and_rechecks_transport_metadata() {
        let p = package();
        let cancellation = ToolCancellation::new();
        let transport = CapturingSearchTransport {
            response: TransportSearchResponse {
                content_type: "text/html; charset=utf-8".into(),
                byte_count: 9,
                results: vec![],
            },
            requests: Mutex::new(vec![]),
        };
        let gateway = NetworkGateway::new(Policies(policy()), transport, Resolver(public()));

        let evidence = gateway
            .search(
                &lease(LeaseStatus::Active, 10),
                &p,
                &p.scope,
                SearchIntent {
                    provider_host: "search.example".into(),
                    query: "policy bound".into(),
                },
                1,
                &cancellation,
            )
            .await
            .unwrap();

        assert_eq!(evidence.audit.byte_count, 9);
        let requests = gateway.transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].response_bounds.max_response_bytes, 10);
        assert_eq!(
            requests[0].response_bounds.allowed_mime_types,
            BTreeSet::from(["text/html".to_owned()])
        );

        let rejected = NetworkGateway::new(
            Policies(policy()),
            CapturingSearchTransport {
                response: TransportSearchResponse {
                    content_type: "application/json".into(),
                    byte_count: 11,
                    results: vec![],
                },
                requests: Mutex::new(vec![]),
            },
            Resolver(public()),
        );
        assert_eq!(
            rejected
                .search(
                    &lease(LeaseStatus::Active, 10),
                    &p,
                    &p.scope,
                    SearchIntent {
                        provider_host: "search.example".into(),
                        query: "too large".into(),
                    },
                    1,
                    &cancellation,
                )
                .await,
            Err(NetworkDenied::Response("response too large".into()))
        );
    }
    struct CapturingFetchTransport {
        requests: Mutex<Vec<TransportFetchRequest>>,
    }
    #[async_trait]
    impl NetworkTransport for CapturingFetchTransport {
        async fn search(
            &self,
            _: TransportSearchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportSearchResponse, String> {
            Err("search is not used in this test".into())
        }

        async fn fetch(
            &self,
            request: TransportFetchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportFetchResponse, String> {
            self.requests.lock().unwrap().push(request);
            Ok(response(200, &[("content-type", "text/html")], b"ok"))
        }
    }
    #[tokio::test]
    async fn fetch_carries_policy_bounds_to_the_transport_before_receiving_a_body() {
        let p = package();
        let cancellation = ToolCancellation::new();
        let gateway = NetworkGateway::new(
            Policies(policy()),
            CapturingFetchTransport {
                requests: Mutex::new(vec![]),
            },
            Resolver(public()),
        );

        gateway
            .fetch(
                &lease(LeaseStatus::Active, 10),
                &p,
                &p.scope,
                FetchIntent {
                    url: "https://docs.example/a".into(),
                    method: "GET".into(),
                },
                1,
                &cancellation,
            )
            .await
            .unwrap();

        let requests = gateway.transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].response_bounds.max_response_bytes, 10);
        assert_eq!(
            requests[0].response_bounds.allowed_mime_types,
            BTreeSet::from(["text/html".to_owned()])
        );
    }
    #[tokio::test]
    async fn lease_scope_policy_and_action_fail_closed() {
        let p = package();
        let g = gateway(vec![], public());
        let cancellation = ToolCancellation::new();
        assert!(matches!(
            g.search(
                &lease(LeaseStatus::Revoked, 10),
                &p,
                &p.scope,
                SearchIntent {
                    provider_host: "search.example".into(),
                    query: "q".into()
                },
                1,
                &cancellation,
            )
            .await,
            Err(NetworkDenied::Gateway(_))
        ));
        assert!(matches!(
            g.search(
                &lease(LeaseStatus::Active, 1),
                &p,
                &p.scope,
                SearchIntent {
                    provider_host: "search.example".into(),
                    query: "q".into()
                },
                1,
                &cancellation,
            )
            .await,
            Err(NetworkDenied::Gateway(_))
        ));
        let mut wrong = p.clone();
        wrong.status = WorkPackageStatus::Frozen;
        assert!(matches!(
            g.search(
                &lease(LeaseStatus::Active, 10),
                &wrong,
                &wrong.scope,
                SearchIntent {
                    provider_host: "search.example".into(),
                    query: "q".into()
                },
                1,
                &cancellation,
            )
            .await,
            Err(NetworkDenied::WorkPackageDenied)
        ));
    }
    #[tokio::test]
    async fn redirect_dns_and_host_bypasses_are_denied() {
        let p = package();
        let cancellation = ToolCancellation::new();
        let g = gateway(
            vec![response(302, &[("location", "https://localhost/")], b"")],
            public(),
        );
        assert!(g
            .fetch(
                &lease(LeaseStatus::Active, 10),
                &p,
                &p.scope,
                FetchIntent {
                    url: "https://docs.example/a".into(),
                    method: "GET".into()
                },
                1,
                &cancellation,
            )
            .await
            .is_err());
        let g = gateway(vec![], vec!["127.0.0.1".parse().unwrap()]);
        assert!(matches!(
            g.search(
                &lease(LeaseStatus::Active, 10),
                &p,
                &p.scope,
                SearchIntent {
                    provider_host: "search.example".into(),
                    query: "q".into()
                },
                1,
                &cancellation,
            )
            .await,
            Err(NetworkDenied::Dns(_))
        ));
        let g = gateway(vec![], public());
        assert!(matches!(
            g.fetch(
                &lease(LeaseStatus::Active, 10),
                &p,
                &p.scope,
                FetchIntent {
                    url: "http://docs.example/a".into(),
                    method: "POST".into()
                },
                1,
                &cancellation,
            )
            .await,
            Err(NetworkDenied::Intent(_))
        ));
    }
    #[tokio::test]
    async fn mime_size_download_and_redirect_limit_are_denied() {
        let p = package();
        let cancellation = ToolCancellation::new();
        for r in [
            response(200, &[("content-type", "application/pdf")], b"x"),
            response(
                200,
                &[
                    ("content-type", "text/html"),
                    ("content-disposition", "attachment"),
                ],
                b"x",
            ),
            response(200, &[("content-type", "text/html")], b"01234567890"),
            response(302, &[("location", "/b")], b""),
            response(302, &[("location", "/c")], b""),
        ] {
            let g = gateway(vec![r], public());
            assert!(g
                .fetch(
                    &lease(LeaseStatus::Active, 10),
                    &p,
                    &p.scope,
                    FetchIntent {
                        url: "https://docs.example/a".into(),
                        method: "GET".into()
                    },
                    1,
                    &cancellation,
                )
                .await
                .is_err());
        }
        let g = gateway(
            vec![
                response(302, &[("location", "/b")], b""),
                response(302, &[("location", "/c")], b""),
            ],
            public(),
        );
        assert!(matches!(
            g.fetch(
                &lease(LeaseStatus::Active, 10),
                &p,
                &p.scope,
                FetchIntent {
                    url: "https://docs.example/a".into(),
                    method: "GET".into()
                },
                1,
                &cancellation,
            )
            .await,
            Err(NetworkDenied::Response(_))
        ));
    }
    #[test]
    fn registry_is_durable_and_scope_bound() {
        let c = Connection::open_in_memory().unwrap();
        crate::db::migrate(&c).unwrap();
        let p = SourcePolicyRepository::create(
            &c,
            NewSourcePolicy {
                owner_profile_id: 1,
                workspace_key: "w".into(),
                capability_scope_ref: "ref".into(),
                actions: BTreeSet::from([NetworkAction::Search]),
                allowed_hosts: BTreeSet::from(["search.example".into()]),
                allowed_mime_types: BTreeSet::from(["text/html".into()]),
                max_response_bytes: 10,
                max_redirects: 1,
            },
        )
        .unwrap();
        assert_eq!(
            SourcePolicyRepository::load(&c, 1, "w", "ref").unwrap().id,
            p.id
        );
        assert!(matches!(
            SourcePolicyRepository::load(&c, 2, "w", "ref"),
            Err(SourcePolicyError::NotFound)
        ));
    }

    #[test]
    fn shared_db_policy_lookup_reuses_the_application_connection() {
        let connection = Connection::open_in_memory().unwrap();
        crate::db::migrate(&connection).unwrap();
        let expected = SourcePolicyRepository::create(
            &connection,
            NewSourcePolicy {
                owner_profile_id: 7,
                workspace_key: "shared".into(),
                capability_scope_ref: "docs".into(),
                actions: BTreeSet::from([NetworkAction::Fetch]),
                allowed_hosts: BTreeSet::from(["docs.example".into()]),
                allowed_mime_types: BTreeSet::from(["text/plain".into()]),
                max_response_bytes: 128,
                max_redirects: 1,
            },
        )
        .unwrap();
        let db = SharedDb::new(connection);
        let lookup = SharedDbSourcePolicyLookup::new(db.clone());
        let scope = WorkPackageScope {
            session_id: "session".into(),
            owner_profile_id: 7,
            workspace_key: "shared".into(),
        };

        let loaded = lookup.load(&scope, "docs").unwrap();
        assert_eq!(loaded, expected);
        assert!(lookup.db().same_instance(&db));
        // The adapter must not retain a connection guard after the call.
        assert!(db.arc().try_lock().is_ok());
    }

    #[test]
    fn shared_db_policy_lookup_denies_missing_or_wrong_scope() {
        let connection = Connection::open_in_memory().unwrap();
        crate::db::migrate(&connection).unwrap();
        let db = SharedDb::new(connection);
        let lookup = SharedDbSourcePolicyLookup::new(db);
        let scope = WorkPackageScope {
            session_id: "session".into(),
            owner_profile_id: 7,
            workspace_key: "missing".into(),
        };
        assert!(matches!(
            lookup.load(&scope, "docs"),
            Err(SourcePolicyError::NotFound)
        ));
    }
}
