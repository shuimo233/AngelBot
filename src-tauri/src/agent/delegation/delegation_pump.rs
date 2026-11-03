//! Host-owned timer for delegated workers.
//!
//! Main-Agent controls call the same DelegationServiceHandle directly. This
//! adapter owns only polling order, the follow-up wake hook and timer lifetime;
//! it does not proxy delivery decisions or carry a second copy of policy.

use super::delegation_service::{DelegationServiceError, DelegationServiceHandle, ServiceReport};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

/// The only background caller of worker polling and durable reconciliation.
/// AppState starts/stops this host; Main tools receive only its service handle.
#[derive(Clone)]
pub struct DelegationPumpHostHandle {
    service: DelegationServiceHandle,
    stop: Arc<AtomicBool>,
    join: Arc<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>>,
    foreground_follow_up_waker: Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>,
    interval: Duration,
}

impl DelegationPumpHostHandle {
    pub fn new(service: DelegationServiceHandle) -> Self {
        Self {
            service,
            stop: Arc::new(AtomicBool::new(false)),
            join: Arc::new(Mutex::new(None)),
            foreground_follow_up_waker: Arc::new(Mutex::new(None)),
            interval: Duration::from_millis(250),
        }
    }

    /// Controls and polling share one serialized service, not parallel state.
    pub fn service(&self) -> &DelegationServiceHandle {
        &self.service
    }

    /// Runs after tick has released the service mutex. The service knows
    /// neither AppHandle nor foreground runners, and the wake may re-enter it.
    pub fn set_foreground_follow_up_waker(&self, waker: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.foreground_follow_up_waker.lock() {
            *slot = Some(waker);
        }
    }

    pub fn start(&self) -> Result<(), DelegationServiceError> {
        let mut join = self
            .join
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?;
        if join.is_some() {
            return Ok(());
        }
        self.service.start()?;
        self.stop.store(false, Ordering::Release);
        // Do not capture the host: it owns this JoinHandle, so capturing it
        // would form an Arc cycle between the timer and its owner.
        let service = self.service.clone();
        let stop = self.stop.clone();
        let waker = self.foreground_follow_up_waker.clone();
        let interval = self.interval;
        *join = Some(tauri::async_runtime::spawn(async move {
            while !stop.load(Ordering::Acquire) {
                if let Ok(report) = poll_and_reconcile(&service) {
                    if report.reviewed_delivery_follow_ups_queued > 0 {
                        let wake = waker.lock().ok().and_then(|slot| slot.clone());
                        if let Some(wake) = wake {
                            wake();
                        }
                    }
                }
                tokio::time::sleep(interval).await;
            }
        }));
        Ok(())
    }

    /// Serialize start/shutdown as well as aborting the timer. The service
    /// cancels and joins workers before shutdown; repeated calls are safe.
    pub fn shutdown(&self) -> Result<(), DelegationServiceError> {
        let mut join = self
            .join
            .lock()
            .map_err(|_| DelegationServiceError::Runtime)?;
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = join.take() {
            handle.abort();
        }
        self.service.shutdown()
    }
}

/// Poll task events before reconciliation/dispatch. Both operations enforce
/// the same service lifecycle; no service lock remains held at the wake hook.
fn poll_and_reconcile(
    service: &DelegationServiceHandle,
) -> Result<ServiceReport, DelegationServiceError> {
    let follow_ups = service.poll_workers()?;
    let mut report = service.tick()?;
    report.reviewed_delivery_follow_ups_queued += follow_ups;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        delegation_contract::ContractLimits,
        delegation_service::test_support::{recording_service, ServiceCall},
        delivery_inbox::ParentDecision,
        shared_db::SharedDb,
    };
    use rusqlite::Connection;

    #[test]
    fn pump_orders_poll_before_tick_and_shares_limits_with_main_controls() {
        let limits = ContractLimits {
            max_summary_bytes: 37,
            ..ContractLimits::default()
        };
        let (service, calls) = recording_service(limits.clone());
        let host = DelegationPumpHostHandle::new(service);
        host.service().start().unwrap();
        assert_eq!(
            poll_and_reconcile(host.service())
                .unwrap()
                .reviewed_delivery_follow_ups_queued,
            2
        );
        assert_eq!(
            host.service()
                .decide_for_session("session", "delivery", ParentDecision::Accept, None),
            Err(DelegationServiceError::Inbox)
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                ServiceCall::Start,
                ServiceCall::Poll(limits.clone()),
                ServiceCall::Tick,
                ServiceCall::Decide(
                    "session".into(),
                    "delivery".into(),
                    ParentDecision::Accept,
                    None,
                    limits
                ),
            ]
        );
    }

    #[test]
    fn host_shutdown_stops_the_same_service_and_cannot_restart_it() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let service = DelegationServiceHandle::fail_closed(SharedDb::new(conn)).unwrap();
        let host = DelegationPumpHostHandle::new(service.clone());
        assert_eq!(
            service.poll_workers(),
            Err(DelegationServiceError::NotStarted)
        );
        host.start().unwrap();
        host.start().unwrap();
        poll_and_reconcile(host.service()).unwrap();
        host.shutdown().unwrap();
        host.shutdown().unwrap();
        assert_eq!(
            poll_and_reconcile(host.service()),
            Err(DelegationServiceError::Stopped)
        );
        assert_eq!(host.start(), Err(DelegationServiceError::Stopped));
        assert_eq!(
            service.decide_for_session("session", "delivery", ParentDecision::Accept, None),
            Err(DelegationServiceError::Stopped)
        );
        assert_eq!(
            service.retry_for_session("session", "delivery", "retry"),
            Err(DelegationServiceError::Stopped)
        );
        assert_eq!(
            service.materialize_change_for_session("session", "delivery", 60),
            Err(DelegationServiceError::Stopped)
        );
        assert_eq!(
            service.decline_change_for_session("session", "delivery", 60),
            Err(DelegationServiceError::Stopped)
        );
        let owner = Arc::downgrade(&host.join);
        drop(host);
        assert!(
            owner.upgrade().is_none(),
            "the timer must not retain its owner"
        );
    }
}
