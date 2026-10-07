//! Capability-scoped file host for local delegated attempts.
//!
//! The host deliberately exposes only relative file reads and candidate-tree
//! writes through `WorktreeFileProvider`.  It does not expose a canonical
//! path, a `ToolRegistry`, arbitrary shell execution, or network clients.  A
//! future build/test adapter can be added beside this host without weakening
//! the file boundary.

use std::sync::Arc;

use async_trait::async_trait;

use super::{
    delegation_contract::DelegationDelivery,
    worker_host_contract::{
        WorkerHostBinding, WorkerHostError, WorkerHostEvent, WorkerHostFactory,
        WorkerHostOperation, WorkerHostRequest, WorkerHostResult, WorkerHostTerminal, WorkerTask,
    },
    worker_policy::{TaskShape, WorkerProfile},
    workspace_isolation::{
        WorktreeAccess, WorktreeCapability, WorktreeFileProvider, WorktreeManifestIdentity,
        WorktreeOperation,
    },
};

const MAX_READ_BYTES: usize = 64 * 1024;
const MAX_WRITE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ImplementerAccess {
    pub identity: WorktreeManifestIdentity,
    pub capability: WorktreeCapability,
}

pub trait ImplementerAccessResolver: Send + Sync {
    fn resolve(&self, binding: &WorkerHostBinding) -> Result<ImplementerAccess, WorkerHostError>;
}

impl<F> ImplementerAccessResolver for F
where
    F: Fn(&WorkerHostBinding) -> Result<ImplementerAccess, WorkerHostError> + Send + Sync,
{
    fn resolve(&self, binding: &WorkerHostBinding) -> Result<ImplementerAccess, WorkerHostError> {
        self(binding)
    }
}

pub struct WorkerFileHostFactory<P, R> {
    provider: Arc<P>,
    resolver: Arc<R>,
    profile: Option<WorkerProfile>,
}

impl<P, R> WorkerFileHostFactory<P, R> {
    pub fn new(provider: Arc<P>, resolver: Arc<R>) -> Self {
        Self::new_for_profile(WorkerProfile::Implementer, provider, resolver)
    }

    pub fn new_for_profile(profile: WorkerProfile, provider: Arc<P>, resolver: Arc<R>) -> Self {
        Self {
            provider,
            resolver,
            profile: Some(profile),
        }
    }

    /// Construct one file-host factory for Change profiles and the read-only
    /// local Explorer profile. The opaque binding still selects the exact
    /// capability, so this does not widen any worker surface.
    pub fn new_for_change(provider: Arc<P>, resolver: Arc<R>) -> Self {
        Self {
            provider,
            resolver,
            profile: None,
        }
    }
}

pub type ImplementerWorkerHostFactory<P, R> = WorkerFileHostFactory<P, R>;

impl<P, R> WorkerHostFactory for WorkerFileHostFactory<P, R>
where
    P: WorktreeFileProvider + 'static,
    R: ImplementerAccessResolver + 'static,
{
    type Task = ImplementerWorkerHost<P>;

    fn start(&self, binding: WorkerHostBinding) -> Result<Self::Task, WorkerHostError> {
        if self
            .profile
            .is_some_and(|profile| binding.worker_profile() != profile)
            || !matches!(
                (binding.task_shape(), binding.worker_profile()),
                (
                    TaskShape::Change,
                    WorkerProfile::Implementer | WorkerProfile::Verifier
                ) | (TaskShape::Explore, WorkerProfile::Explorer)
            )
        {
            return Err(WorkerHostError::CapabilityDenied);
        }
        let access = self
            .resolver
            .resolve(&binding)
            .map_err(|_| WorkerHostError::BindingUnavailable)?;
        let valid_profile_capability = match binding.worker_profile() {
            WorkerProfile::Implementer => {
                access.capability.operation == WorktreeOperation::Write
                    && !access.capability.write_roots.is_empty()
            }
            WorkerProfile::Verifier => {
                access.capability.operation == WorktreeOperation::Read
                    && access.capability.write_roots.is_empty()
            }
            WorkerProfile::Explorer => {
                access.capability.operation == WorktreeOperation::Read
                    && access.capability.write_roots.is_empty()
            }
        };
        if access.identity.attempt_id != binding.identity().attempt_id()
            || access.identity.work_package_id != binding.identity().work_package_id()
            || access.identity.lease_epoch != binding.lease_epoch()
            || access.capability.lease_epoch != binding.lease_epoch()
            || !valid_profile_capability
        {
            return Err(WorkerHostError::CapabilityDenied);
        }
        let file_access = self
            .provider
            .open_file_access(&access.identity, access.capability.clone())
            .map_err(|_| WorkerHostError::WorktreeUnavailable)?;
        Ok(ImplementerWorkerHost {
            binding,
            provider: self.provider.clone(),
            file_access,
            stopped: false,
            events: Vec::new(),
        })
    }
}

pub struct ImplementerWorkerHost<P> {
    binding: WorkerHostBinding,
    provider: Arc<P>,
    file_access: WorktreeAccess,
    stopped: bool,
    events: Vec<WorkerHostEvent>,
}

impl<P> ImplementerWorkerHost<P> {
    fn validate_request(&self, request: &WorkerHostRequest) -> Result<(), WorkerHostError> {
        if request.identity().attempt_id() != self.binding.identity().attempt_id()
            || request.identity().delegation_id() != self.binding.identity().delegation_id()
            || request.identity().work_package_id() != self.binding.identity().work_package_id()
            || request.lease_epoch() != self.binding.lease_epoch()
        {
            return Err(WorkerHostError::StaleEvent);
        }
        Ok(())
    }

    fn record(&mut self, event: WorkerHostEvent) -> WorkerHostEvent {
        self.events.push(event.clone());
        event
    }
}

#[async_trait]
impl<P> WorkerTask for ImplementerWorkerHost<P>
where
    P: WorktreeFileProvider + 'static,
{
    async fn dispatch(&mut self, request: WorkerHostRequest) -> Result<(), WorkerHostError> {
        self.dispatch_with_result(request).await.map(|_| ())
    }

    async fn dispatch_with_result(
        &mut self,
        request: WorkerHostRequest,
    ) -> Result<WorkerHostResult, WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        self.validate_request(&request)?;
        match request.operation() {
            WorkerHostOperation::Read { reference } => {
                let bytes = self
                    .provider
                    .read_file(&self.file_access, reference, MAX_READ_BYTES)
                    .map_err(|_| WorkerHostError::CapabilityDenied)?;
                let text =
                    String::from_utf8(bytes).map_err(|_| WorkerHostError::CapabilityDenied)?;
                Ok(WorkerHostResult::Text(text))
            }
            WorkerHostOperation::WriteCandidate {
                reference,
                contents,
            } => {
                self.provider
                    .write_candidate(&self.file_access, reference, contents, MAX_WRITE_BYTES)
                    .map_err(|_| WorkerHostError::CapabilityDenied)?;
                Ok(WorkerHostResult::Acknowledged)
            }
            _ => Err(WorkerHostError::CapabilityDenied),
        }
    }

    async fn heartbeat(&mut self) -> Result<WorkerHostEvent, WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        Ok(self.record(WorkerHostEvent::heartbeat(
            self.binding.identity(),
            self.binding.lease_epoch(),
        )))
    }

    async fn deliver(
        &mut self,
        delivery: DelegationDelivery,
    ) -> Result<WorkerHostEvent, WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        if delivery.identity.attempt_id != self.binding.identity().attempt_id()
            || delivery.identity.delegation_id != self.binding.identity().delegation_id()
        {
            return Err(WorkerHostError::DeliveryIdentityMismatch);
        }
        Ok(self.record(WorkerHostEvent::Delivery {
            identity: self.binding.identity(),
            lease_epoch: self.binding.lease_epoch(),
            delivery,
        }))
    }

    async fn stop(
        &mut self,
        terminal: WorkerHostTerminal,
    ) -> Result<WorkerHostEvent, WorkerHostError> {
        if self.stopped {
            return Err(WorkerHostError::TaskStopped);
        }
        self.stopped = true;
        Ok(self.record(WorkerHostEvent::Terminal {
            identity: self.binding.identity(),
            lease_epoch: self.binding.lease_epoch(),
            terminal,
        }))
    }
}
