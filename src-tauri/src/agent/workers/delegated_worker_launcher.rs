//! Narrow seam between the durable scheduler and an async delegated worker.
//!
//! The scheduler is intentionally synchronous: `WorkerAdapter::start` only
//! hands a claim to an injected launcher.  A real adapter may enqueue that
//! request onto the application's async executor, but it must not construct a
//! model loop, tool registry, or filesystem path here.  Every launch requires
//! an opaque Main-Agent binding; an unbound dispatch fails closed.

use std::collections::HashMap;

use super::{
    delegation_contract::DelegationDelivery,
    delegation_runtime::{Heartbeat, StopReason, WorkerAdapter, WorkerDispatch, WorkerEvent},
    explorer_worker_runtime::ExplorerWorkPlan,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerAttemptBinding {
    pub attempt_id: String,
    pub delegation_id: String,
    pub work_package_id: String,
    pub lease_epoch: u64,
    /// Opaque references.  Filesystem paths and capability roots never cross
    /// this seam; the launcher resolves them inside its own sandbox host.
    pub capability_ref: String,
    pub sandbox_ref: String,
}

impl WorkerAttemptBinding {
    pub fn validate_for(&self, dispatch: &WorkerDispatch) -> Result<(), WorkerLaunchError> {
        if self.attempt_id != dispatch.attempt_id || self.delegation_id != dispatch.delegation_id {
            return Err(WorkerLaunchError::BindingMismatch);
        }
        if self.lease_epoch != dispatch.epoch
            || self.work_package_id.trim().is_empty()
            || self.capability_ref.trim().is_empty()
            || self.sandbox_ref.trim().is_empty()
        {
            return Err(WorkerLaunchError::InvalidBinding);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLaunchRequest {
    pub dispatch: WorkerDispatch,
    pub binding: WorkerAttemptBinding,
    /// Typed parent-authored plan. Explorer resolution must populate this
    /// before crossing the launcher seam; `None` remains for generic adapters.
    pub plan: Option<ExplorerWorkPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerLaunchError {
    MissingBinding(String),
    BindingMismatch,
    InvalidBinding,
    DuplicateBinding,
    Launcher(String),
}

/// Adapter implemented by the async worker host.  `launch` must only enqueue
/// or hand off work; the host owns the async task and reports heartbeat and
/// structured delivery through the existing `DelegationRuntime`/Inbox ports.
pub trait DelegatedWorkerLauncher: Send {
    fn launch(&mut self, request: WorkerLaunchRequest) -> Result<(), WorkerLaunchError>;
    fn cancel(&mut self, attempt_id: &str, reason: StopReason) -> Result<(), WorkerLaunchError>;
}

/// Structured events emitted by a task running on the application's async
/// executor.  Raw model/tool logs deliberately have no representation here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerTaskEvent {
    Heartbeat(Heartbeat),
    Delivery {
        delivery: DelegationDelivery,
        lease_epoch: u64,
    },
    Terminal {
        attempt_id: String,
        lease_epoch: u64,
        reason: String,
    },
}

/// Handle returned by an async launcher. `poll` is non-blocking; `join` is
/// used only after cancellation/expiry to guarantee the task is no longer
/// able to emit a second terminal event.
pub trait WorkerTask: Send {
    fn poll(&mut self) -> Result<Option<WorkerTaskEvent>, WorkerLaunchError>;
    fn cancel(&mut self, reason: StopReason) -> Result<(), WorkerLaunchError>;
    fn join(&mut self) -> Result<Vec<WorkerTaskEvent>, WorkerLaunchError>;
}

/// Async execution host contract. The implementation may use Tokio/Tauri's
/// executor internally; this seam never owns an executor or a model/tool
/// registry. The returned task is retained by the bridge until terminal.
pub trait AsyncWorkerLauncher: Send {
    fn launch(
        &mut self,
        request: WorkerLaunchRequest,
    ) -> Result<Box<dyn WorkerTask>, WorkerLaunchError>;
}

/// Production placeholder until an async model host is explicitly injected.
/// It never creates a task, so fail-closed startup cannot dispatch work.
#[derive(Default)]
pub struct FailClosedAsyncLauncher;
impl AsyncWorkerLauncher for FailClosedAsyncLauncher {
    fn launch(&mut self, _: WorkerLaunchRequest) -> Result<Box<dyn WorkerTask>, WorkerLaunchError> {
        Err(WorkerLaunchError::Launcher(
            "async delegated launcher is not configured".into(),
        ))
    }
}

pub type WorkerEventCallback = Box<dyn FnMut(WorkerTaskEvent) -> Result<(), String> + Send>;

/// Scheduler bridge with task registry, cancellation/join, and exactly-once
/// terminal callback delivery. It is deliberately synchronous at its seam;
/// `poll` is called by the host's existing loop and does not create another
/// scheduler loop.
pub struct AsyncWorkerLauncherBridge<L> {
    launcher: L,
    bindings: HashMap<String, WorkerAttemptBinding>,
    tasks: HashMap<String, Box<dyn WorkerTask>>,
    callback: WorkerEventCallback,
}

impl<L> AsyncWorkerLauncherBridge<L>
where
    L: AsyncWorkerLauncher,
{
    pub fn new(launcher: L, callback: WorkerEventCallback) -> Self {
        Self {
            launcher,
            bindings: HashMap::new(),
            tasks: HashMap::new(),
            callback,
        }
    }

    pub fn bind(&mut self, binding: WorkerAttemptBinding) -> Result<(), WorkerLaunchError> {
        if let Some(existing) = self.bindings.get(&binding.attempt_id) {
            return if existing == &binding {
                Ok(())
            } else {
                Err(WorkerLaunchError::DuplicateBinding)
            };
        }
        self.bindings.insert(binding.attempt_id.clone(), binding);
        Ok(())
    }

    /// Drain at most one event per active task. The service's existing `tick`
    /// lease timer must call this method; the bridge never starts its own
    /// background loop. A terminal event removes the task before invoking the
    /// callback, preventing callback re-entry from observing an active task.
    pub fn poll(&mut self) -> Result<(), WorkerLaunchError> {
        let events = self.poll_events()?;
        for event in events {
            (self.callback)(event).map_err(WorkerLaunchError::Launcher)?;
        }
        Ok(())
    }

    /// Drain at most one event per active task without invoking the callback.
    /// DelegationService uses this path so worker events are applied after the
    /// bridge has released task state and never recursively lock the service
    /// handle from inside a worker callback.
    pub fn poll_events(&mut self) -> Result<Vec<WorkerTaskEvent>, WorkerLaunchError> {
        let mut events = Vec::new();
        let ids: Vec<String> = self.tasks.keys().cloned().collect();
        for attempt_id in ids {
            let event = self
                .tasks
                .get_mut(&attempt_id)
                .ok_or_else(|| WorkerLaunchError::MissingBinding(attempt_id.clone()))?
                .poll()?;
            let Some(event) = event else { continue };
            let binding = self
                .bindings
                .get(&attempt_id)
                .ok_or_else(|| WorkerLaunchError::MissingBinding(attempt_id.clone()))?;
            // A late callback from a cancelled/restarted task is discarded;
            // it must not reach the runtime or parent inbox.
            if !event_matches_binding(&event, binding) {
                continue;
            }
            let terminal = matches!(event, WorkerTaskEvent::Terminal { .. });
            if terminal {
                self.tasks.remove(&attempt_id);
            }
            events.push(event);
        }
        Ok(events)
    }

    pub fn active_attempts(&self) -> usize {
        self.tasks.len()
    }

    pub fn active_attempt_ids(&self) -> Vec<String> {
        self.tasks.keys().cloned().collect()
    }

    pub fn stop_all(&mut self, reason: StopReason) -> Result<(), WorkerLaunchError> {
        for attempt_id in self.active_attempt_ids() {
            self.stop(&attempt_id, reason)
                .map_err(WorkerLaunchError::Launcher)?;
        }
        Ok(())
    }

    pub fn launcher(&self) -> &L {
        &self.launcher
    }
}

impl<L> AsyncWorkerLauncherBridge<L>
where
    L: AsyncWorkerLauncher,
{
    /// Launch with a resolver-produced typed plan while retaining the bridge's
    /// single binding/task ownership.
    pub fn start_with_plan(
        &mut self,
        dispatch: WorkerDispatch,
        plan: Option<ExplorerWorkPlan>,
    ) -> Result<(), String> {
        let binding = self
            .bindings
            .get(&dispatch.attempt_id)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "{:?}",
                    WorkerLaunchError::MissingBinding(dispatch.attempt_id.clone())
                )
            })?;
        binding
            .validate_for(&dispatch)
            .map_err(|error| format!("{error:?}"))?;
        if self.tasks.contains_key(&dispatch.attempt_id) {
            return Err(format!("{:?}", WorkerLaunchError::DuplicateBinding));
        }
        let attempt_id = dispatch.attempt_id.clone();
        let task = self
            .launcher
            .launch(WorkerLaunchRequest {
                dispatch,
                binding,
                plan,
            })
            .map_err(|error| format!("{error:?}"))?;
        self.tasks.insert(attempt_id, task);
        Ok(())
    }
}

impl<L> WorkerAdapter for AsyncWorkerLauncherBridge<L>
where
    L: AsyncWorkerLauncher,
{
    fn start(&mut self, dispatch: WorkerDispatch) -> Result<(), String> {
        self.start_with_plan(dispatch, None)
    }

    fn stop(&mut self, attempt_id: &str, reason: StopReason) -> Result<(), String> {
        let Some(mut task) = self.tasks.remove(attempt_id) else {
            // Runtime recovery/expiry may race a task's terminal callback;
            // removing the task first makes repeated stop idempotent.
            return Ok(());
        };
        task.cancel(reason).map_err(|error| format!("{error:?}"))?;
        let binding = self.bindings.get(attempt_id).cloned();
        for event in task.join().map_err(|error| format!("{error:?}"))? {
            if binding
                .as_ref()
                .is_some_and(|binding| event_matches_binding(&event, binding))
            {
                (self.callback)(event).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    fn poll_workers(&mut self) -> Result<Vec<WorkerEvent>, String> {
        self.poll_events()
            .map(|events| events.into_iter().map(WorkerEvent::from).collect())
            .map_err(|error| format!("{error:?}"))
    }

    fn active_worker_attempts(&self) -> usize {
        self.active_attempts()
    }
}

impl From<WorkerTaskEvent> for WorkerEvent {
    fn from(event: WorkerTaskEvent) -> Self {
        match event {
            WorkerTaskEvent::Heartbeat(heartbeat) => Self::Heartbeat(heartbeat),
            WorkerTaskEvent::Delivery {
                delivery,
                lease_epoch,
            } => Self::Delivery {
                delivery,
                lease_epoch,
            },
            WorkerTaskEvent::Terminal {
                attempt_id,
                lease_epoch,
                reason,
            } => Self::Terminal {
                attempt_id,
                lease_epoch,
                reason,
            },
        }
    }
}

fn event_matches_binding(event: &WorkerTaskEvent, binding: &WorkerAttemptBinding) -> bool {
    match event {
        WorkerTaskEvent::Heartbeat(heartbeat) => {
            heartbeat.attempt_id == binding.attempt_id && heartbeat.epoch == binding.lease_epoch
        }
        WorkerTaskEvent::Delivery {
            delivery,
            lease_epoch,
        } => {
            delivery.identity.attempt_id == binding.attempt_id
                && delivery.identity.delegation_id == binding.delegation_id
                && *lease_epoch == binding.lease_epoch
        }
        WorkerTaskEvent::Terminal {
            attempt_id,
            lease_epoch,
            ..
        } => attempt_id == &binding.attempt_id && *lease_epoch == binding.lease_epoch,
    }
}

/// Synchronous scheduler adapter that enforces Main-Agent bindings before
/// delegating to the async launcher.  Bindings are per attempt and cannot be
/// replaced with a different lease/capability for the same attempt.
pub struct BoundWorkerLauncher<L> {
    launcher: L,
    bindings: HashMap<String, WorkerAttemptBinding>,
}

impl<L> BoundWorkerLauncher<L> {
    pub fn new(launcher: L) -> Self {
        Self {
            launcher,
            bindings: HashMap::new(),
        }
    }

    pub fn bind(&mut self, binding: WorkerAttemptBinding) -> Result<(), WorkerLaunchError> {
        if binding.attempt_id.trim().is_empty() || binding.delegation_id.trim().is_empty() {
            return Err(WorkerLaunchError::InvalidBinding);
        }
        if let Some(existing) = self.bindings.get(&binding.attempt_id) {
            if existing == &binding {
                return Ok(());
            }
            return Err(WorkerLaunchError::DuplicateBinding);
        }
        self.bindings.insert(binding.attempt_id.clone(), binding);
        Ok(())
    }

    pub fn launcher(&self) -> &L {
        &self.launcher
    }

    pub fn launcher_mut(&mut self) -> &mut L {
        &mut self.launcher
    }
}

impl<L> WorkerAdapter for BoundWorkerLauncher<L>
where
    L: DelegatedWorkerLauncher,
{
    fn start(&mut self, dispatch: WorkerDispatch) -> Result<(), String> {
        let binding = self
            .bindings
            .get(&dispatch.attempt_id)
            .cloned()
            .ok_or_else(|| WorkerLaunchError::MissingBinding(dispatch.attempt_id.clone()))
            .map_err(|error| format!("{error:?}"))?;
        binding
            .validate_for(&dispatch)
            .map_err(|error| format!("{error:?}"))?;
        self.launcher
            .launch(WorkerLaunchRequest {
                dispatch,
                binding,
                plan: None,
            })
            .map_err(|error| format!("{error:?}"))
    }

    fn stop(&mut self, attempt_id: &str, reason: StopReason) -> Result<(), String> {
        self.launcher
            .cancel(attempt_id, reason)
            .map_err(|error| format!("{error:?}"))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::agent::delegation_contract::{
        CandidateArtifact, Confidence, DelegationIdentity, KeyFact, Milestone, UsageFlags,
        VerificationRecord, VerificationStatus,
    };

    #[derive(Default)]
    struct FakeLauncher {
        launches: Vec<WorkerLaunchRequest>,
        cancellations: Vec<(String, StopReason)>,
    }

    impl DelegatedWorkerLauncher for FakeLauncher {
        fn launch(&mut self, request: WorkerLaunchRequest) -> Result<(), WorkerLaunchError> {
            self.launches.push(request);
            Ok(())
        }

        fn cancel(
            &mut self,
            attempt_id: &str,
            reason: StopReason,
        ) -> Result<(), WorkerLaunchError> {
            self.cancellations.push((attempt_id.into(), reason));
            Ok(())
        }
    }

    fn dispatch() -> WorkerDispatch {
        WorkerDispatch {
            outbox_id: "out-1".into(),
            delegation_id: "delegation-1".into(),
            attempt_id: "attempt-1".into(),
            payload_json: "{}".into(),
            epoch: 7,
        }
    }

    fn binding() -> WorkerAttemptBinding {
        WorkerAttemptBinding {
            attempt_id: "attempt-1".into(),
            delegation_id: "delegation-1".into(),
            work_package_id: "package-1".into(),
            lease_epoch: 7,
            capability_ref: "capability://opaque".into(),
            sandbox_ref: "sandbox://opaque".into(),
        }
    }

    #[test]
    fn unbound_dispatch_fails_closed_and_bound_dispatch_is_handed_off() {
        let mut adapter = BoundWorkerLauncher::new(FakeLauncher::default());
        assert!(adapter.start(dispatch()).is_err());
        adapter.bind(binding()).unwrap();
        adapter.start(dispatch()).unwrap();
        assert_eq!(adapter.launcher().launches.len(), 1);
        assert_eq!(
            adapter.launcher().launches[0].binding.capability_ref,
            "capability://opaque"
        );
    }

    #[test]
    fn binding_cannot_drift_or_change_lease_epoch() {
        let mut adapter = BoundWorkerLauncher::new(FakeLauncher::default());
        adapter.bind(binding()).unwrap();
        assert_eq!(adapter.bind(binding()), Ok(()));
        let mut changed = binding();
        changed.lease_epoch = 8;
        assert_eq!(
            adapter.bind(changed),
            Err(WorkerLaunchError::DuplicateBinding)
        );
        let mut dispatch = dispatch();
        dispatch.epoch = 8;
        assert!(adapter.start(dispatch).is_err());
    }

    #[test]
    fn cancellation_is_forwarded_for_terminal_cleanup_and_expiry() {
        let mut adapter = BoundWorkerLauncher::new(FakeLauncher::default());
        adapter.stop("attempt-1", StopReason::LeaseExpired).unwrap();
        assert_eq!(
            adapter.launcher().cancellations,
            vec![("attempt-1".into(), StopReason::LeaseExpired)]
        );
    }

    #[derive(Default)]
    struct AsyncFakeLauncher {
        launches: Vec<WorkerLaunchRequest>,
    }

    struct FakeTask {
        attempt_id: String,
        events: VecDeque<WorkerTaskEvent>,
    }

    impl WorkerTask for FakeTask {
        fn poll(&mut self) -> Result<Option<WorkerTaskEvent>, WorkerLaunchError> {
            Ok(self.events.pop_front())
        }

        fn cancel(&mut self, reason: StopReason) -> Result<(), WorkerLaunchError> {
            self.events.clear();
            self.events.push_back(WorkerTaskEvent::Terminal {
                attempt_id: self.attempt_id.clone(),
                lease_epoch: 7,
                reason: format!("cancelled:{reason:?}"),
            });
            Ok(())
        }

        fn join(&mut self) -> Result<Vec<WorkerTaskEvent>, WorkerLaunchError> {
            Ok(self.events.drain(..).collect())
        }
    }

    impl AsyncWorkerLauncher for AsyncFakeLauncher {
        fn launch(
            &mut self,
            request: WorkerLaunchRequest,
        ) -> Result<Box<dyn WorkerTask>, WorkerLaunchError> {
            let attempt_id = request.dispatch.attempt_id.clone();
            self.launches.push(request);
            Ok(Box::new(FakeTask {
                attempt_id,
                events: VecDeque::from([
                    WorkerTaskEvent::Heartbeat(Heartbeat {
                        attempt_id: "attempt-1".into(),
                        epoch: 7,
                        stage: "model".into(),
                        progress_percent: 20,
                        budget_remaining: "test".into(),
                        evidence_refs: vec![],
                    }),
                    WorkerTaskEvent::Delivery {
                        delivery: delivery(),
                        lease_epoch: 7,
                    },
                    WorkerTaskEvent::Terminal {
                        attempt_id: "attempt-1".into(),
                        lease_epoch: 7,
                        reason: "completed".into(),
                    },
                ]),
            }))
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
            status: crate::agent::delegation_contract::DeliveryStatus::Completed,
            executive_summary: "structured result".into(),
            key_facts: vec![KeyFact {
                statement: "fact".into(),
                confidence: Confidence::High,
                evidence_refs: vec![],
            }],
            milestones: vec![Milestone {
                label: "done".into(),
                outcome: "done".into(),
                evidence_refs: vec![],
            }],
            verifications: vec![VerificationRecord {
                item: "tests".into(),
                method: "fake".into(),
                status: VerificationStatus::Passed,
                conclusion: "ok".into(),
                evidence_ref: "evidence://test".into(),
            }],
            candidate_artifacts: vec![CandidateArtifact {
                kind: "file".into(),
                relative_ref: "result.txt".into(),
                description: "result".into(),
                evidence_ref: None,
            }],
            open_questions: vec![],
            risks: vec![],
            evidence_refs: vec!["evidence://test".into()],
            usage: UsageFlags {
                context_truncated: false,
                output_truncated: false,
                budget_exhausted: false,
            },
        }
    }

    #[test]
    fn async_bridge_routes_heartbeat_delivery_and_terminal_once() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let callback: WorkerEventCallback = Box::new(move |event| {
            captured.lock().unwrap().push(event);
            Ok(())
        });
        let mut bridge = AsyncWorkerLauncherBridge::new(AsyncFakeLauncher::default(), callback);
        bridge.bind(binding()).unwrap();
        WorkerAdapter::start(&mut bridge, dispatch()).unwrap();
        for _ in 0..3 {
            bridge.poll().unwrap();
        }
        assert_eq!(bridge.active_attempts(), 0);
        let events = events.lock().unwrap();
        assert!(matches!(events[0], WorkerTaskEvent::Heartbeat(_)));
        assert!(matches!(events[1], WorkerTaskEvent::Delivery { .. }));
        assert!(matches!(events[2], WorkerTaskEvent::Terminal { .. }));
        bridge.stop("attempt-1", StopReason::LeaseExpired).unwrap();
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn cancellation_join_is_idempotent_and_cannot_reenter_terminal_callback() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let callback: WorkerEventCallback = Box::new(move |event| {
            captured.lock().unwrap().push(event);
            Ok(())
        });
        let mut bridge = AsyncWorkerLauncherBridge::new(AsyncFakeLauncher::default(), callback);
        bridge.bind(binding()).unwrap();
        WorkerAdapter::start(&mut bridge, dispatch()).unwrap();
        bridge.stop("attempt-1", StopReason::LeaseExpired).unwrap();
        bridge.stop("attempt-1", StopReason::LeaseExpired).unwrap();
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], WorkerTaskEvent::Terminal { .. }));
    }

    struct StaleLauncher;
    impl AsyncWorkerLauncher for StaleLauncher {
        fn launch(
            &mut self,
            _: WorkerLaunchRequest,
        ) -> Result<Box<dyn WorkerTask>, WorkerLaunchError> {
            Ok(Box::new(FakeTask {
                attempt_id: "attempt-1".into(),
                events: VecDeque::from([
                    WorkerTaskEvent::Heartbeat(Heartbeat {
                        attempt_id: "attempt-1".into(),
                        epoch: 99,
                        stage: "stale".into(),
                        progress_percent: 100,
                        budget_remaining: "stale".into(),
                        evidence_refs: vec![],
                    }),
                    WorkerTaskEvent::Terminal {
                        attempt_id: "attempt-1".into(),
                        lease_epoch: 99,
                        reason: "stale".into(),
                    },
                ]),
            }))
        }
    }

    #[test]
    fn stale_epoch_callbacks_are_dropped_before_inbox_boundary() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let callback: WorkerEventCallback = Box::new(move |event| {
            captured.lock().unwrap().push(event);
            Ok(())
        });
        let mut bridge = AsyncWorkerLauncherBridge::new(StaleLauncher, callback);
        bridge.bind(binding()).unwrap();
        WorkerAdapter::start(&mut bridge, dispatch()).unwrap();
        bridge.poll().unwrap();
        bridge.poll().unwrap();
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(bridge.active_attempts(), 1);
        bridge.stop("attempt-1", StopReason::LeaseExpired).unwrap();
        assert_eq!(events.lock().unwrap().len(), 1);
    }
}
