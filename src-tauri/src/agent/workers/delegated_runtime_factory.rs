//! Single production composition root for delegated worker profiles.
//!
//! Explorer (and, once its review gate is enabled, Change) shares one
//! `DelegationRuntime`, one scheduler, one pump, and one async bridge. A disabled or partially
//! constructible graph returns the existing fail-closed pump; it never starts
//! a second loop or silently falls back to a foreground model.

use std::sync::Arc;

use super::{
    change_pipeline::ChangePipeline,
    change_worker_runtime::{ChangeWorkerLauncher, DurableWorktreeAccessResolver},
    concrete_sandbox::ProductionResourceCleanupEffects,
    delegated_model_host::DeferredKeychainDelegatedModelHostFactory,
    delegated_network_scope::SharedDbApprovedNetworkScopeLookup,
    delegated_worker_router::RoutedWorkerAdapter,
    delegation_contract::ContractLimits,
    delegation_pump::{DelegationPump, DelegationPumpHandle, DelegationPumpHostHandle},
    delegation_runtime::{DelegationRuntime, FixedAdmissionPolicy, SandboxController, SystemClock},
    delegation_service::{DelegationService, DelegationServiceError, DelegationServiceHandle},
    explorer_plan::ExplorerBindingResolver,
    explorer_runtime_config::{
        decide, ExplorerOptIn, ExplorerRuntimeDecision, ExplorerRuntimeDependencies,
        ExplorerRuntimeDisableReason,
    },
    explorer_worker_host_adapter::{ExplorerWorkerHostFactory, GatewayExplorerPort},
    explorer_worker_runtime::{ExplorerWorkerLauncher, FailClosedExplorerPlanLoader},
    implementer_worker_host::WorkerFileHostFactory,
    network_gateway::{HostResolver, NetworkGateway, NetworkTransport},
    network_transport::{GatewayNetworkTransport, ReqwestPinnedHttpsConnector},
    policy_lease_issuer::MAX_RUNNING_ATTEMPTS,
    resource_binding::ResourceBindingRepository,
    review_agent::{BindingReviewModelResolver, LlmReviewExecutor, ReviewExecutor},
    review_coordinator::ReviewCoordinator,
    safe_host_resolver::SafeHostResolver,
    shared_db::SharedDb,
    terminal_cleanup::PersistentResourceCleanup,
    workspace_admission::WorkspaceAdmission,
    workspace_isolation::GitWorktreeProvider,
};

pub struct DelegatedRuntimeAssembly {
    pub pump: DelegationPumpHostHandle,
    pub enabled: bool,
    pub reason: Option<ExplorerRuntimeDisableReason>,
}

impl DelegatedRuntimeAssembly {
    pub fn into_pump(self) -> DelegationPumpHostHandle {
        self.pump
    }
}

pub struct DelegatedRuntimeFactory;

impl DelegatedRuntimeFactory {
    pub fn build(
        db: SharedDb,
        opt_in: ExplorerOptIn,
        dependencies: ExplorerRuntimeDependencies,
        sandbox: impl SandboxController + Send + 'static,
        cleanup_effects: ProductionResourceCleanupEffects,
        provider: Arc<GitWorktreeProvider>,
        model_factory: DeferredKeychainDelegatedModelHostFactory,
    ) -> Result<DelegatedRuntimeAssembly, DelegationServiceError> {
        Self::build_with_explorer_network_io(
            db,
            opt_in,
            dependencies,
            sandbox,
            cleanup_effects,
            provider,
            model_factory,
            GatewayNetworkTransport::<ReqwestPinnedHttpsConnector>::default(),
            SafeHostResolver::system(),
        )
    }

    /// Compose the normal delegated graph with a capability-local network I/O
    /// adapter.  The adapter is deliberately below the Explorer gateway: plan
    /// binding, source-policy approval, lease checks, bounded evidence, worker
    /// lifecycle, and delivery all stay identical for production and tests.
    pub(crate) fn build_with_explorer_network_io<T, R>(
        db: SharedDb,
        opt_in: ExplorerOptIn,
        dependencies: ExplorerRuntimeDependencies,
        sandbox: impl SandboxController + Send + 'static,
        cleanup_effects: ProductionResourceCleanupEffects,
        provider: Arc<GitWorktreeProvider>,
        model_factory: DeferredKeychainDelegatedModelHostFactory,
        transport: T,
        resolver: R,
    ) -> Result<DelegatedRuntimeAssembly, DelegationServiceError>
    where
        T: NetworkTransport + 'static,
        R: HostResolver + Send + Sync + 'static,
    {
        let review_resolver = BindingReviewModelResolver::new(db.clone(), model_factory.clone());
        let review_executor = Box::new(LlmReviewExecutor::new(Arc::new(review_resolver)));
        Self::build_with_explorer_runtime_adapters(
            db,
            opt_in,
            dependencies,
            sandbox,
            cleanup_effects,
            provider,
            model_factory,
            transport,
            resolver,
            review_executor,
        )
    }

    /// Compose the delegated graph from already capability-bounded adapters.
    ///
    /// This is intentionally a composition seam rather than a second worker
    /// path: callers can replace physical Explorer I/O or the independent
    /// reviewer, but cannot bypass source policy, leases, sandboxing, delivery
    /// persistence, or the scheduler. Production calls the narrower constructor
    /// above; deterministic desktop verification supplies both adapters here.
    pub(crate) fn build_with_explorer_runtime_adapters<T, R>(
        db: SharedDb,
        opt_in: ExplorerOptIn,
        dependencies: ExplorerRuntimeDependencies,
        sandbox: impl SandboxController + Send + 'static,
        cleanup_effects: ProductionResourceCleanupEffects,
        provider: Arc<GitWorktreeProvider>,
        model_factory: DeferredKeychainDelegatedModelHostFactory,
        transport: T,
        resolver: R,
        review_executor: Box<dyn ReviewExecutor>,
    ) -> Result<DelegatedRuntimeAssembly, DelegationServiceError>
    where
        T: NetworkTransport + 'static,
        R: HostResolver + Send + Sync + 'static,
    {
        if let ExplorerRuntimeDecision::Disabled(reason) = decide(opt_in, dependencies) {
            return Ok(DelegatedRuntimeAssembly {
                pump: fail_closed_pump(db)?,
                enabled: false,
                reason: Some(reason),
            });
        }

        let context =
            super::explorer_gateway_context::ExplorerGatewayContextLoader::new(db.clone());
        let policies = SharedDbApprovedNetworkScopeLookup::new(db.clone());
        let gateway = NetworkGateway::new(policies, transport, resolver);
        let context =
            super::explorer_worker_runtime::LoadedExplorerGatewayContext::new(Arc::new(context));
        let gateway_port = GatewayExplorerPort::new(gateway, context, now_millis);
        let explorer_host = ExplorerWorkerHostFactory::new(gateway_port);
        let explorer_launcher =
            ExplorerWorkerLauncher::new(explorer_host, FailClosedExplorerPlanLoader, now_millis);

        let change_access = Arc::new(DurableWorktreeAccessResolver::new(db.clone()));
        let change_host = Arc::new(WorkerFileHostFactory::new_for_change(
            provider.clone(),
            change_access,
        ));
        let change_launcher: Box<dyn super::delegated_worker_launcher::AsyncWorkerLauncher> =
            Box::new(ChangeWorkerLauncher::new(
                db.clone(),
                change_host,
                Arc::new(model_factory.clone()),
                None,
                now_seconds,
            ));
        let explorer_launcher: Box<dyn super::delegated_worker_launcher::AsyncWorkerLauncher> =
            Box::new(explorer_launcher);
        let worker = RoutedWorkerAdapter::new(
            db.clone(),
            ExplorerBindingResolver::new(super::explorer_plan::ExplorerPlanRepository::new(
                db.clone(),
            )),
            explorer_launcher,
            change_launcher,
            Box::new(|_| Ok(())),
            now_seconds,
        );
        let runtime = DelegationRuntime::with_sandbox(
            db.clone(),
            worker,
            SystemClock,
            FixedAdmissionPolicy {
                max_running_attempts: MAX_RUNNING_ATTEMPTS,
            },
            sandbox,
            WORKER_HEARTBEAT_TTL_SECS,
        )
        .map_err(|_| DelegationServiceError::Runtime)?;
        let scheduler =
            super::delegated_attempt_scheduler::DelegatedAttemptScheduler::with_sandbox(runtime);
        let cleanup = PersistentResourceCleanup::new(
            ResourceBindingRepository::new(db.clone()),
            cleanup_effects,
        );
        let review_coordinator = ReviewCoordinator::new(db.clone());
        let service = DelegationService::with_cleanup(
            db.clone(),
            scheduler,
            WorkspaceAdmission::new(),
            cleanup,
        )?
        .with_review(review_coordinator, review_executor)
        .with_change_pipeline(ChangePipeline::new(db.clone(), provider));
        let service = DelegationServiceHandle::new(service);
        let pump =
            DelegationPumpHandle::new(DelegationPump::new(service, ContractLimits::default()));
        Ok(DelegatedRuntimeAssembly {
            pump: DelegationPumpHostHandle::new(pump),
            enabled: true,
            reason: None,
        })
    }

    /// Construct the one durable fail-closed graph used for disabled or
    /// partially constructible deployments. Keeping this here makes the
    /// factory the only production composition root.
    pub fn fail_closed(db: SharedDb) -> Result<DelegatedRuntimeAssembly, DelegationServiceError> {
        Ok(DelegatedRuntimeAssembly {
            pump: fail_closed_pump(db)?,
            enabled: false,
            reason: Some(ExplorerRuntimeDisableReason::DependenciesUnavailable),
        })
    }
}

fn fail_closed_pump(db: SharedDb) -> Result<DelegationPumpHostHandle, DelegationServiceError> {
    let service = DelegationServiceHandle::fail_closed(db)?;
    let pump = DelegationPumpHandle::fail_closed(service, ContractLimits::default());
    Ok(DelegationPumpHostHandle::new(pump))
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// `DelegatedWorkerAdapter` emits a durable heartbeat before and after a model
// turn. A single bounded turn may run for up to five minutes, so the runtime
// lease must outlive that call; otherwise a healthy worker is sealed while it
// is still awaiting its model response. This remains shorter than the
// immutable 15-minute package lease.
const WORKER_HEARTBEAT_TTL_SECS: i64 = 330;

fn now_seconds() -> i64 {
    chrono::Utc::now().timestamp()
}
