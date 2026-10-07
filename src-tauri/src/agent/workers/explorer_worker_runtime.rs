//! Minimal, explicitly injected runtime for explorer workers.
//!
//! This module is the adapter between the asynchronous `WorkerHost` contract
//! and the synchronous polling seam used by `AsyncWorkerLauncherBridge`.  It
//! intentionally does not read `WorkerDispatch::payload_json`: plans are
//! supplied by a trusted parent-side loader as a bounded typed value.  The
//! worker receives only an opaque binding and a host factory; it never gets a
//! database connection, a filesystem path, or a capability root.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
        Arc,
    },
    time::Duration,
};

use super::{
    delegated_worker_launcher::{
        AsyncWorkerLauncher, WorkerAttemptBinding, WorkerLaunchError, WorkerLaunchRequest,
        WorkerTask as PollableWorkerTask, WorkerTaskEvent,
    },
    delegation_contract::{DelegationDelivery, DeliveryStatus},
    delegation_runtime::{Heartbeat, StopReason, WorkerDispatch},
    explorer_worker_host_adapter::{ExplorerGatewayContext, ExplorerGatewaySnapshot},
    worker_host_contract::{
        WorkerHostBinding, WorkerHostError, WorkerHostEvent, WorkerHostFactory,
        WorkerHostOperation, WorkerHostRequest, WorkerHostTerminal, WorkerTask,
    },
    worker_policy::{TaskShape, WorkerProfile},
};
use crate::agent::tool::ToolCancellation;

const MAX_PLAN_OPERATIONS: usize = 12;
const MAX_PLAN_SEARCHES: usize = 8;
const MAX_PLAN_FETCHES: usize = 8;
const MAX_PROVIDER_HOST_CHARS: usize = 253;
const MAX_QUERY_CHARS: usize = 2_048;
const MAX_URL_CHARS: usize = 4_096;
const JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// A parent-authored explorer operation.  It is not deserialised from worker
/// or model text; callers construct it after applying their own policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplorerOperation {
    Search {
        provider_host: String,
        query: String,
    },
    Fetch {
        url: String,
        method: String,
    },
}

impl ExplorerOperation {
    fn validate(&self) -> Result<(), WorkerLaunchError> {
        match self {
            Self::Search {
                provider_host,
                query,
            } => {
                if provider_host.trim().is_empty()
                    || provider_host.chars().count() > MAX_PROVIDER_HOST_CHARS
                    || provider_host.contains('/')
                    || provider_host.contains("://")
                    || query.trim().is_empty()
                    || query.chars().count() > MAX_QUERY_CHARS
                {
                    return Err(WorkerLaunchError::Launcher(
                        "invalid explorer search operation".into(),
                    ));
                }
            }
            Self::Fetch { url, method } => {
                let method = method.trim().to_ascii_uppercase();
                if url.trim().is_empty()
                    || !url.starts_with("https://")
                    || url.chars().count() > MAX_URL_CHARS
                    || !(method == "GET" || method == "HEAD")
                {
                    return Err(WorkerLaunchError::Launcher(
                        "invalid explorer fetch operation".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn request(&self, binding: &WorkerHostBinding) -> Result<WorkerHostRequest, WorkerHostError> {
        let operation = match self {
            Self::Search {
                provider_host,
                query,
            } => WorkerHostOperation::NetworkSearch {
                provider_host: provider_host.clone(),
                query: query.clone(),
            },
            Self::Fetch { url, method } => WorkerHostOperation::NetworkFetch {
                url: url.clone(),
                method: method.clone(),
            },
        };
        WorkerHostRequest::new(binding, operation)
    }
}

/// A bounded typed plan made by the trusted parent side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerWorkPlan {
    pub operations: Vec<ExplorerOperation>,
    pub delivery: DelegationDelivery,
}

impl ExplorerWorkPlan {
    pub fn new(
        operations: Vec<ExplorerOperation>,
        delivery: DelegationDelivery,
    ) -> Result<Self, WorkerLaunchError> {
        if operations.is_empty() || operations.len() > MAX_PLAN_OPERATIONS {
            return Err(WorkerLaunchError::Launcher(
                "explorer operation bound exceeded".into(),
            ));
        }
        let searches = operations
            .iter()
            .filter(|operation| matches!(operation, ExplorerOperation::Search { .. }))
            .count();
        let fetches = operations
            .iter()
            .filter(|operation| matches!(operation, ExplorerOperation::Fetch { .. }))
            .count();
        if searches > MAX_PLAN_SEARCHES || fetches > MAX_PLAN_FETCHES {
            return Err(WorkerLaunchError::Launcher(
                "explorer operation kind bound exceeded".into(),
            ));
        }
        for operation in &operations {
            operation.validate()?;
        }
        if delivery.status != DeliveryStatus::Completed
            || delivery.identity.attempt_id.trim().is_empty()
            || delivery.identity.delegation_id.trim().is_empty()
        {
            return Err(WorkerLaunchError::Launcher(
                "explorer delivery must be completed and identified".into(),
            ));
        }
        Ok(Self {
            operations,
            delivery,
        })
    }
}

/// Parent-side source for a typed plan.  Implementations may resolve a plan
/// from durable state, but must return a complete bounded value; child input
/// and arbitrary JSON are never consulted by this seam.
pub trait ExplorerWorkPlanLoader: Send + Sync {
    fn load(
        &self,
        binding: &WorkerAttemptBinding,
        dispatch: &WorkerDispatch,
    ) -> Result<ExplorerWorkPlan, WorkerLaunchError>;
}

/// Loader for a live gateway capability snapshot.  The snapshot is fetched on
/// every network operation, so a previously valid lease cannot be replayed
/// after expiry/revocation.
pub trait ExplorerGatewayContextLoader: Send + Sync {
    fn snapshot(
        &self,
        binding: &WorkerHostBinding,
        now_ms: i64,
    ) -> Result<ExplorerGatewaySnapshot, String>;
}

/// Adapter that lets `GatewayExplorerPort` use a loader without retaining a
/// database connection or a filesystem path.
#[derive(Clone)]
pub struct LoadedExplorerGatewayContext<L> {
    loader: Arc<L>,
}

impl<L> LoadedExplorerGatewayContext<L> {
    pub fn new(loader: Arc<L>) -> Self {
        Self { loader }
    }
}

impl<L> ExplorerGatewayContext for LoadedExplorerGatewayContext<L>
where
    L: ExplorerGatewayContextLoader,
{
    fn snapshot(
        &self,
        binding: &WorkerHostBinding,
        now_ms: i64,
    ) -> Result<ExplorerGatewaySnapshot, String> {
        self.loader.snapshot(binding, now_ms)
    }
}

/// No-op plan source used only to make the default launcher fail closed.
#[derive(Default)]
pub struct FailClosedExplorerPlanLoader;

impl ExplorerWorkPlanLoader for FailClosedExplorerPlanLoader {
    fn load(
        &self,
        _: &WorkerAttemptBinding,
        _: &WorkerDispatch,
    ) -> Result<ExplorerWorkPlan, WorkerLaunchError> {
        Err(WorkerLaunchError::Launcher(
            "explorer work plan loader is not configured".into(),
        ))
    }
}

#[derive(Debug)]
enum RuntimeEvent {
    Worker(WorkerTaskEvent),
}

/// Async launcher that runs only explorer bindings.  It is deliberately
/// generic over both the worker-host factory and parent-side plan loader so
/// production can inject a real gateway while tests use fakes.
pub struct ExplorerWorkerLauncher<F, P> {
    factory: Arc<F>,
    plans: Arc<P>,
    now_ms: fn() -> i64,
}

impl<F, P> ExplorerWorkerLauncher<F, P>
where
    F: WorkerHostFactory + 'static,
    P: ExplorerWorkPlanLoader + 'static,
{
    pub fn new(factory: F, plans: P, now_ms: fn() -> i64) -> Self {
        Self {
            factory: Arc::new(factory),
            plans: Arc::new(plans),
            now_ms,
        }
    }

    pub fn factory(&self) -> &Arc<F> {
        &self.factory
    }
}

impl<F, P> AsyncWorkerLauncher for ExplorerWorkerLauncher<F, P>
where
    F: WorkerHostFactory + 'static,
    P: ExplorerWorkPlanLoader + 'static,
{
    fn launch(
        &mut self,
        request: WorkerLaunchRequest,
    ) -> Result<Box<dyn PollableWorkerTask>, WorkerLaunchError> {
        let binding = request.binding;
        let dispatch = request.dispatch;
        // ResolvedExplorerWorkerAdapter supplies the durable typed plan. The
        // loader fallback remains only for generic direct launcher tests and
        // still returns a typed plan; payload_json is never inspected.
        let plan = match request.plan {
            Some(plan) => plan,
            None => self.plans.load(&binding, &dispatch)?,
        };
        if plan.delivery.identity.attempt_id != binding.attempt_id
            || plan.delivery.identity.delegation_id != binding.delegation_id
        {
            return Err(WorkerLaunchError::BindingMismatch);
        }

        // Explorer is intentionally the only profile accepted by this launch
        // path. Implementers/verifiers require separate capability adapters.
        let host_binding = WorkerHostBinding::new(
            binding.attempt_id.clone(),
            binding.delegation_id.clone(),
            binding.work_package_id.clone(),
            binding.lease_epoch,
            binding.capability_ref.clone(),
            WorkerProfile::Explorer,
            TaskShape::Explore,
        )
        .map_err(|_| WorkerLaunchError::InvalidBinding)?;
        let host = self
            .factory
            .start(host_binding.clone())
            .map_err(|error| WorkerLaunchError::Launcher(error.to_string()))?;
        let (event_tx, event_rx) = mpsc::channel();
        let cancellation = ToolCancellation::new();
        let terminal_sent = Arc::new(AtomicBool::new(false));
        let worker = ExplorerWorkerTask {
            attempt_id: binding.attempt_id,
            lease_epoch: binding.lease_epoch,
            event_rx,
            cancellation: cancellation.clone(),
            terminal_sent: terminal_sent.clone(),
            finished: false,
        };
        let now_ms = self.now_ms;
        tauri::async_runtime::spawn(run_explorer_host(
            host,
            host_binding,
            plan,
            event_tx,
            cancellation,
            terminal_sent,
            now_ms,
        ));
        Ok(Box::new(worker))
    }
}

/// A bridge that can be injected directly as a `WorkerAdapter` when a caller
/// already owns the binding map. Most application code should pass the
/// launcher to `DelegationPump`, which adds the shared callback bridge.
pub type ExplorerWorkerAdapter<F, P> =
    super::delegated_worker_launcher::AsyncWorkerLauncherBridge<ExplorerWorkerLauncher<F, P>>;

struct ExplorerWorkerTask {
    attempt_id: String,
    lease_epoch: u64,
    event_rx: Receiver<RuntimeEvent>,
    cancellation: ToolCancellation,
    terminal_sent: Arc<AtomicBool>,
    finished: bool,
}

impl PollableWorkerTask for ExplorerWorkerTask {
    fn poll(&mut self) -> Result<Option<WorkerTaskEvent>, WorkerLaunchError> {
        match self.event_rx.try_recv() {
            Ok(RuntimeEvent::Worker(event)) => {
                if matches!(event, WorkerTaskEvent::Terminal { .. }) {
                    self.finished = true;
                }
                Ok(Some(event))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) if self.finished => Ok(None),
            Err(TryRecvError::Disconnected) => Err(WorkerLaunchError::Launcher(
                "explorer worker event channel closed".into(),
            )),
        }
    }

    fn cancel(&mut self, _: StopReason) -> Result<(), WorkerLaunchError> {
        self.cancellation.cancel();
        Ok(())
    }

    fn join(&mut self) -> Result<Vec<WorkerTaskEvent>, WorkerLaunchError> {
        let mut events = Vec::new();
        while !self.finished {
            match self.event_rx.recv_timeout(JOIN_TIMEOUT) {
                Ok(RuntimeEvent::Worker(event)) => {
                    if matches!(event, WorkerTaskEvent::Terminal { .. }) {
                        self.finished = true;
                    }
                    events.push(event);
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(WorkerLaunchError::Launcher(
                        "explorer worker did not terminate".into(),
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(events)
    }
}

async fn run_explorer_host<T>(
    mut host: T,
    binding: WorkerHostBinding,
    plan: ExplorerWorkPlan,
    event_tx: Sender<RuntimeEvent>,
    cancellation: ToolCancellation,
    terminal_sent: Arc<AtomicBool>,
    now_ms: fn() -> i64,
) where
    T: WorkerTask,
{
    let attempt_id = binding.identity().attempt_id().to_string();
    let expected_epoch = binding.lease_epoch();

    if emit_heartbeat(&mut host, &event_tx, &attempt_id, expected_epoch, 0)
        .await
        .is_err()
    {
        send_terminal(
            &mut host,
            DeliveryStatus::Failed,
            "explorer heartbeat failed".into(),
            &event_tx,
            &terminal_sent,
            &attempt_id,
            expected_epoch,
        )
        .await;
        return;
    }

    for (index, operation) in plan.operations.iter().enumerate() {
        if cancellation.is_cancelled() {
            send_terminal(
                &mut host,
                DeliveryStatus::Cancelled,
                "explorer worker cancelled".into(),
                &event_tx,
                &terminal_sent,
                &attempt_id,
                expected_epoch,
            )
            .await;
            return;
        }
        let request = match operation.request(&binding) {
            Ok(request) => request,
            Err(error) => {
                send_terminal(
                    &mut host,
                    DeliveryStatus::Failed,
                    error.to_string(),
                    &event_tx,
                    &terminal_sent,
                    &attempt_id,
                    expected_epoch,
                )
                .await;
                return;
            }
        };
        match host
            .dispatch_cancellable(request, cancellation.clone())
            .await
        {
            Ok(()) if cancellation.is_cancelled() => {
                send_terminal(
                    &mut host,
                    DeliveryStatus::Cancelled,
                    "explorer worker cancelled".into(),
                    &event_tx,
                    &terminal_sent,
                    &attempt_id,
                    expected_epoch,
                )
                .await;
                return;
            }
            Ok(()) => {}
            Err(WorkerHostError::Cancelled) => {
                send_terminal(
                    &mut host,
                    DeliveryStatus::Cancelled,
                    "explorer worker cancelled".into(),
                    &event_tx,
                    &terminal_sent,
                    &attempt_id,
                    expected_epoch,
                )
                .await;
                return;
            }
            Err(error) => {
                send_terminal(
                    &mut host,
                    DeliveryStatus::Failed,
                    error.to_string(),
                    &event_tx,
                    &terminal_sent,
                    &attempt_id,
                    expected_epoch,
                )
                .await;
                return;
            }
        }
        let progress = (((index + 1) * 80) / plan.operations.len()) as u8;
        if emit_heartbeat(&mut host, &event_tx, &attempt_id, expected_epoch, progress)
            .await
            .is_err()
        {
            send_terminal(
                &mut host,
                DeliveryStatus::Failed,
                "explorer heartbeat failed".into(),
                &event_tx,
                &terminal_sent,
                &attempt_id,
                expected_epoch,
            )
            .await;
            return;
        }
    }

    if cancellation.is_cancelled() {
        send_terminal(
            &mut host,
            DeliveryStatus::Cancelled,
            "explorer worker cancelled".into(),
            &event_tx,
            &terminal_sent,
            &attempt_id,
            expected_epoch,
        )
        .await;
        return;
    }
    match host.deliver(plan.delivery).await {
        Ok(event) => {
            let _ = send_host_event(event, &event_tx, &attempt_id, expected_epoch, 90);
            let _ = now_ms;
            send_terminal(
                &mut host,
                DeliveryStatus::Completed,
                "explorer worker completed".into(),
                &event_tx,
                &terminal_sent,
                &attempt_id,
                expected_epoch,
            )
            .await;
        }
        Err(error) => {
            send_terminal(
                &mut host,
                DeliveryStatus::Failed,
                error.to_string(),
                &event_tx,
                &terminal_sent,
                &attempt_id,
                expected_epoch,
            )
            .await;
        }
    }
}

async fn emit_heartbeat<T: WorkerTask>(
    host: &mut T,
    event_tx: &Sender<RuntimeEvent>,
    attempt_id: &str,
    expected_epoch: u64,
    progress: u8,
) -> Result<(), WorkerLaunchError> {
    match host.heartbeat().await {
        Ok(event) => send_host_event(event, event_tx, attempt_id, expected_epoch, progress),
        Err(error) => Err(WorkerLaunchError::Launcher(error.to_string())),
    }
}

async fn send_terminal<T: WorkerTask>(
    host: &mut T,
    status: DeliveryStatus,
    reason: String,
    event_tx: &Sender<RuntimeEvent>,
    terminal_sent: &Arc<AtomicBool>,
    attempt_id: &str,
    expected_epoch: u64,
) {
    if terminal_sent.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Ok(terminal) = WorkerHostTerminal::new(status, reason) {
        if let Ok(event) = host.stop(terminal).await {
            let _ = send_host_event(event, event_tx, attempt_id, expected_epoch, 100);
        }
    }
}

fn send_host_event(
    event: WorkerHostEvent,
    event_tx: &Sender<RuntimeEvent>,
    attempt_id: &str,
    expected_epoch: u64,
    progress: u8,
) -> Result<(), WorkerLaunchError> {
    let event = match event {
        WorkerHostEvent::Heartbeat {
            identity,
            lease_epoch: event_epoch,
        } => {
            if identity.attempt_id() != attempt_id || event_epoch != expected_epoch {
                return Err(WorkerLaunchError::BindingMismatch);
            }
            WorkerTaskEvent::Heartbeat(Heartbeat {
                attempt_id: attempt_id.into(),
                epoch: event_epoch,
                stage: "explorer".into(),
                progress_percent: progress,
                budget_remaining: "bounded".into(),
                evidence_refs: Vec::new(),
            })
        }
        WorkerHostEvent::Delivery {
            identity,
            lease_epoch: event_epoch,
            delivery,
        } => {
            if identity.attempt_id() != attempt_id
                || identity.delegation_id() != delivery.identity.delegation_id
                || event_epoch != expected_epoch
            {
                return Err(WorkerLaunchError::BindingMismatch);
            }
            WorkerTaskEvent::Delivery {
                delivery,
                lease_epoch: event_epoch,
            }
        }
        WorkerHostEvent::Terminal {
            identity,
            lease_epoch: event_epoch,
            terminal,
        } => {
            if identity.attempt_id() != attempt_id || event_epoch != expected_epoch {
                return Err(WorkerLaunchError::BindingMismatch);
            }
            WorkerTaskEvent::Terminal {
                attempt_id: attempt_id.into(),
                lease_epoch: event_epoch,
                reason: terminal.reason,
            }
        }
    };
    event_tx
        .send(RuntimeEvent::Worker(event))
        .map_err(|_| WorkerLaunchError::Launcher("explorer event sink closed".into()))
}

/// A minimal context loader for tests or explicit callers. Production callers
/// should replace it with a loader that reads the current lease/package rows.
pub struct StaticExplorerGatewayContextLoader {
    snapshot: ExplorerGatewaySnapshot,
}

impl StaticExplorerGatewayContextLoader {
    pub fn new(snapshot: ExplorerGatewaySnapshot) -> Self {
        Self { snapshot }
    }
}

impl ExplorerGatewayContextLoader for StaticExplorerGatewayContextLoader {
    fn snapshot(&self, _: &WorkerHostBinding, _: i64) -> Result<ExplorerGatewaySnapshot, String> {
        Ok(self.snapshot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        delegated_worker_launcher::{AsyncWorkerLauncherBridge, WorkerTaskEvent},
        delegation_contract::{
            Confidence, DelegationIdentity, KeyFact, Milestone, UsageFlags, VerificationRecord,
            VerificationStatus,
        },
        delegation_runtime::WorkerAdapter,
        explorer_worker_host_adapter::{
            BoundedExplorerEvidence, ExplorerGatewayPort, ExplorerWorkerHostFactory,
        },
    };
    use async_trait::async_trait;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeGateway {
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ExplorerGatewayPort for FakeGateway {
        async fn search(
            &self,
            _: &WorkerHostBinding,
            host: &str,
            _: &str,
            _: ToolCancellation,
        ) -> Result<Vec<BoundedExplorerEvidence>, String> {
            self.calls.lock().unwrap().push(format!("search:{host}"));
            Ok(vec![BoundedExplorerEvidence::new(
                "evidence://search",
                host,
            )
            .unwrap()])
        }

        async fn fetch(
            &self,
            _: &WorkerHostBinding,
            url: &str,
            _: &str,
            _: ToolCancellation,
        ) -> Result<BoundedExplorerEvidence, String> {
            self.calls.lock().unwrap().push(format!("fetch:{url}"));
            Ok(BoundedExplorerEvidence::new("evidence://fetch", "docs.example").unwrap())
        }
    }

    #[derive(Clone)]
    struct PlanLoader {
        plan: ExplorerWorkPlan,
    }
    impl ExplorerWorkPlanLoader for PlanLoader {
        fn load(
            &self,
            _: &WorkerAttemptBinding,
            _: &WorkerDispatch,
        ) -> Result<ExplorerWorkPlan, WorkerLaunchError> {
            Ok(self.plan.clone())
        }
    }

    fn binding() -> WorkerAttemptBinding {
        WorkerAttemptBinding {
            attempt_id: "attempt-1".into(),
            delegation_id: "delegation-1".into(),
            work_package_id: "package-1".into(),
            lease_epoch: 7,
            capability_ref: "cap-1".into(),
            sandbox_ref: "sandbox-1".into(),
        }
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
            executive_summary: "explored".into(),
            key_facts: vec![KeyFact {
                statement: "bounded".into(),
                confidence: Confidence::High,
                evidence_refs: vec![],
            }],
            milestones: vec![Milestone {
                label: "network".into(),
                outcome: "done".into(),
                evidence_refs: vec![],
            }],
            verifications: vec![VerificationRecord {
                item: "gateway".into(),
                method: "fake".into(),
                status: VerificationStatus::Passed,
                conclusion: "ok".into(),
                evidence_ref: "evidence://search".into(),
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

    fn launcher() -> ExplorerWorkerLauncher<ExplorerWorkerHostFactory<FakeGateway>, PlanLoader> {
        let plan = ExplorerWorkPlan::new(
            vec![
                ExplorerOperation::Search {
                    provider_host: "search.example".into(),
                    query: "rust".into(),
                },
                ExplorerOperation::Fetch {
                    url: "https://docs.example/a".into(),
                    method: "GET".into(),
                },
            ],
            delivery(),
        )
        .unwrap();
        ExplorerWorkerLauncher::new(
            ExplorerWorkerHostFactory::new(FakeGateway::default()),
            PlanLoader { plan },
            || 1,
        )
    }

    #[test]
    fn typed_plan_rejects_unbounded_or_invalid_operations() {
        let mut operations = Vec::new();
        for _ in 0..(MAX_PLAN_OPERATIONS + 1) {
            operations.push(ExplorerOperation::Search {
                provider_host: "search.example".into(),
                query: "x".into(),
            });
        }
        assert!(ExplorerWorkPlan::new(operations, delivery()).is_err());
        assert!(ExplorerWorkPlan::new(
            vec![ExplorerOperation::Fetch {
                url: "http://insecure.example".into(),
                method: "GET".into(),
            }],
            delivery(),
        )
        .is_err());
    }

    #[tokio::test]
    async fn explorer_launcher_routes_search_fetch_heartbeat_delivery_terminal() {
        let mut launcher = launcher();
        let b = binding();
        let request = WorkerLaunchRequest {
            dispatch: WorkerDispatch {
                outbox_id: "out-1".into(),
                delegation_id: b.delegation_id.clone(),
                attempt_id: b.attempt_id.clone(),
                payload_json: "opaque".into(),
                epoch: b.lease_epoch,
            },
            binding: b.clone(),
            plan: None,
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let callback = Box::new(move |event: WorkerTaskEvent| {
            sink.lock().unwrap().push(event);
            Ok(())
        });
        let mut bridge = AsyncWorkerLauncherBridge::new(launcher, callback);
        bridge.bind(b).unwrap();
        // AsyncWorkerLauncherBridge validates the binding before launch.
        bridge.start(request.dispatch.clone()).unwrap();
        for _ in 0..100 {
            bridge.poll().unwrap();
            if bridge.active_attempts() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let events = events.lock().unwrap();
        assert!(events
            .iter()
            .any(|event| matches!(event, WorkerTaskEvent::Heartbeat(_))));
        assert!(events
            .iter()
            .any(|event| matches!(event, WorkerTaskEvent::Delivery { .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event, WorkerTaskEvent::Terminal { .. })));
    }

    struct StalledGateway {
        started: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl ExplorerGatewayPort for StalledGateway {
        async fn search(
            &self,
            _: &WorkerHostBinding,
            _: &str,
            _: &str,
            cancellation: ToolCancellation,
        ) -> Result<Vec<BoundedExplorerEvidence>, String> {
            self.started.notify_one();
            cancellation.cancelled().await;
            Err("network operation cancelled".into())
        }

        async fn fetch(
            &self,
            _: &WorkerHostBinding,
            _: &str,
            _: &str,
            _: ToolCancellation,
        ) -> Result<BoundedExplorerEvidence, String> {
            Err("fetch is not used in this test".into())
        }
    }

    #[tokio::test]
    async fn cancelling_a_running_explorer_stops_the_in_flight_network_operation_without_delivery()
    {
        let started = Arc::new(tokio::sync::Notify::new());
        let plan = ExplorerWorkPlan::new(
            vec![ExplorerOperation::Search {
                provider_host: "search.example".into(),
                query: "bounded".into(),
            }],
            delivery(),
        )
        .unwrap();
        let mut launcher = ExplorerWorkerLauncher::new(
            ExplorerWorkerHostFactory::new(StalledGateway {
                started: started.clone(),
            }),
            PlanLoader { plan },
            || 1,
        );
        let binding = binding();
        let started_wait = started.notified();
        let mut task = launcher
            .launch(WorkerLaunchRequest {
                dispatch: WorkerDispatch {
                    outbox_id: "out-cancel".into(),
                    delegation_id: binding.delegation_id.clone(),
                    attempt_id: binding.attempt_id.clone(),
                    payload_json: "opaque".into(),
                    epoch: binding.lease_epoch,
                },
                binding,
                plan: None,
            })
            .unwrap();
        started_wait.await;
        task.cancel(StopReason::Cancelled).unwrap();
        let events = tokio::time::timeout(
            Duration::from_millis(500),
            tokio::task::spawn_blocking(move || task.join()),
        )
        .await
        .expect("cancellation must not wait for the network timeout")
        .unwrap()
        .unwrap();

        assert!(events
            .iter()
            .any(|event| matches!(event, WorkerTaskEvent::Terminal { reason, .. } if reason == "explorer worker cancelled")));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, WorkerTaskEvent::Delivery { .. })),
            "a cancelled operation must not deliver evidence or a completed result"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, WorkerTaskEvent::Terminal { .. }))
                .count(),
            1,
            "cancellation must emit exactly one terminal event"
        );
    }
}
