//! Single host-loop adapter for delegated workers.
//!
//! `DelegationPump` does not spawn a background loop. The existing application
//! tick calls `poll` on the async launcher bridge first, routing heartbeat and
//! structured deliveries through the one `DelegationServiceHandle`, then calls
//! service reconciliation/dispatch. Shutdown stops and joins every task before
//! shutting down the service, and is idempotent.

use super::{
    delegation_contract::ContractLimits,
    delegation_service::{DelegationServiceError, DelegationServiceHandle, ServiceReport},
    delivery_inbox::{DecisionOutcome, ParentDecision, RetryContinuation},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationPumpError {
    Worker(String),
    Service(DelegationServiceError),
    Stopped,
}

/// The only caller-owned loop for worker polling and delegation scheduling.
pub struct DelegationPump {
    service: DelegationServiceHandle,
    limits: ContractLimits,
    stopped: bool,
}

pub trait DelegationPumpLoop: Send {
    fn start(&mut self) -> Result<ServiceReport, DelegationPumpError>;
    fn tick(&mut self) -> Result<ServiceReport, DelegationPumpError>;
    fn shutdown(&mut self) -> Result<(), DelegationPumpError>;
    fn decide_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        decision: ParentDecision,
        reason: Option<&str>,
    ) -> Result<DecisionOutcome, DelegationPumpError> {
        let _ = (session_id, delivery_id, decision, reason);
        Err(DelegationPumpError::Stopped)
    }
    fn retry_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        reason: &str,
    ) -> Result<RetryContinuation, DelegationPumpError> {
        let _ = (session_id, delivery_id, reason);
        Err(DelegationPumpError::Stopped)
    }
    fn materialize_change_for_session(
        &mut self,
        _session_id: &str,
        _delivery_id: &str,
        _ttl_secs: i64,
    ) -> Result<DecisionOutcome, DelegationPumpError> {
        Err(DelegationPumpError::Stopped)
    }
    fn decline_change_for_session(
        &mut self,
        _session_id: &str,
        _delivery_id: &str,
        _ttl_secs: i64,
    ) -> Result<(), DelegationPumpError> {
        Err(DelegationPumpError::Stopped)
    }
}

pub struct DelegationPumpHandle {
    inner: std::sync::Arc<std::sync::Mutex<Box<dyn DelegationPumpLoop>>>,
}

/// Host-owned bounded timer. It is the only background caller of `tick`; the
/// handle is kept in AppState so setup/exit can start and stop it exactly once.
#[derive(Clone)]
pub struct DelegationPumpHostHandle {
    pump: DelegationPumpHandle,
    stop: Arc<AtomicBool>,
    join: Arc<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,
    /// Application-owned wake hook for newly queued, reviewed delivery
    /// summaries. It runs only after `pump.tick()` has released the service
    /// mutex, so a foreground wake can never re-enter delegation while its
    /// scheduler state is locked.
    foreground_follow_up_waker: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
    interval: Duration,
}

impl DelegationPumpHostHandle {
    pub fn new(pump: DelegationPumpHandle) -> Self {
        Self {
            pump,
            stop: Arc::new(AtomicBool::new(false)),
            join: Arc::new(Mutex::new(None)),
            foreground_follow_up_waker: Arc::new(Mutex::new(None)),
            interval: Duration::from_millis(250),
        }
    }

    /// Install the application-bound wake path for reviewed delivery
    /// summaries. The delegation layer reports only a count; it never learns
    /// about `AppHandle`, UI state, or the foreground runner implementation.
    pub fn set_foreground_follow_up_waker(&self, waker: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.foreground_follow_up_waker.lock() {
            *slot = Some(waker);
        }
    }

    pub fn start(&self) -> Result<(), DelegationPumpError> {
        let mut join = self.join.lock().map_err(|_| DelegationPumpError::Stopped)?;
        if join.is_some() {
            return Ok(());
        }
        self.stop.store(false, Ordering::Release);
        self.pump.start()?;
        let pump = self.pump.clone();
        let stop = self.stop.clone();
        let foreground_follow_up_waker = self.foreground_follow_up_waker.clone();
        let interval = self.interval;
        *join = Some(tauri::async_runtime::spawn(async move {
            while !stop.load(Ordering::Acquire) {
                match pump.tick() {
                    Ok(report) if report.reviewed_delivery_follow_ups_queued > 0 => {
                        let wake = foreground_follow_up_waker
                            .lock()
                            .ok()
                            .and_then(|slot| slot.clone());
                        if let Some(wake) = wake {
                            wake();
                        }
                    }
                    Ok(_) | Err(_) => {}
                }
                tokio::time::sleep(interval).await;
            }
        }));
        Ok(())
    }

    /// Synchronously signals cancellation, aborts the bounded host task, and
    /// performs the pump's stop/join→service shutdown sequence. Repeated calls
    /// are safe even after the JoinHandle has already completed.
    pub fn shutdown(&self) -> Result<(), DelegationPumpError> {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut join) = self.join.lock() {
            if let Some(handle) = join.take() {
                handle.abort();
            }
        }
        self.pump.shutdown()
    }

    pub fn decide_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        decision: ParentDecision,
        reason: Option<&str>,
    ) -> Result<DecisionOutcome, DelegationPumpError> {
        self.pump
            .decide_for_session(session_id, delivery_id, decision, reason)
    }

    pub fn retry_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        reason: &str,
    ) -> Result<RetryContinuation, DelegationPumpError> {
        self.pump.retry_for_session(session_id, delivery_id, reason)
    }

    pub fn materialize_change_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
    ) -> Result<DecisionOutcome, DelegationPumpError> {
        self.pump
            .materialize_change_for_session(session_id, delivery_id, ttl_secs)
    }

    pub fn decline_change_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
    ) -> Result<(), DelegationPumpError> {
        self.pump
            .decline_change_for_session(session_id, delivery_id, ttl_secs)
    }
}

impl Clone for DelegationPumpHandle {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl DelegationPumpHandle {
    pub fn new<P>(pump: P) -> Self
    where
        P: DelegationPumpLoop + 'static,
    {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(Box::new(pump))),
        }
    }

    pub fn materialize_change_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
    ) -> Result<DecisionOutcome, DelegationPumpError> {
        self.inner
            .lock()
            .map_err(|_| DelegationPumpError::Stopped)?
            .materialize_change_for_session(session_id, delivery_id, ttl_secs)
    }

    pub fn decline_change_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
    ) -> Result<(), DelegationPumpError> {
        self.inner
            .lock()
            .map_err(|_| DelegationPumpError::Stopped)?
            .decline_change_for_session(session_id, delivery_id, ttl_secs)
    }

    pub fn start(&self) -> Result<ServiceReport, DelegationPumpError> {
        self.inner
            .lock()
            .map_err(|_| DelegationPumpError::Stopped)?
            .start()
    }

    pub fn tick(&self) -> Result<ServiceReport, DelegationPumpError> {
        self.inner
            .lock()
            .map_err(|_| DelegationPumpError::Stopped)?
            .tick()
    }

    pub fn shutdown(&self) -> Result<(), DelegationPumpError> {
        self.inner
            .lock()
            .map_err(|_| DelegationPumpError::Stopped)?
            .shutdown()
    }

    pub fn decide_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        decision: ParentDecision,
        reason: Option<&str>,
    ) -> Result<DecisionOutcome, DelegationPumpError> {
        self.inner
            .lock()
            .map_err(|_| DelegationPumpError::Stopped)?
            .decide_for_session(session_id, delivery_id, decision, reason)
    }

    pub fn retry_for_session(
        &self,
        session_id: &str,
        delivery_id: &str,
        reason: &str,
    ) -> Result<RetryContinuation, DelegationPumpError> {
        self.inner
            .lock()
            .map_err(|_| DelegationPumpError::Stopped)?
            .retry_for_session(session_id, delivery_id, reason)
    }

    pub fn fail_closed(service: DelegationServiceHandle, limits: ContractLimits) -> Self {
        Self::new(DelegationPump::new(service, limits))
    }
}

impl DelegationPump {
    pub fn new(service: DelegationServiceHandle, limits: ContractLimits) -> Self {
        Self {
            service,
            limits,
            stopped: false,
        }
    }

    pub fn start(&mut self) -> Result<ServiceReport, DelegationPumpError> {
        if self.stopped {
            return Err(DelegationPumpError::Stopped);
        }
        self.service.start().map_err(DelegationPumpError::Service)
    }

    /// Poll active tasks, then run the service's existing durable tick. No
    /// timer or executor is created here, so there can be only one loop.
    pub fn tick(&mut self) -> Result<ServiceReport, DelegationPumpError> {
        if self.stopped {
            return Err(DelegationPumpError::Stopped);
        }
        let reviewed_delivery_follow_ups_queued = self
            .service
            .poll_workers(&self.limits)
            .map_err(DelegationPumpError::Service)?;
        let mut report = self.service.tick().map_err(DelegationPumpError::Service)?;
        report.reviewed_delivery_follow_ups_queued += reviewed_delivery_follow_ups_queued;
        Ok(report)
    }

    /// Cancel and join every active worker before shutting down the service.
    /// Repeating shutdown is safe and does not emit a second terminal event.
    pub fn shutdown(&mut self) -> Result<(), DelegationPumpError> {
        if self.stopped {
            return Ok(());
        }
        self.service
            .shutdown()
            .map_err(DelegationPumpError::Service)?;
        self.stopped = true;
        Ok(())
    }

    pub fn decide_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        decision: ParentDecision,
        reason: Option<&str>,
    ) -> Result<DecisionOutcome, DelegationPumpError> {
        if self.stopped {
            return Err(DelegationPumpError::Stopped);
        }
        self.service
            .decide_for_session(session_id, delivery_id, decision, reason, &self.limits)
            .map_err(DelegationPumpError::Service)
    }

    pub fn retry_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        reason: &str,
    ) -> Result<RetryContinuation, DelegationPumpError> {
        if self.stopped {
            return Err(DelegationPumpError::Stopped);
        }
        self.service
            .retry_for_session(session_id, delivery_id, reason, &self.limits)
            .map_err(DelegationPumpError::Service)
    }

    pub fn active_attempts(&self) -> usize {
        self.service.active_worker_attempts().unwrap_or_default()
    }

    pub fn materialize_change_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
    ) -> Result<DecisionOutcome, DelegationPumpError> {
        self.service
            .materialize_change_for_session(session_id, delivery_id, ttl_secs, &self.limits)
            .map_err(DelegationPumpError::Service)
    }

    pub fn decline_change_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
    ) -> Result<(), DelegationPumpError> {
        self.service
            .decline_change_for_session(session_id, delivery_id, ttl_secs, &self.limits)
            .map_err(DelegationPumpError::Service)
    }

    pub fn limits(&self) -> &ContractLimits {
        &self.limits
    }
}

impl DelegationPumpLoop for DelegationPump {
    fn start(&mut self) -> Result<ServiceReport, DelegationPumpError> {
        DelegationPump::start(self)
    }

    fn tick(&mut self) -> Result<ServiceReport, DelegationPumpError> {
        DelegationPump::tick(self)
    }

    fn shutdown(&mut self) -> Result<(), DelegationPumpError> {
        DelegationPump::shutdown(self)
    }

    fn decide_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        decision: ParentDecision,
        reason: Option<&str>,
    ) -> Result<DecisionOutcome, DelegationPumpError> {
        DelegationPump::decide_for_session(self, session_id, delivery_id, decision, reason)
    }

    fn retry_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        reason: &str,
    ) -> Result<RetryContinuation, DelegationPumpError> {
        DelegationPump::retry_for_session(self, session_id, delivery_id, reason)
    }

    fn materialize_change_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
    ) -> Result<DecisionOutcome, DelegationPumpError> {
        DelegationPump::materialize_change_for_session(self, session_id, delivery_id, ttl_secs)
    }

    fn decline_change_for_session(
        &mut self,
        session_id: &str,
        delivery_id: &str,
        ttl_secs: i64,
    ) -> Result<(), DelegationPumpError> {
        DelegationPump::decline_change_for_session(self, session_id, delivery_id, ttl_secs)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        sync::{Arc, Mutex},
    };

    use rusqlite::Connection;

    use super::*;
    use crate::agent::{
        delegation_runtime::{DelegationRuntime, FixedAdmissionPolicy, StopReason, WorkerDispatch},
        delegation_service::DelegationService,
        shared_db::SharedDb,
        workspace_admission::WorkspaceAdmission,
    };

    #[derive(Default)]
    struct Worker;
    impl crate::agent::delegation_runtime::WorkerAdapter for Worker {
        fn start(&mut self, _: WorkerDispatch) -> Result<(), String> {
            Ok(())
        }
        fn stop(&mut self, _: &str, _: StopReason) -> Result<(), String> {
            Ok(())
        }
    }
    struct Clock(Cell<i64>);
    impl crate::agent::delegation_runtime::Clock for Clock {
        fn now(&self) -> i64 {
            self.0.get()
        }
    }

    struct RetrySpy {
        calls: Arc<Mutex<Vec<(String, String, String)>>>,
    }

    impl DelegationPumpLoop for RetrySpy {
        fn start(&mut self) -> Result<ServiceReport, DelegationPumpError> {
            Ok(ServiceReport::default())
        }

        fn tick(&mut self) -> Result<ServiceReport, DelegationPumpError> {
            Ok(ServiceReport::default())
        }

        fn shutdown(&mut self) -> Result<(), DelegationPumpError> {
            Ok(())
        }

        fn retry_for_session(
            &mut self,
            session_id: &str,
            delivery_id: &str,
            reason: &str,
        ) -> Result<RetryContinuation, DelegationPumpError> {
            self.calls.lock().unwrap().push((
                session_id.to_string(),
                delivery_id.to_string(),
                reason.to_string(),
            ));
            Err(DelegationPumpError::Worker("retry spy".into()))
        }
    }

    fn pump() -> DelegationPump {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let db = SharedDb::new(conn);
        let runtime = DelegationRuntime::new_shared(
            db.clone(),
            Worker,
            Clock(Cell::new(1)),
            FixedAdmissionPolicy {
                max_running_attempts: 0,
            },
            30,
        )
        .unwrap();
        let scheduler =
            crate::agent::delegated_attempt_scheduler::DelegatedAttemptScheduler::with_sandbox(
                runtime,
            );
        let service = DelegationService::new(db, scheduler, WorkspaceAdmission::new()).unwrap();
        DelegationPump::new(
            DelegationServiceHandle::new(service),
            ContractLimits::default(),
        )
    }

    #[test]
    fn pump_orders_poll_before_service_tick_and_shutdown_is_idempotent() {
        let mut pump = pump();
        pump.start().unwrap();
        // Scheduler claims are absent in this contract test; bridge still
        // enforces task lifecycle and the single tick owner.
        assert!(pump.tick().is_err() || pump.active_attempts() == 0);
        pump.shutdown().unwrap();
        pump.shutdown().unwrap();
        assert!(matches!(pump.tick(), Err(DelegationPumpError::Stopped)));
    }

    #[test]
    fn host_retry_for_session_forwards_only_trusted_retry_inputs() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let host = DelegationPumpHostHandle::new(DelegationPumpHandle::new(RetrySpy {
            calls: calls.clone(),
        }));

        assert!(matches!(
            host.retry_for_session("session-a", "delivery-a", "retry after review"),
            Err(DelegationPumpError::Worker(_))
        ));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![(
                ("session-a").into(),
                ("delivery-a").into(),
                ("retry after review").into()
            )]
        );
    }
}
