//! Explorer-only worker-host adapter contract.
//!
//! The port is deliberately narrower than `NetworkGateway`: a production
//! implementation can delegate to the existing gateway after checking its live
//! lease, work-package scope, and source policy.  This module carries no
//! transport, path, credential, or raw-body data. The production delegated
//! runtime composes this adapter at startup; callers only receive bounded,
//! opaque evidence references.

use async_trait::async_trait;
use std::sync::Arc;

use super::{
    delegated_worker_adapter::DelegatedAttempt,
    delegation_contract::DelegationDelivery,
    network_gateway::{
        FetchIntent, HostResolver, NetworkGateway, NetworkTransport, SearchIntent,
        SourcePolicyLookup,
    },
    tool::ToolCancellation,
    work_package::{WorkPackage, WorkPackageScope},
    worker_host_contract::{
        WorkerHostBinding, WorkerHostError, WorkerHostEvent, WorkerHostFactory,
        WorkerHostOperation, WorkerHostRequest, WorkerHostTerminal, WorkerTask,
    },
    worker_policy::{TaskShape, WorkerProfile},
};

pub const MAX_EVIDENCE_ITEMS: usize = 5;
pub const MAX_EVIDENCE_REF_CHARS: usize = 256;
pub const MAX_SOURCE_HOST_CHARS: usize = 253;
pub const MAX_EVIDENCE_SUMMARY_BYTES: usize = 900;
pub const MAX_DELIVERY_EVIDENCE_REFS: usize = 32;
pub const MAX_DELIVERY_EVIDENCE_FACTS: usize = 10;

/// A bounded, untrusted projection. The source text is data, never an
/// instruction: it is normalized and size-limited before it can enter a
/// delivery. Raw response bodies, credentials and request URLs never cross
/// this seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedExplorerEvidence {
    pub evidence_ref: String,
    pub source_host: String,
    pub summary: String,
}

impl BoundedExplorerEvidence {
    pub fn new(
        evidence_ref: impl Into<String>,
        source_host: impl Into<String>,
    ) -> Result<Self, WorkerHostError> {
        let source_host = source_host.into();
        let summary = format!("Evidence captured from {source_host}");
        Self::with_summary(evidence_ref, source_host, summary)
    }

    pub fn with_summary(
        evidence_ref: impl Into<String>,
        source_host: impl Into<String>,
        summary: impl Into<String>,
    ) -> Result<Self, WorkerHostError> {
        let evidence_ref = evidence_ref.into();
        let source_host = source_host.into();
        let summary = normalize_untrusted_text(&summary.into());
        if evidence_ref.trim().is_empty()
            || evidence_ref.chars().count() > MAX_EVIDENCE_REF_CHARS
            || source_host.trim().is_empty()
            || source_host.chars().count() > MAX_SOURCE_HOST_CHARS
            || source_host.contains('/')
            || source_host.contains("://")
            || summary.is_empty()
            || summary.len() > MAX_EVIDENCE_SUMMARY_BYTES
        {
            return Err(WorkerHostError::NetworkDenied(
                "bounded evidence validation failed".into(),
            ));
        }
        Ok(Self {
            evidence_ref,
            source_host,
            summary,
        })
    }
}

fn normalize_untrusted_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len().min(MAX_EVIDENCE_SUMMARY_BYTES));
    let mut needs_space = false;
    for character in value.chars() {
        if character.is_whitespace() || character.is_control() {
            needs_space = !normalized.is_empty();
            continue;
        }
        if needs_space {
            if normalized.len() + 1 >= MAX_EVIDENCE_SUMMARY_BYTES {
                break;
            }
            normalized.push(' ');
            needs_space = false;
        }
        if normalized.len() + character.len_utf8() > MAX_EVIDENCE_SUMMARY_BYTES {
            break;
        }
        normalized.push(character);
    }
    normalized.trim().to_string()
}

/// Narrow asynchronous gateway seam.  The same one-way cancellation signal
/// follows an explorer operation through policy checks, transport and evidence
/// projection, so a stopped attempt cannot keep reading an HTTP response in a
/// detached blocking task.
#[async_trait]
pub trait ExplorerGatewayPort: Send + Sync {
    async fn search(
        &self,
        binding: &WorkerHostBinding,
        provider_host: &str,
        query: &str,
        cancellation: ToolCancellation,
    ) -> Result<Vec<BoundedExplorerEvidence>, String>;

    async fn fetch(
        &self,
        binding: &WorkerHostBinding,
        url: &str,
        method: &str,
        cancellation: ToolCancellation,
    ) -> Result<BoundedExplorerEvidence, String>;
}

/// The immutable values needed to authorize one network operation.  The
/// lookup is deliberately a separate seam: a `WorkerHostBinding` only carries
/// opaque identity and capability references, so a host cannot manufacture a
/// lease or infer a filesystem/database path from it.
#[derive(Debug, Clone)]
pub struct ExplorerGatewaySnapshot {
    pub attempt: DelegatedAttempt,
    pub package: WorkPackage,
    pub scope: WorkPackageScope,
    /// A durable lookup must set this from the live lease row immediately
    /// before each request.  A signed lease snapshot alone is insufficient.
    pub live_lease: bool,
}

pub trait ExplorerGatewayContext: Send + Sync {
    fn snapshot(
        &self,
        binding: &WorkerHostBinding,
        now_ms: i64,
    ) -> Result<ExplorerGatewaySnapshot, String>;
}

/// Production gateway adapter.  It translates the opaque worker binding into
/// a live attempt/package/scope snapshot, delegates all policy and transport
/// checks to `NetworkGateway`, and projects only opaque evidence references.
/// Query text and response bodies never cross the `ExplorerGatewayPort` seam.
pub struct GatewayExplorerPort<P, T, R, C> {
    gateway: NetworkGateway<P, T, R>,
    context: C,
    now_ms: fn() -> i64,
}

impl<P, T, R, C> GatewayExplorerPort<P, T, R, C> {
    pub fn new(gateway: NetworkGateway<P, T, R>, context: C, now_ms: fn() -> i64) -> Self {
        Self {
            gateway,
            context,
            now_ms,
        }
    }
}

impl<P, T, R, C> GatewayExplorerPort<P, T, R, C>
where
    P: SourcePolicyLookup + Send + Sync,
    T: NetworkTransport + Send + Sync,
    R: HostResolver + Send + Sync,
    C: ExplorerGatewayContext + Send + Sync,
{
    fn authorize_snapshot(
        &self,
        binding: &WorkerHostBinding,
        snapshot: &ExplorerGatewaySnapshot,
    ) -> Result<(), String> {
        let identity = binding.identity();
        if snapshot.attempt.attempt_id != identity.attempt_id()
            || snapshot.attempt.delegation_id != identity.delegation_id()
            || snapshot.attempt.work_package_id != identity.work_package_id()
            || snapshot.attempt.epoch != binding.lease_epoch()
            || snapshot.attempt.lease.attempt_id != snapshot.attempt.attempt_id
            || snapshot.package.id != identity.work_package_id()
            || snapshot.package.scope != snapshot.scope
            || snapshot.package.capability_scope_ref != binding.capability_ref()
            || !snapshot.live_lease
        {
            return Err("live explorer capability mismatch".into());
        }
        Ok(())
    }

    fn evidence_ref(action: &str, digest: &str, index: usize) -> String {
        // The digest is produced by NetworkGateway; this reference is opaque
        // and intentionally contains no query, URL, title, snippet or body.
        format!("evidence://network/{action}/{digest}/{index}")
    }
}

#[async_trait]
impl<P, T, R, C> ExplorerGatewayPort for GatewayExplorerPort<P, T, R, C>
where
    P: SourcePolicyLookup + Send + Sync,
    T: NetworkTransport + Send + Sync,
    R: HostResolver + Send + Sync,
    C: ExplorerGatewayContext + Send + Sync,
{
    async fn search(
        &self,
        binding: &WorkerHostBinding,
        provider_host: &str,
        query: &str,
        cancellation: ToolCancellation,
    ) -> Result<Vec<BoundedExplorerEvidence>, String> {
        if cancellation.is_cancelled() {
            return Err("network operation cancelled".into());
        }
        let now = (self.now_ms)();
        let snapshot = self.context.snapshot(binding, now)?;
        self.authorize_snapshot(binding, &snapshot)?;
        let evidence = self
            .gateway
            .search(
                &snapshot.attempt.lease,
                &snapshot.package,
                &snapshot.scope,
                SearchIntent {
                    provider_host: provider_host.into(),
                    query: query.into(),
                },
                now,
                &cancellation,
            )
            .await
            .map_err(|error| error.to_string())?;
        if cancellation.is_cancelled() {
            return Err("network operation cancelled".into());
        }
        let projection_snapshot = self.context.snapshot(binding, (self.now_ms)())?;
        self.authorize_snapshot(binding, &projection_snapshot)?;
        let host = provider_host
            .trim()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        evidence
            .results
            .into_iter()
            .take(MAX_EVIDENCE_ITEMS)
            .enumerate()
            .map(|(index, result)| {
                BoundedExplorerEvidence::with_summary(
                    Self::evidence_ref("search", &evidence.audit.request_digest, index),
                    host.clone(),
                    format!("{} — {}", result.title, result.snippet),
                )
                .map_err(|error| error.to_string())
            })
            .collect()
    }

    async fn fetch(
        &self,
        binding: &WorkerHostBinding,
        url: &str,
        method: &str,
        cancellation: ToolCancellation,
    ) -> Result<BoundedExplorerEvidence, String> {
        if cancellation.is_cancelled() {
            return Err("network operation cancelled".into());
        }
        let now = (self.now_ms)();
        let snapshot = self.context.snapshot(binding, now)?;
        self.authorize_snapshot(binding, &snapshot)?;
        let evidence = self
            .gateway
            .fetch(
                &snapshot.attempt.lease,
                &snapshot.package,
                &snapshot.scope,
                FetchIntent {
                    url: url.into(),
                    method: method.into(),
                },
                now,
                &cancellation,
            )
            .await
            .map_err(|error| error.to_string())?;
        if cancellation.is_cancelled() {
            return Err("network operation cancelled".into());
        }
        let projection_snapshot = self.context.snapshot(binding, (self.now_ms)())?;
        self.authorize_snapshot(binding, &projection_snapshot)?;
        let host = evidence
            .audit
            .final_host
            .ok_or_else(|| "gateway returned no final host".to_string())?;
        BoundedExplorerEvidence::with_summary(
            Self::evidence_ref("fetch", &evidence.audit.request_digest, 0),
            host,
            String::from_utf8_lossy(&evidence.body),
        )
        .map_err(|error| error.to_string())
    }
}

/// Factory used by the scheduler to construct the gateway-backed Explorer host
/// from the production composition root.
pub struct ExplorerWorkerHostFactory<G> {
    gateway: Arc<G>,
}

impl<G> ExplorerWorkerHostFactory<G>
where
    G: ExplorerGatewayPort + 'static,
{
    pub fn new(gateway: G) -> Self {
        Self {
            gateway: Arc::new(gateway),
        }
    }
}

impl<G> WorkerHostFactory for ExplorerWorkerHostFactory<G>
where
    G: ExplorerGatewayPort + 'static,
{
    type Task = ExplorerWorkerHost<G>;

    fn start(&self, binding: WorkerHostBinding) -> Result<Self::Task, WorkerHostError> {
        if binding.worker_profile() != WorkerProfile::Explorer
            || binding.task_shape() != TaskShape::Explore
        {
            return Err(WorkerHostError::CapabilityDenied);
        }
        Ok(ExplorerWorkerHost {
            binding,
            gateway: self.gateway.clone(),
            stopped: false,
            evidence_refs: Vec::new(),
            evidence_items: Vec::new(),
        })
    }
}

pub struct ExplorerWorkerHost<G> {
    binding: WorkerHostBinding,
    gateway: Arc<G>,
    stopped: bool,
    evidence_refs: Vec<String>,
    evidence_items: Vec<BoundedExplorerEvidence>,
}

impl<G> ExplorerWorkerHost<G>
where
    G: ExplorerGatewayPort + 'static,
{
    pub fn evidence_refs(&self) -> &[String] {
        &self.evidence_refs
    }

    /// Contract-only execution entrypoint used by adapter bring-up.  Network
    /// evidence is retained as bounded refs and the returned event is a
    /// heartbeat acknowledgement; a later delivery call carries the
    /// structured `DelegationDelivery`.
    pub async fn execute(
        &mut self,
        request: WorkerHostRequest,
    ) -> Result<WorkerHostEvent, WorkerHostError> {
        self.dispatch(request).await?;
        self.heartbeat().await
    }

    /// Build a useful structured result from bounded evidence projections.
    /// External text remains explicitly low-confidence and cannot influence
    /// capabilities, control flow or tool dispatch.
    pub fn structured_delivery(
        &self,
        mut delivery: DelegationDelivery,
    ) -> Result<DelegationDelivery, WorkerHostError> {
        if delivery.identity.attempt_id != self.binding.identity().attempt_id()
            || delivery.identity.delegation_id != self.binding.identity().delegation_id()
        {
            return Err(WorkerHostError::DeliveryIdentityMismatch);
        }
        for reference in &self.evidence_refs {
            if !delivery.evidence_refs.contains(reference) {
                delivery.evidence_refs.push(reference.clone());
            }
        }
        if delivery.evidence_refs.len() > MAX_DELIVERY_EVIDENCE_REFS {
            return Err(WorkerHostError::NetworkDenied(
                "delivery evidence bound exceeded".into(),
            ));
        }
        if !self.evidence_items.is_empty() {
            delivery.executive_summary = format!(
                "Explorer collected {} bounded evidence item{} from approved sources",
                self.evidence_items.len(),
                if self.evidence_items.len() == 1 {
                    ""
                } else {
                    "s"
                },
            );
            delivery.key_facts = self
                .evidence_items
                .iter()
                .take(MAX_DELIVERY_EVIDENCE_FACTS)
                .map(|item| super::delegation_contract::KeyFact {
                    statement: format!(
                        "Untrusted source {} reports: {}",
                        item.source_host, item.summary
                    ),
                    confidence: super::delegation_contract::Confidence::Low,
                    evidence_refs: vec![item.evidence_ref.clone()],
                })
                .collect();
            if let Some(verification) = delivery.verifications.first_mut() {
                verification.conclusion = format!(
                    "{} bounded evidence item{} returned through the approved gateway",
                    self.evidence_items.len(),
                    if self.evidence_items.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                );
            }
        }
        Ok(delivery)
    }

    fn validate_request(
        &self,
        request: &WorkerHostRequest,
        cancellation: &ToolCancellation,
    ) -> Result<(), WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        if cancellation.is_cancelled() {
            return Err(WorkerHostError::Cancelled);
        }
        if request.identity() != &self.binding.identity() {
            return Err(WorkerHostError::IdentityMismatch);
        }
        if request.lease_epoch() != self.binding.lease_epoch() {
            return Err(WorkerHostError::StaleEvent);
        }
        Ok(())
    }

    fn append_evidence(
        &mut self,
        evidence: Vec<BoundedExplorerEvidence>,
    ) -> Result<(), WorkerHostError> {
        if evidence.len() > MAX_EVIDENCE_ITEMS {
            return Err(WorkerHostError::NetworkDenied(
                "evidence item bound exceeded".into(),
            ));
        }
        for item in evidence {
            BoundedExplorerEvidence::with_summary(
                item.evidence_ref.clone(),
                item.source_host.clone(),
                item.summary.clone(),
            )?;
            if !self.evidence_refs.contains(&item.evidence_ref) {
                self.evidence_refs.push(item.evidence_ref.clone());
                self.evidence_items.push(item);
            }
        }
        if self.evidence_refs.len() > MAX_DELIVERY_EVIDENCE_REFS {
            return Err(WorkerHostError::NetworkDenied(
                "delivery evidence bound exceeded".into(),
            ));
        }
        Ok(())
    }

    async fn dispatch_with_cancellation(
        &mut self,
        request: WorkerHostRequest,
        cancellation: ToolCancellation,
    ) -> Result<(), WorkerHostError> {
        self.validate_request(&request, &cancellation)?;
        match request.operation() {
            WorkerHostOperation::NetworkSearch {
                provider_host,
                query,
            } => {
                let evidence = self
                    .gateway
                    .search(&self.binding, provider_host, query, cancellation.clone())
                    .await;
                if cancellation.is_cancelled() {
                    return Err(WorkerHostError::Cancelled);
                }
                let evidence = evidence.map_err(WorkerHostError::NetworkDenied)?;
                self.append_evidence(evidence)
            }
            WorkerHostOperation::NetworkFetch { url, method } => {
                let evidence = self
                    .gateway
                    .fetch(&self.binding, url, method, cancellation.clone())
                    .await;
                if cancellation.is_cancelled() {
                    return Err(WorkerHostError::Cancelled);
                }
                let evidence = evidence.map_err(WorkerHostError::NetworkDenied)?;
                self.append_evidence(vec![evidence])
            }
            WorkerHostOperation::Read { .. }
            | WorkerHostOperation::WriteCandidate { .. }
            | WorkerHostOperation::Network { .. } => Err(WorkerHostError::UnknownOperation),
        }
    }
}

#[async_trait]
impl<G> WorkerTask for ExplorerWorkerHost<G>
where
    G: ExplorerGatewayPort + 'static,
{
    async fn dispatch(&mut self, request: WorkerHostRequest) -> Result<(), WorkerHostError> {
        self.dispatch_with_cancellation(request, ToolCancellation::new())
            .await
    }

    async fn dispatch_cancellable(
        &mut self,
        request: WorkerHostRequest,
        cancellation: ToolCancellation,
    ) -> Result<(), WorkerHostError> {
        self.dispatch_with_cancellation(request, cancellation).await
    }

    async fn heartbeat(&mut self) -> Result<WorkerHostEvent, WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        Ok(WorkerHostEvent::heartbeat(
            self.binding.identity(),
            self.binding.lease_epoch(),
        ))
    }

    async fn deliver(
        &mut self,
        delivery: DelegationDelivery,
    ) -> Result<WorkerHostEvent, WorkerHostError> {
        self.validate_request(
            &WorkerHostRequest::new(
                &self.binding,
                WorkerHostOperation::NetworkSearch {
                    provider_host: "evidence".into(),
                    query: "delivery".into(),
                },
            )?,
            &ToolCancellation::new(),
        )?;
        let delivery = self.structured_delivery(delivery)?;
        let event = WorkerHostEvent::Delivery {
            identity: self.binding.identity(),
            lease_epoch: self.binding.lease_epoch(),
            delivery,
        };
        Ok(event)
    }

    async fn stop(
        &mut self,
        terminal: WorkerHostTerminal,
    ) -> Result<WorkerHostEvent, WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        self.stopped = true;
        Ok(WorkerHostEvent::Terminal {
            identity: self.binding.identity(),
            lease_epoch: self.binding.lease_epoch(),
            terminal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::delegation_contract::{
        Confidence, DelegationIdentity, KeyFact, Milestone, UsageFlags, VerificationRecord,
        VerificationStatus,
    };
    use crate::agent::{
        delegation::LeaseStatus,
        execution_gateway::{CapabilityGrant, CapabilityLease},
        network_gateway::{
            NetworkTransport, SourcePolicy, SourcePolicyError, SourcePolicyLookup,
            TransportFetchRequest, TransportFetchResponse, TransportSearchRequest,
            TransportSearchResponse, TransportSearchResult,
        },
    };
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use std::{
        collections::BTreeSet,
        net::{IpAddr, Ipv4Addr},
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct FakeGateway {
        calls: Arc<Mutex<Vec<String>>>,
        evidence: Vec<BoundedExplorerEvidence>,
    }

    #[async_trait]
    impl ExplorerGatewayPort for FakeGateway {
        async fn search(
            &self,
            _binding: &WorkerHostBinding,
            provider_host: &str,
            _query: &str,
            _: ToolCancellation,
        ) -> Result<Vec<BoundedExplorerEvidence>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("search:{provider_host}"));
            Ok(self.evidence.clone())
        }

        async fn fetch(
            &self,
            _binding: &WorkerHostBinding,
            url: &str,
            _method: &str,
            _: ToolCancellation,
        ) -> Result<BoundedExplorerEvidence, String> {
            self.calls.lock().unwrap().push(format!("fetch:{url}"));
            self.evidence
                .first()
                .cloned()
                .ok_or_else(|| "no evidence".into())
        }
    }

    fn binding(profile: WorkerProfile, shape: TaskShape) -> WorkerHostBinding {
        WorkerHostBinding::new(
            "attempt-1",
            "delegation-1",
            "package-1",
            7,
            "cap-1",
            profile,
            shape,
        )
        .unwrap()
    }

    fn delivery() -> DelegationDelivery {
        DelegationDelivery {
            schema_version: 1,
            delivery_id: "delivery-1".into(),
            delivery_revision: 1,
            identity: DelegationIdentity {
                session_id: "session-1".into(),
                message_id: "message-1".into(),
                parent_run_id: "run-1".into(),
                delegation_id: "delegation-1".into(),
                attempt_id: "attempt-1".into(),
            },
            status: super::super::delegation_contract::DeliveryStatus::Completed,
            executive_summary: "bounded explorer result".into(),
            key_facts: vec![KeyFact {
                statement: "evidence ref captured".into(),
                confidence: Confidence::High,
                evidence_refs: vec!["record://evidence".into()],
            }],
            milestones: vec![Milestone {
                label: "network".into(),
                outcome: "bounded".into(),
                evidence_refs: vec!["record://evidence".into()],
            }],
            verifications: vec![VerificationRecord {
                item: "gateway".into(),
                method: "fake".into(),
                status: VerificationStatus::Passed,
                conclusion: "ok".into(),
                evidence_ref: "record://evidence".into(),
            }],
            candidate_artifacts: vec![],
            open_questions: vec![],
            risks: vec![],
            evidence_refs: vec![],
            usage: UsageFlags {
                context_truncated: false,
                output_truncated: false,
                budget_exhausted: false,
            },
        }
    }

    #[tokio::test]
    async fn factory_accepts_only_explorer_explore_bindings() {
        let gateway = FakeGateway::default();
        let factory = ExplorerWorkerHostFactory::new(gateway);
        assert!(factory
            .start(binding(WorkerProfile::Explorer, TaskShape::Explore))
            .is_ok());
        assert!(matches!(
            factory.start(binding(WorkerProfile::Explorer, TaskShape::Change)),
            Err(WorkerHostError::CapabilityDenied)
        ));
        assert!(matches!(
            factory.start(binding(WorkerProfile::Implementer, TaskShape::Change)),
            Err(WorkerHostError::CapabilityDenied)
        ));
    }

    #[tokio::test]
    async fn search_and_fetch_emit_bounded_low_trust_evidence() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let gateway = FakeGateway {
            calls: calls.clone(),
            evidence: vec![BoundedExplorerEvidence::with_summary(
                "evidence://1",
                "example.com",
                "Rust release notes summarize the supported change.",
            )
            .unwrap()],
        };
        let factory = ExplorerWorkerHostFactory::new(gateway);
        let b = binding(WorkerProfile::Explorer, TaskShape::Explore);
        let mut task = factory.start(b.clone()).unwrap();
        let event = task
            .execute(
                WorkerHostRequest::new(
                    &b,
                    WorkerHostOperation::NetworkSearch {
                        provider_host: "example.com".into(),
                        query: "rust".into(),
                    },
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(event.identity(), &b.identity());
        task.dispatch(
            WorkerHostRequest::new(
                &b,
                WorkerHostOperation::NetworkFetch {
                    url: "https://example.com/docs".into(),
                    method: "GET".into(),
                },
            )
            .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(task.evidence_refs(), &["evidence://1"]);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &["search:example.com", "fetch:https://example.com/docs"]
        );
        let output = task.structured_delivery(delivery()).unwrap();
        assert_eq!(output.evidence_refs, vec!["evidence://1"]);
        assert_eq!(output.key_facts.len(), 1);
        assert_eq!(output.key_facts[0].confidence, Confidence::Low);
        assert!(output.key_facts[0].statement.contains("Rust release notes"));
        assert!(output.executive_summary.contains("1 bounded evidence item"));
        assert!(!serde_json::to_string(&output)
            .unwrap()
            .contains("\"query\""));
    }

    #[tokio::test]
    async fn stale_identity_epoch_and_unknown_write_fail_closed() {
        let factory = ExplorerWorkerHostFactory::new(FakeGateway::default());
        let b = binding(WorkerProfile::Explorer, TaskShape::Explore);
        let mut task = factory.start(b.clone()).unwrap();
        let stale = WorkerHostRequest::with_stamp(
            b.identity(),
            b.lease_epoch() + 1,
            WorkerHostOperation::NetworkSearch {
                provider_host: "example.com".into(),
                query: "rust".into(),
            },
        )
        .unwrap();
        assert_eq!(task.dispatch(stale).await, Err(WorkerHostError::StaleEvent));
        let write = WorkerHostRequest::new(
            &b,
            WorkerHostOperation::WriteCandidate {
                reference: "src/lib.rs".into(),
                contents: b"candidate".to_vec(),
            },
        )
        .unwrap();
        assert_eq!(
            task.dispatch(write).await,
            Err(WorkerHostError::UnknownOperation)
        );
    }

    #[tokio::test]
    async fn cancellation_and_oversized_gateway_evidence_are_rejected() {
        let gateway = FakeGateway {
            calls: Arc::new(Mutex::new(Vec::new())),
            evidence: vec![BoundedExplorerEvidence {
                evidence_ref: "x".repeat(MAX_EVIDENCE_REF_CHARS + 1),
                source_host: "example.com".into(),
                summary: "bounded evidence".into(),
            }],
        };
        let factory = ExplorerWorkerHostFactory::new(gateway);
        let b = binding(WorkerProfile::Explorer, TaskShape::Explore);
        let mut task = factory.start(b.clone()).unwrap();
        let request = WorkerHostRequest::new(
            &b,
            WorkerHostOperation::NetworkSearch {
                provider_host: "example.com".into(),
                query: "rust".into(),
            },
        )
        .unwrap();
        assert!(matches!(
            task.dispatch(request).await,
            Err(WorkerHostError::NetworkDenied(_))
        ));
        let cancellation = ToolCancellation::new();
        cancellation.cancel();
        let request = WorkerHostRequest::new(
            &b,
            WorkerHostOperation::NetworkSearch {
                provider_host: "example.com".into(),
                query: "rust".into(),
            },
        )
        .unwrap();
        assert_eq!(
            task.dispatch_cancellable(request, cancellation).await,
            Err(WorkerHostError::Cancelled)
        );
    }

    #[tokio::test]
    async fn terminal_event_keeps_epoch_and_identity_after_delivery() {
        let gateway = FakeGateway {
            calls: Arc::new(Mutex::new(Vec::new())),
            evidence: vec![
                BoundedExplorerEvidence::new("evidence://terminal", "example.com").unwrap(),
            ],
        };
        let factory = ExplorerWorkerHostFactory::new(gateway);
        let b = binding(WorkerProfile::Explorer, TaskShape::Explore);
        let mut task = factory.start(b.clone()).unwrap();
        let event = task.deliver(delivery()).await.unwrap();
        assert_eq!(event.identity(), &b.identity());
        assert_eq!(event.lease_epoch(), b.lease_epoch());
        let terminal = WorkerHostTerminal::new(
            super::super::delegation_contract::DeliveryStatus::Completed,
            "done",
        )
        .unwrap();
        let event = task.stop(terminal).await.unwrap();
        assert_eq!(event.identity(), &b.identity());
        assert_eq!(event.lease_epoch(), b.lease_epoch());
    }

    struct RealPolicies;
    impl SourcePolicyLookup for RealPolicies {
        fn load(
            &self,
            scope: &WorkPackageScope,
            scope_ref: &str,
        ) -> Result<SourcePolicy, SourcePolicyError> {
            if scope.owner_profile_id != 1
                || scope.workspace_key != "workspace"
                || scope_ref != "source"
            {
                return Err(SourcePolicyError::NotFound);
            }
            Ok(SourcePolicy {
                id: "policy-1".into(),
                owner_profile_id: 1,
                workspace_key: "workspace".into(),
                capability_scope_ref: "source".into(),
                policy_version: 1,
                actions: BTreeSet::from([
                    super::super::worker_policy::NetworkAction::Search,
                    super::super::worker_policy::NetworkAction::Fetch,
                ]),
                allowed_hosts: BTreeSet::from(["search.example".into(), "docs.example".into()]),
                allowed_mime_types: BTreeSet::from(["text/plain".into()]),
                max_response_bytes: 1024,
                max_redirects: 1,
                enabled: true,
            })
        }
    }
    struct RealTransport;
    #[async_trait]
    impl NetworkTransport for RealTransport {
        async fn search(
            &self,
            _: TransportSearchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportSearchResponse, String> {
            Ok(TransportSearchResponse {
                content_type: "text/plain".into(),
                byte_count: 128,
                results: vec![TransportSearchResult {
                    title: "untrusted title".into(),
                    url: "https://docs.example/a".into(),
                    snippet: "query must not leak".into(),
                }],
            })
        }
        async fn fetch(
            &self,
            _: TransportFetchRequest,
            _: &ToolCancellation,
        ) -> Result<TransportFetchResponse, String> {
            Ok(TransportFetchResponse {
                status: 200,
                headers: BTreeSet::from([("content-type".into(), "text/plain".into())]),
                body: b"raw body must not cross adapter".to_vec(),
            })
        }
    }
    struct RealResolver;
    impl super::super::network_gateway::HostResolver for RealResolver {
        fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, String> {
            Ok(vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))])
        }
    }
    struct RealContext {
        snapshot: ExplorerGatewaySnapshot,
        live: AtomicBool,
    }
    impl ExplorerGatewayContext for RealContext {
        fn snapshot(
            &self,
            _: &WorkerHostBinding,
            _: i64,
        ) -> Result<ExplorerGatewaySnapshot, String> {
            let mut snapshot = self.snapshot.clone();
            snapshot.live_lease = self.live.load(Ordering::SeqCst);
            Ok(snapshot)
        }
    }
    struct SnapshotThenInactiveContext {
        snapshot: ExplorerGatewaySnapshot,
        calls: Arc<AtomicUsize>,
    }
    impl ExplorerGatewayContext for SnapshotThenInactiveContext {
        fn snapshot(
            &self,
            _: &WorkerHostBinding,
            _: i64,
        ) -> Result<ExplorerGatewaySnapshot, String> {
            let mut snapshot = self.snapshot.clone();
            snapshot.live_lease = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
            Ok(snapshot)
        }
    }
    fn real_snapshot() -> ExplorerGatewaySnapshot {
        let b = WorkerHostBinding::new(
            "attempt-1",
            "delegation-1",
            "package-1",
            7,
            "source",
            WorkerProfile::Explorer,
            TaskShape::Explore,
        )
        .unwrap();
        let lease = CapabilityLease::from_grant(CapabilityGrant {
            id: "lease-1".into(),
            attempt_id: b.identity().attempt_id().into(),
            status: LeaseStatus::Active,
            expires_at_ms: 100,
            read_roots: vec![],
            write_roots: vec![],
            tool_allowlist: vec!["network.search".into(), "network.fetch".into()],
            network_hosts: vec!["search.example".into(), "docs.example".into()],
        })
        .unwrap();
        ExplorerGatewaySnapshot {
            attempt: DelegatedAttempt {
                delegation_id: b.identity().delegation_id().into(),
                attempt_id: b.identity().attempt_id().into(),
                work_package_id: b.identity().work_package_id().into(),
                lease,
                epoch: b.lease_epoch(),
            },
            package: WorkPackage {
                id: b.identity().work_package_id().into(),
                scope: WorkPackageScope {
                    session_id: "session".into(),
                    owner_profile_id: 1,
                    workspace_key: "workspace".into(),
                },
                task_shape: TaskShape::Explore,
                worker_profile: WorkerProfile::Explorer,
                worker_policy_version: 1,
                scope_digest: "digest".into(),
                capability_scope_ref: "source".into(),
                capability_expires_at: 100,
                candidate_version: 0,
                status: super::super::work_package::WorkPackageStatus::Active,
            },
            scope: WorkPackageScope {
                session_id: "session".into(),
                owner_profile_id: 1,
                workspace_key: "workspace".into(),
            },
            live_lease: true,
        }
    }
    fn real_now() -> i64 {
        10
    }

    #[tokio::test]
    async fn real_gateway_adapter_enforces_live_snapshot_and_projects_refs() {
        let context = RealContext {
            snapshot: real_snapshot(),
            live: AtomicBool::new(true),
        };
        let gateway = NetworkGateway::new(RealPolicies, RealTransport, RealResolver);
        let port = GatewayExplorerPort::new(gateway, context, real_now);
        let b = WorkerHostBinding::new(
            "attempt-1",
            "delegation-1",
            "package-1",
            7,
            "source",
            WorkerProfile::Explorer,
            TaskShape::Explore,
        )
        .unwrap();
        let evidence = port
            .search(
                &b,
                "search.example",
                "secret query",
                ToolCancellation::new(),
            )
            .await
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0]
            .evidence_ref
            .starts_with("evidence://network/search/"));
        assert_eq!(evidence[0].source_host, "search.example");
        assert!(!format!("{evidence:?}").contains("secret query"));
        let fetched = port
            .fetch(&b, "https://docs.example/a", "GET", ToolCancellation::new())
            .await
            .unwrap();
        assert!(fetched
            .evidence_ref
            .starts_with("evidence://network/fetch/"));
        assert_eq!(fetched.source_host, "docs.example");
    }

    #[tokio::test]
    async fn real_gateway_adapter_rechecks_live_snapshot_before_projecting_evidence() {
        let calls = Arc::new(AtomicUsize::new(0));
        let context = SnapshotThenInactiveContext {
            snapshot: real_snapshot(),
            calls: calls.clone(),
        };
        let gateway = NetworkGateway::new(RealPolicies, RealTransport, RealResolver);
        let port = GatewayExplorerPort::new(gateway, context, real_now);
        let binding = WorkerHostBinding::new(
            "attempt-1",
            "delegation-1",
            "package-1",
            7,
            "source",
            WorkerProfile::Explorer,
            TaskShape::Explore,
        )
        .unwrap();

        assert!(port
            .search(
                &binding,
                "search.example",
                "must not become evidence",
                ToolCancellation::new(),
            )
            .await
            .is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the live lease must be loaded again after transport and before evidence projection"
        );
    }

    #[tokio::test]
    async fn real_gateway_adapter_rejects_dead_lease_and_binding_drift() {
        let context = RealContext {
            snapshot: real_snapshot(),
            live: AtomicBool::new(false),
        };
        let gateway = NetworkGateway::new(RealPolicies, RealTransport, RealResolver);
        let port = GatewayExplorerPort::new(gateway, context, real_now);
        let b = WorkerHostBinding::new(
            "attempt-1",
            "delegation-1",
            "package-1",
            7,
            "source",
            WorkerProfile::Explorer,
            TaskShape::Explore,
        )
        .unwrap();
        assert!(port
            .search(&b, "search.example", "q", ToolCancellation::new())
            .await
            .is_err());
        let wrong = WorkerHostBinding::new(
            "attempt-2",
            "delegation-1",
            "package-1",
            7,
            "source",
            WorkerProfile::Explorer,
            TaskShape::Explore,
        )
        .unwrap();
        assert!(port
            .fetch(
                &wrong,
                "https://docs.example/a",
                "GET",
                ToolCancellation::new(),
            )
            .await
            .is_err());
    }
}
