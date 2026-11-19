//! One desktop-owned clock for the existing durable automation dispatcher.
//! Webview reloads/focus do not own its lifetime. This is not another scheduler.

use rusqlite::Connection;
use std::{future::Future, time::Duration};
use tauri::{AppHandle, Manager};
use tokio::sync::watch;

use crate::runtime_health::{RuntimeHealthState, RuntimeIssue};

const SCAN_INTERVAL: Duration = Duration::from_secs(60);

pub(crate) struct AutomationClock {
    shutdown: watch::Sender<bool>,
}

impl AutomationClock {
    /// Establish the durable startup state before publishing host health. A
    /// failed recovery or startup transaction grants no permission to scan.
    pub(crate) fn prepare(
        conn: &mut Connection,
        recovered: bool,
        os_launch: bool,
        now: i64,
        health: &mut RuntimeHealthState,
    ) -> Option<Self> {
        if !recovered {
            eprintln!("[Automation] Startup recovery incomplete; clock not started");
        } else {
            match crate::commands::automation::prepare_startup_automations(conn, os_launch, now) {
                Ok(()) => {
                    return Some(Self {
                        shutdown: watch::channel(false).0,
                    });
                }
                Err(error) => {
                    eprintln!("[Automation] Startup admission failed; clock not started: {error}");
                }
            }
        }
        health.add(RuntimeIssue::automation_scheduler_unavailable());
        None
    }

    /// Start the prepared clock only after the desktop shell is ready.
    pub(crate) fn install(self, app: &AppHandle) {
        let receiver = self.shutdown.subscribe();
        if app.manage(self) {
            let handle = app.clone();
            tauri::async_runtime::spawn(run_clock(SCAN_INTERVAL, receiver, move || {
                crate::commands::automation::run_due_automations(handle.clone())
            }));
        }
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

    #[test]
    fn failed_startup_transaction_degrades_health_and_an_ordinary_relaunch_discards_the_login_trigger(
    ) {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE automations (trigger_kind TEXT, next_run_at INTEGER, enabled INTEGER, updated_at INTEGER);
                            INSERT INTO automations VALUES ('startup', 10, 1, 1), ('schedule', 200, 0, 1);")
            .unwrap();
        // Missing settings causes a deterministic failure after the first
        // update in the real transaction. Its changes must roll back.
        let startup_due = |conn: &Connection| {
            conn.query_row(
                "SELECT next_run_at FROM automations WHERE trigger_kind='startup'",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .unwrap()
        };
        let mut health = RuntimeHealthState::default();
        assert!(AutomationClock::prepare(&mut conn, true, true, 20, &mut health).is_none());
        assert_eq!(startup_due(&conn), Some(10));
        let issue = serde_json::to_value(health.snapshot(true)).unwrap();
        assert_eq!(issue["status"], "degraded");
        assert_eq!(
            issue["issues"][0]["code"],
            "automation_scheduler_unavailable"
        );
        assert_eq!(issue["issues"][0]["recoveryAction"], "none");

        let mut relaunched_health = RuntimeHealthState::default();
        assert!(
            AutomationClock::prepare(&mut conn, true, false, 30, &mut relaunched_health).is_some()
        );
        assert_eq!(startup_due(&conn), None);
        assert!(relaunched_health.snapshot(true).issues.is_empty());
        assert_eq!(
            conn.query_row(
                "SELECT enabled, next_run_at FROM automations WHERE trigger_kind='schedule'",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            )
            .unwrap(),
            (0, 200)
        );
    }

    #[test]
    fn incomplete_recovery_does_not_arm_a_valid_login_or_change_saved_deadlines() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE automations (trigger_kind TEXT, next_run_at INTEGER, enabled INTEGER, updated_at INTEGER);
                            CREATE TABLE settings (key TEXT, value TEXT);
                            INSERT INTO automations VALUES ('startup', NULL, 1, 1), ('schedule', 20, 1, 1);
                            INSERT INTO settings VALUES ('autostart_enabled', 'true'), ('autostart_launch_args', '--angelbot-autostart');")
            .unwrap();
        let deadlines = |conn: &Connection| {
            conn.prepare("SELECT next_run_at FROM automations ORDER BY trigger_kind")
                .unwrap()
                .query_map([], |row| row.get::<_, Option<i64>>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let mut health = RuntimeHealthState::default();
        assert!(AutomationClock::prepare(&mut conn, false, true, 30, &mut health).is_none());
        assert_eq!(deadlines(&conn), vec![Some(20), None]);
        assert_eq!(
            health.snapshot(true).issues,
            vec![RuntimeIssue::automation_scheduler_unavailable()]
        );

        assert!(AutomationClock::prepare(
            &mut conn,
            true,
            true,
            40,
            &mut RuntimeHealthState::default()
        )
        .is_some());
        assert_eq!(deadlines(&conn), vec![Some(20), Some(40)]);
    }

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
