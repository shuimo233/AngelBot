//! Narrow contract between the delegation scheduler and a real worker host.
//!
//! This module intentionally contains no filesystem paths, database handles,
//! tool registries, or network clients.  The main agent injects an opaque
//! binding; a concrete host may later translate that binding into a sandbox,
//! gateway, and model execution kernel.  Every request and emitted event is
//! checked against the binding before it can cross that seam.

use async_trait::async_trait;
use std::fmt;

use super::{
    delegation_contract::{DelegationDelivery, DeliveryStatus},
    tool::ToolCancellation,
    worker_policy::{TaskShape, WorkerProfile},
};

/// Stable identity projected into callbacks.  The binding itself remains
/// opaque and cannot be reconstructed from this projection alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerHostIdentity {
    attempt_id: String,
    delegation_id: String,
    work_package_id: String,
}

impl WorkerHostIdentity {
    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }

    pub fn delegation_id(&self) -> &str {
        &self.delegation_id
    }

    pub fn work_package_id(&self) -> &str {
        &self.work_package_id
    }
}

/// Opaque authority selected by the main agent.  In particular, this does not
/// contain a path or a bearer token.  `capability_ref` is an audit/persistence
/// reference resolved by a future host adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerHostBinding {
    identity: WorkerHostIdentity,
    lease_epoch: u64,
    capability_ref: String,
    worker_profile: WorkerProfile,
    task_shape: TaskShape,
}

impl WorkerHostBinding {
    pub fn new(
        attempt_id: impl Into<String>,
        delegation_id: impl Into<String>,
        work_package_id: impl Into<String>,
        lease_epoch: u64,
        capability_ref: impl Into<String>,
        worker_profile: WorkerProfile,
        task_shape: TaskShape,
    ) -> Result<Self, WorkerHostError> {
        let identity = WorkerHostIdentity {
            attempt_id: attempt_id.into(),
            delegation_id: delegation_id.into(),
            work_package_id: work_package_id.into(),
        };
        if identity.attempt_id.trim().is_empty()
            || identity.delegation_id.trim().is_empty()
            || identity.work_package_id.trim().is_empty()
        {
            return Err(WorkerHostError::InvalidBinding);
        }
        if lease_epoch == 0 {
            return Err(WorkerHostError::InvalidBinding);
        }
        let capability_ref = capability_ref.into();
        if capability_ref.trim().is_empty() {
            return Err(WorkerHostError::InvalidBinding);
        }
        Ok(Self {
            identity,
            lease_epoch,
            capability_ref,
            worker_profile,
            task_shape,
        })
    }

    pub fn identity(&self) -> WorkerHostIdentity {
        self.identity.clone()
    }

    pub fn lease_epoch(&self) -> u64 {
        self.lease_epoch
    }

    pub fn capability_ref(&self) -> &str {
        &self.capability_ref
    }

    pub fn worker_profile(&self) -> WorkerProfile {
        self.worker_profile
    }

    pub fn task_shape(&self) -> TaskShape {
        self.task_shape
    }
}

/// Operations a host may be asked to perform.  References are opaque values;
/// resolving them to a sandbox path or a network client is a host concern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerHostOperation {
    Read {
        reference: String,
    },
    /// Explicit network tool names prevent a host from guessing whether an
    /// arbitrary string is a search query, URL, or tool identifier.
    NetworkSearch {
        provider_host: String,
        query: String,
    },
    NetworkFetch {
        url: String,
        method: String,
    },
    /// Kept as a compatibility sentinel.  Concrete adapters must reject it;
    /// it is intentionally not a capability-bearing operation.
    Network {
        host: String,
    },
    WriteCandidate {
        reference: String,
        contents: Vec<u8>,
    },
}

/// Main-agent request carrying the identity and epoch that were captured when
/// the operation was scheduled.  A stale worker callback cannot be accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerHostRequest {
    identity: WorkerHostIdentity,
    lease_epoch: u64,
    operation: WorkerHostOperation,
}

impl WorkerHostRequest {
    pub fn new(
        binding: &WorkerHostBinding,
        operation: WorkerHostOperation,
    ) -> Result<Self, WorkerHostError> {
        validate_operation_shape(&operation)?;
        Ok(Self {
            identity: binding.identity(),
            lease_epoch: binding.lease_epoch,
            operation,
        })
    }

    /// Test/adapter seam for callbacks that arrived with an explicit stamp.
    /// The host still validates the stamp and never trusts this constructor.
    pub fn with_stamp(
        identity: WorkerHostIdentity,
        lease_epoch: u64,
        operation: WorkerHostOperation,
    ) -> Result<Self, WorkerHostError> {
        validate_operation_shape(&operation)?;
        Ok(Self {
            identity,
            lease_epoch,
            operation,
        })
    }

    pub fn identity(&self) -> &WorkerHostIdentity {
        &self.identity
    }

    pub fn lease_epoch(&self) -> u64 {
        self.lease_epoch
    }

    pub fn operation(&self) -> &WorkerHostOperation {
        &self.operation
    }
}

/// Bounded result of one capability-scoped host operation. Raw filesystem
/// paths, handles, and network bodies never cross this seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerHostResult {
    Acknowledged,
    Text(String),
    Evidence(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerHostTerminal {
    pub status: DeliveryStatus,
    pub reason: String,
}

impl WorkerHostTerminal {
    pub fn new(status: DeliveryStatus, reason: impl Into<String>) -> Result<Self, WorkerHostError> {
        if matches!(status, DeliveryStatus::InProgress) {
            return Err(WorkerHostError::InvalidTerminal);
        }
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(WorkerHostError::InvalidTerminal);
        }
        Ok(Self { status, reason })
    }
}

/// Events are the only values a worker may return to the scheduler.  All
/// variants carry the identity and lease epoch so a caller can reject stale
/// callbacks before touching durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerHostEvent {
    Heartbeat {
        identity: WorkerHostIdentity,
        lease_epoch: u64,
    },
    Delivery {
        identity: WorkerHostIdentity,
        lease_epoch: u64,
        delivery: DelegationDelivery,
    },
    Terminal {
        identity: WorkerHostIdentity,
        lease_epoch: u64,
        terminal: WorkerHostTerminal,
    },
}

impl WorkerHostEvent {
    pub fn heartbeat(identity: WorkerHostIdentity, lease_epoch: u64) -> Self {
        Self::Heartbeat {
            identity,
            lease_epoch,
        }
    }

    pub fn identity(&self) -> &WorkerHostIdentity {
        match self {
            Self::Heartbeat { identity, .. }
            | Self::Delivery { identity, .. }
            | Self::Terminal { identity, .. } => identity,
        }
    }

    pub fn lease_epoch(&self) -> u64 {
        match self {
            Self::Heartbeat { lease_epoch, .. }
            | Self::Delivery { lease_epoch, .. }
            | Self::Terminal { lease_epoch, .. } => *lease_epoch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerHostError {
    InvalidBinding,
    InvalidOperation,
    InvalidTerminal,
    IdentityMismatch,
    LeaseEpochMismatch,
    CapabilityDenied,
    BindingUnavailable,
    WorktreeUnavailable,
    UnknownOperation,
    Cancelled,
    NetworkDenied(String),
    StaleEvent,
    DeliveryIdentityMismatch,
    DeliveryContract(String),
    TaskStopped,
}

impl fmt::Display for WorkerHostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for WorkerHostError {}

/// Factory boundary used by the scheduler.  A real implementation can inject
/// an execution kernel, gateway, and sandbox behind this trait without making
/// those dependencies part of the scheduler's state machine.
pub trait WorkerHostFactory: Send + Sync {
    type Task: WorkerTask;

    fn start(&self, binding: WorkerHostBinding) -> Result<Self::Task, WorkerHostError>;
}

#[async_trait]
pub trait WorkerTask: Send {
    async fn dispatch(&mut self, request: WorkerHostRequest) -> Result<(), WorkerHostError>;

    /// Dispatch one operation with the parent-owned, one-way cancellation
    /// signal for this attempt.  Hosts that do not perform cancellable I/O can
    /// keep the acknowledgement-only default; network hosts override this
    /// seam so cancelling an attempt reaches the operation in flight.
    async fn dispatch_cancellable(
        &mut self,
        request: WorkerHostRequest,
        cancellation: ToolCancellation,
    ) -> Result<(), WorkerHostError> {
        if cancellation.is_cancelled() {
            return Err(WorkerHostError::Cancelled);
        }
        self.dispatch(request).await
    }

    /// Result-bearing variant used by model hosts. Existing hosts can retain
    /// their acknowledgement-only implementation through this default.
    async fn dispatch_with_result(
        &mut self,
        request: WorkerHostRequest,
    ) -> Result<WorkerHostResult, WorkerHostError> {
        self.dispatch(request)
            .await
            .map(|_| WorkerHostResult::Acknowledged)
    }

    async fn heartbeat(&mut self) -> Result<WorkerHostEvent, WorkerHostError>;
    async fn deliver(
        &mut self,
        delivery: DelegationDelivery,
    ) -> Result<WorkerHostEvent, WorkerHostError>;
    async fn stop(
        &mut self,
        terminal: WorkerHostTerminal,
    ) -> Result<WorkerHostEvent, WorkerHostError>;
}

/// In-memory contract implementation for focused tests and adapter bring-up.
/// It performs policy/identity checks but intentionally does not execute any
/// file, network, or registry operation.
#[derive(Debug)]
pub struct FakeWorkerHost {
    binding: WorkerHostBinding,
    stopped: bool,
    events: Vec<WorkerHostEvent>,
}

impl FakeWorkerHost {
    pub fn events(&self) -> &[WorkerHostEvent] {
        &self.events
    }

    pub fn accept_event(&mut self, event: WorkerHostEvent) -> Result<(), WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        self.validate_stamp(event.identity(), event.lease_epoch())?;
        self.events.push(event);
        Ok(())
    }

    fn validate_stamp(
        &self,
        identity: &WorkerHostIdentity,
        lease_epoch: u64,
    ) -> Result<(), WorkerHostError> {
        if identity != &self.binding.identity {
            return Err(WorkerHostError::IdentityMismatch);
        }
        if lease_epoch != self.binding.lease_epoch {
            return Err(WorkerHostError::StaleEvent);
        }
        Ok(())
    }

    fn authorize(&self, operation: &WorkerHostOperation) -> Result<(), WorkerHostError> {
        validate_operation_shape(operation)?;
        match (
            &self.binding.worker_profile,
            &self.binding.task_shape,
            operation,
        ) {
            (WorkerProfile::Explorer, _, WorkerHostOperation::Read { .. })
            | (WorkerProfile::Implementer, TaskShape::Change, WorkerHostOperation::Read { .. })
            | (WorkerProfile::Verifier, _, WorkerHostOperation::Read { .. }) => Ok(()),
            (
                WorkerProfile::Explorer,
                TaskShape::Explore,
                WorkerHostOperation::NetworkSearch { .. }
                | WorkerHostOperation::NetworkFetch { .. },
            ) => Ok(()),
            (
                WorkerProfile::Implementer,
                TaskShape::Change,
                WorkerHostOperation::WriteCandidate { .. },
            ) => Ok(()),
            _ => Err(WorkerHostError::CapabilityDenied),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FakeWorkerHostFactory;

impl WorkerHostFactory for FakeWorkerHostFactory {
    type Task = FakeWorkerHost;

    fn start(&self, binding: WorkerHostBinding) -> Result<Self::Task, WorkerHostError> {
        Ok(FakeWorkerHost {
            binding,
            stopped: false,
            events: Vec::new(),
        })
    }
}

#[async_trait]
impl WorkerTask for FakeWorkerHost {
    async fn dispatch(&mut self, request: WorkerHostRequest) -> Result<(), WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        self.validate_stamp(&request.identity, request.lease_epoch)?;
        self.authorize(&request.operation)
    }

    async fn heartbeat(&mut self) -> Result<WorkerHostEvent, WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        let event = WorkerHostEvent::heartbeat(self.binding.identity(), self.binding.lease_epoch);
        self.events.push(event.clone());
        Ok(event)
    }

    async fn deliver(
        &mut self,
        delivery: DelegationDelivery,
    ) -> Result<WorkerHostEvent, WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        if delivery.identity.attempt_id != self.binding.identity.attempt_id
            || delivery.identity.delegation_id != self.binding.identity.delegation_id
        {
            return Err(WorkerHostError::DeliveryIdentityMismatch);
        }
        let brief = super::delegation_contract::DelegationBrief {
            schema_version: delivery.schema_version,
            identity: delivery.identity.clone(),
            goal: "worker host delivery".into(),
            background: vec![],
            constraints: vec![],
            allowed_references: vec![],
            allowed_capabilities: vec![],
            completion_criteria: vec!["structured delivery".into()],
        };
        super::delegation_contract::validate_delivery(
            &delivery,
            &brief,
            None,
            &super::delegation_contract::ContractLimits::default(),
        )
        .map_err(|error| WorkerHostError::DeliveryContract(format!("{error:?}")))?;
        let event = WorkerHostEvent::Delivery {
            identity: self.binding.identity(),
            lease_epoch: self.binding.lease_epoch,
            delivery,
        };
        self.events.push(event.clone());
        Ok(event)
    }

    async fn stop(
        &mut self,
        terminal: WorkerHostTerminal,
    ) -> Result<WorkerHostEvent, WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        let event = WorkerHostEvent::Terminal {
            identity: self.binding.identity(),
            lease_epoch: self.binding.lease_epoch,
            terminal,
        };
        self.stopped = true;
        self.events.push(event.clone());
        Ok(event)
    }
}

fn validate_operation_shape(operation: &WorkerHostOperation) -> Result<(), WorkerHostError> {
    const MAX_PROVIDER_HOST_CHARS: usize = 253;
    const MAX_QUERY_CHARS: usize = 2_048;
    const MAX_URL_CHARS: usize = 4_096;
    const MAX_METHOD_CHARS: usize = 16;
    let non_empty = |value: &str| !value.trim().is_empty() && value != "." && value != "..";
    match operation {
        WorkerHostOperation::Read { reference }
        | WorkerHostOperation::WriteCandidate { reference, .. } => {
            if !non_empty(reference) {
                return Err(WorkerHostError::InvalidOperation);
            }
            if let WorkerHostOperation::WriteCandidate { contents, .. } = operation {
                if contents.len() > 1024 * 1024 {
                    return Err(WorkerHostError::InvalidOperation);
                }
            }
        }
        WorkerHostOperation::NetworkSearch {
            provider_host,
            query,
        } => {
            if !non_empty(provider_host)
                || provider_host.contains('/')
                || provider_host.contains("://")
                || !non_empty(query)
                || provider_host.chars().count() > MAX_PROVIDER_HOST_CHARS
                || query.chars().count() > MAX_QUERY_CHARS
            {
                return Err(WorkerHostError::InvalidOperation);
            }
        }
        WorkerHostOperation::NetworkFetch { url, method } => {
            let method = method.trim().to_ascii_uppercase();
            if !non_empty(url)
                || !url.starts_with("https://")
                || url.chars().count() > MAX_URL_CHARS
                || method.chars().count() > MAX_METHOD_CHARS
                || !(method == "GET" || method == "HEAD")
            {
                return Err(WorkerHostError::InvalidOperation);
            }
        }
        WorkerHostOperation::Network { .. } => return Err(WorkerHostError::UnknownOperation),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::delegation_contract::{
        Confidence, DelegationIdentity, KeyFact, Milestone, UsageFlags, VerificationRecord,
        VerificationStatus,
    };

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
        .expect("valid binding")
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
            status: DeliveryStatus::Completed,
            executive_summary: "structured delivery".into(),
            key_facts: vec![KeyFact {
                statement: "fact".into(),
                confidence: Confidence::High,
                evidence_refs: vec!["record://fact".into()],
            }],
            milestones: vec![Milestone {
                label: "done".into(),
                outcome: "completed".into(),
                evidence_refs: vec!["record://milestone".into()],
            }],
            verifications: vec![VerificationRecord {
                item: "focused test".into(),
                method: "cargo test".into(),
                status: VerificationStatus::Passed,
                conclusion: "green".into(),
                evidence_ref: "record://test".into(),
            }],
            candidate_artifacts: vec![],
            open_questions: vec![],
            risks: vec![],
            evidence_refs: vec!["record://delivery".into()],
            usage: UsageFlags {
                context_truncated: false,
                output_truncated: false,
                budget_exhausted: false,
            },
        }
    }

    fn request(binding: &WorkerHostBinding, operation: WorkerHostOperation) -> WorkerHostRequest {
        WorkerHostRequest::new(binding, operation).expect("valid request")
    }

    #[tokio::test]
    async fn explorer_is_read_only_and_can_emit_heartbeat() {
        let factory = FakeWorkerHostFactory;
        let b = binding(WorkerProfile::Explorer, TaskShape::Explore);
        let mut task = factory.start(b.clone()).unwrap();
        task.dispatch(request(
            &b,
            WorkerHostOperation::Read {
                reference: "record://note".into(),
            },
        ))
        .await
        .unwrap();
        task.heartbeat().await.unwrap();
        assert_eq!(task.events().len(), 1);
        assert!(matches!(
            task.dispatch(request(
                &b,
                WorkerHostOperation::WriteCandidate {
                    reference: "src/lib.rs".into(),
                    contents: b"candidate".to_vec(),
                },
            ))
            .await,
            Err(WorkerHostError::CapabilityDenied)
        ));
    }

    #[tokio::test]
    async fn implementer_can_only_write_candidate_in_change_shape() {
        let factory = FakeWorkerHostFactory;
        let b = binding(WorkerProfile::Implementer, TaskShape::Change);
        let mut task = factory.start(b.clone()).unwrap();
        task.dispatch(request(
            &b,
            WorkerHostOperation::WriteCandidate {
                reference: "src/lib.rs".into(),
                contents: b"candidate".to_vec(),
            },
        ))
        .await
        .unwrap();
        assert!(matches!(
            task.dispatch(request(
                &b,
                WorkerHostOperation::NetworkSearch {
                    provider_host: "example.com".into(),
                    query: "safe query".into(),
                },
            ))
            .await,
            Err(WorkerHostError::CapabilityDenied)
        ));
    }

    #[tokio::test]
    async fn verifier_is_read_only() {
        let factory = FakeWorkerHostFactory;
        let b = binding(WorkerProfile::Verifier, TaskShape::Change);
        let mut task = factory.start(b.clone()).unwrap();
        task.dispatch(request(
            &b,
            WorkerHostOperation::Read {
                reference: "record://diff".into(),
            },
        ))
        .await
        .unwrap();
        assert!(matches!(
            task.dispatch(request(
                &b,
                WorkerHostOperation::WriteCandidate {
                    reference: "src/lib.rs".into(),
                    contents: b"candidate".to_vec(),
                },
            ))
            .await,
            Err(WorkerHostError::CapabilityDenied)
        ));
    }

    #[tokio::test]
    async fn stale_identity_or_epoch_events_are_rejected() {
        let factory = FakeWorkerHostFactory;
        let b = binding(WorkerProfile::Explorer, TaskShape::Explore);
        let mut task = factory.start(b.clone()).unwrap();
        let stale_epoch = WorkerHostEvent::heartbeat(b.identity(), b.lease_epoch() + 1);
        assert_eq!(
            task.accept_event(stale_epoch),
            Err(WorkerHostError::StaleEvent)
        );
        let forged = WorkerHostIdentity {
            attempt_id: "other-attempt".into(),
            delegation_id: "delegation-1".into(),
            work_package_id: "package-1".into(),
        };
        assert_eq!(
            task.accept_event(WorkerHostEvent::heartbeat(forged, b.lease_epoch())),
            Err(WorkerHostError::IdentityMismatch)
        );
    }

    #[tokio::test]
    async fn structured_delivery_and_terminal_are_bounded_and_ordered() {
        let factory = FakeWorkerHostFactory;
        let b = binding(WorkerProfile::Implementer, TaskShape::Change);
        let mut task = factory.start(b.clone()).unwrap();
        task.heartbeat().await.unwrap();
        let delivery_event = task.deliver(delivery()).await.unwrap();
        assert!(matches!(delivery_event, WorkerHostEvent::Delivery { .. }));
        let terminal = WorkerHostTerminal::new(DeliveryStatus::Completed, "complete").unwrap();
        let terminal_event = task.stop(terminal).await.unwrap();
        assert!(matches!(terminal_event, WorkerHostEvent::Terminal { .. }));
        assert_eq!(task.events().len(), 3);
        assert!(matches!(
            task.heartbeat().await,
            Err(WorkerHostError::TaskStopped)
        ));
    }

    #[test]
    fn network_operation_names_are_explicit_and_generic_network_is_denied() {
        let b = binding(WorkerProfile::Explorer, TaskShape::Explore);
        assert!(WorkerHostRequest::new(
            &b,
            WorkerHostOperation::NetworkSearch {
                provider_host: "example.com".into(),
                query: "rust".into(),
            },
        )
        .is_ok());
        assert!(WorkerHostRequest::new(
            &b,
            WorkerHostOperation::NetworkFetch {
                url: "https://example.com/docs".into(),
                method: "GET".into(),
            },
        )
        .is_ok());
        assert_eq!(
            WorkerHostRequest::new(
                &b,
                WorkerHostOperation::Network {
                    host: "example.com".into(),
                },
            ),
            Err(WorkerHostError::UnknownOperation)
        );
    }

    #[test]
    fn network_request_bounds_reject_oversized_inputs() {
        let b = binding(WorkerProfile::Explorer, TaskShape::Explore);
        assert!(matches!(
            WorkerHostRequest::new(
                &b,
                WorkerHostOperation::NetworkSearch {
                    provider_host: "a".repeat(254),
                    query: "query".into(),
                },
            ),
            Err(WorkerHostError::InvalidOperation)
        ));
        assert!(matches!(
            WorkerHostRequest::new(
                &b,
                WorkerHostOperation::NetworkSearch {
                    provider_host: "example.com".into(),
                    query: "q".repeat(2_049),
                },
            ),
            Err(WorkerHostError::InvalidOperation)
        ));
        assert!(matches!(
            WorkerHostRequest::new(
                &b,
                WorkerHostOperation::NetworkFetch {
                    url: format!("https://example.com/{}", "x".repeat(4_100)),
                    method: "GET".into(),
                },
            ),
            Err(WorkerHostError::InvalidOperation)
        ));
    }

    #[test]
    fn network_request_shape_rejects_empty_or_unsafe_values() {
        let b = binding(WorkerProfile::Explorer, TaskShape::Explore);
        for operation in [
            WorkerHostOperation::NetworkSearch {
                provider_host: "".into(),
                query: "q".into(),
            },
            WorkerHostOperation::NetworkSearch {
                provider_host: "example.com".into(),
                query: "..".into(),
            },
            WorkerHostOperation::NetworkFetch {
                url: "http://example.com".into(),
                method: "GET".into(),
            },
            WorkerHostOperation::NetworkFetch {
                url: "https://example.com".into(),
                method: "POST".into(),
            },
        ] {
            assert!(matches!(
                WorkerHostRequest::new(&b, operation),
                Err(WorkerHostError::InvalidOperation)
            ));
        }
    }
}
