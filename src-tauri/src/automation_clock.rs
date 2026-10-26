//! One desktop-owned clock for the existing durable automation dispatcher.
//! Webview reloads/focus do not own its lifetime. This is not another scheduler.

use std::{future::Future, time::Duration};
use tauri::{AppHandle, Manager};
use tokio::sync::watch;

const SCAN_INTERVAL: Duration = Duration::from_secs(60);

pub(crate) struct AutomationClock {
    shutdown: watch::Sender<bool>,
}

impl AutomationClock {
    pub(crate) fn install(app: &AppHandle) {
        if app.try_state::<Self>().is_some() {
            return;
        }
        let (shutdown, receiver) = watch::channel(false);
        let handle = app.clone();
        tauri::async_runtime::spawn(run_clock(SCAN_INTERVAL, receiver, move || {
            crate::commands::automation::run_due_automations(handle.clone())
        }));
        app.manage(Self { shutdown });
    }

    /// Prevent further scans; never cancel an in-flight external action or
    /// pretend that dropping its future would undo its effects.
    pub(crate) fn stop(&self) {
        let _ = self.shutdown.send(true);
    }
}

async fn run_clock<F, Scan>(period: Duration, mut shutdown: watch::Receiver<bool>, mut scan: F)
where
    F: FnMut() -> Scan,
    Scan: Future<Output = Result<usize, String>>,
{
    let mut ticks = tokio::time::interval(period);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            _ = ticks.tick() => {}
        }
        if *shutdown.borrow() {
            break;
        }
        // Serial scans. Durable claims, authorization, recovery, dispatch and
        // OS notification receipts remain in the existing implementation.
        if scan().await.is_err() {
            eprintln!("[Automation] Due scan failed; check automation results. No completion is inferred.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::Notify;

    #[tokio::test]
    async fn stop_before_start_does_not_dispatch() {
        let (_shutdown, receiver) = watch::channel(true);
        run_clock(Duration::from_millis(5), receiver, || async {
            panic!("stopped clock must not scan")
        })
        .await;
    }

    #[tokio::test]
    async fn serial_scans_finish_inflight_work_before_stop_without_catchup_replay() {
        let (shutdown, receiver) = watch::channel(false);
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(run_clock(Duration::from_millis(5), receiver, {
            let (started, release, calls, finished) = (
                started.clone(),
                release.clone(),
                calls.clone(),
                finished.clone(),
            );
            move || {
                let (started, release, calls, finished) = (
                    started.clone(),
                    release.clone(),
                    calls.clone(),
                    finished.clone(),
                );
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    release.notified().await;
                    finished.fetch_add(1, Ordering::SeqCst);
                    Ok(0)
                }
            }
        }));
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        // Several deadlines pass while the same real scan remains in flight.
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        shutdown.send(true).unwrap();
        assert_eq!(finished.load(Ordering::SeqCst), 0);
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(finished.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_scan_does_not_kill_the_clock_or_claim_success() {
        let (shutdown, receiver) = watch::channel(false);
        let mut calls = 0;
        tokio::time::timeout(
            Duration::from_secs(2),
            run_clock(Duration::from_millis(5), receiver, || {
                calls += 1;
                let first = calls == 1;
                if !first {
                    shutdown.send(true).unwrap();
                }
                async move {
                    if first {
                        Err("synthetic scan failure".into())
                    } else {
                        Ok(0)
                    }
                }
            }),
        )
        .await
        .unwrap();
        assert_eq!(calls, 2);
    }
}
