//! Durable scheduler for delegated attempts.
//!
//! This is intentionally only the small control loop around
//! [`DelegationRuntime`].  It does not own a model, user confirmation,
//! materialization, or any child-delegation operation.  The runtime remains
//! the only place that claims an outbox row, changes attempt state, revokes a
//! lease, and seals a sandbox.

use super::{
    delegation_contract::{ContractLimits, DelegationDelivery},
    delegation_runtime::{
        AdmissionPolicy, Clock, DelegationRuntime, DelegationRuntimeError, Heartbeat,
        NoopSandboxController, RecoveryReport, SandboxController, WorkerAdapter,
    },
};

/// The durable outcome of a single scheduler pass.  It is a structured
/// projection for the Main Agent, not a user-facing activity stream.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SchedulerReport {
    pub recovered_unknown_effects: Vec<String>,
    pub policy_ineligible: Vec<String>,
    pub expired: Vec<String>,
    pub dispatched: Vec<String>,
}

/// A narrow error surface: details remain in durable attempt events and are
/// never exposed as raw worker/model output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegatedSchedulerError {
    Runtime,
    Storage,
}

/// A persistent, fail-closed scheduler over the existing delegation runtime.
///
/// `W` is the injected worker-launch port. Production wiring may adapt it to
/// `DelegatedWorkerAdapter`; this module deliberately does not construct a
/// model host or application-wide background executor.
pub struct DelegatedAttemptScheduler<W, C, P, S = NoopSandboxController> {
    runtime: DelegationRuntime<W, C, P, S>,
    recovered: bool,
}

impl<W, C, P> DelegatedAttemptScheduler<W, C, P, NoopSandboxController>
where
    W: WorkerAdapter,
    C: Clock,
    P: AdmissionPolicy,
{
    pub fn new(runtime: DelegationRuntime<W, C, P>) -> Self {
        Self {
            runtime,
            recovered: false,
        }
    }
}

impl<W, C, P, S> DelegatedAttemptScheduler<W, C, P, S>
where
    W: WorkerAdapter,
    C: Clock,
    P: AdmissionPolicy,
    S: SandboxController,
{
    pub fn with_sandbox(runtime: DelegationRuntime<W, C, P, S>) -> Self {
        Self {
            runtime,
            recovered: false,
        }
    }

    pub fn runtime(&self) -> &DelegationRuntime<W, C, P, S> {
        &self.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut DelegationRuntime<W, C, P, S> {
        &mut self.runtime
    }

    /// Start is intentionally idempotent.  Any attempt that was already
    /// running at process loss is sealed rather than replayed.
    pub fn start(&mut self) -> Result<SchedulerReport, DelegatedSchedulerError> {
        if self.recovered {
            return Ok(SchedulerReport::default());
        }
        let RecoveryReport {
            sealed_unknown_effect_attempts,
        } = self.runtime.recover().map_err(runtime_error)?;
        self.recovered = true;
        Ok(SchedulerReport {
            recovered_unknown_effects: sealed_unknown_effect_attempts,
            ..SchedulerReport::default()
        })
    }

    /// Reconcile durable policy before each launch and then let the runtime
    /// atomically claim at most the configured admission capacity.  Repeating
    /// this pass is safe: a claimed outbox row is never replayed.
    pub fn tick(&mut self) -> Result<SchedulerReport, DelegatedSchedulerError> {
        let mut report = self.start()?;
        report.policy_ineligible = self.seal_non_active_packages()?;
        report.expired = self.seal_non_running_leases()?;
        for id in self.runtime.expire_leases().map_err(runtime_error)? {
            if !report.expired.contains(&id) {
                report.expired.push(id);
            }
        }

        // `dispatch_one` performs the authoritative outbox claim and global
        // admission check. It returns `None` once capacity or work is absent.
        while let Some(dispatch) = self.runtime.dispatch_one().map_err(runtime_error)? {
            report.dispatched.push(dispatch.attempt_id);
        }
        Ok(report)
    }

    /// Forward worker events only through the runtime's durable inbox.  These
    /// operations neither accept a delivery nor publish a user-visible result.
    pub fn heartbeat(&mut self, heartbeat: Heartbeat) -> Result<(), DelegatedSchedulerError> {
        self.runtime.heartbeat(heartbeat).map_err(runtime_error)
    }

    pub fn submit_delivery(
        &mut self,
        delivery: &DelegationDelivery,
        limits: &ContractLimits,
    ) -> Result<(), DelegatedSchedulerError> {
        self.runtime
            .submit_delivery(delivery, limits)
            .map_err(runtime_error)
    }

    /// A worker may report only a safe, classified failure.  Raw worker output
    /// is not accepted here; the runtime durably revokes and seals the attempt.
    pub fn report_failure(&mut self, attempt_id: &str) -> Result<(), DelegatedSchedulerError> {
        self.runtime
            .needs_decision(attempt_id)
            .map_err(runtime_error)
    }

    fn seal_non_active_packages(&mut self) -> Result<Vec<String>, DelegatedSchedulerError> {
        let ids = self.ids_for(
            "a.status IN ('queued', 'running')\
             AND (wp.id IS NULL OR wp.status <> 'active')\
             AND ?1 = ?1",
        )?;
        for id in &ids {
            self.runtime.needs_decision(id).map_err(runtime_error)?;
        }
        Ok(ids)
    }

    fn seal_non_running_leases(&mut self) -> Result<Vec<String>, DelegatedSchedulerError> {
        let ids = self.ids_for(
            "a.status = 'queued'\
             AND (l.status <> 'active' OR l.expires_at <= ?1)",
        )?;
        for id in &ids {
            self.runtime.needs_decision(id).map_err(runtime_error)?;
        }
        Ok(ids)
    }

    fn ids_for(&self, predicate: &str) -> Result<Vec<String>, DelegatedSchedulerError> {
        // A work package link is mandatory for scheduler-owned execution.  A
        // missing link is deliberately policy-ineligible, never a fallback to
        // the old unscoped dispatch path.
        let sql = format!(
            "SELECT a.id
             FROM delegation_attempts a
             JOIN delegations d ON d.id = a.delegation_id
             LEFT JOIN work_packages wp ON wp.id = d.work_package_id
             JOIN delegation_capability_leases l ON l.attempt_id = a.id
             WHERE {predicate}
             ORDER BY a.created_at ASC"
        );
        let now = self.runtime.now();
        let conn = self.runtime.connection();
        let mut statement = conn
            .prepare(&sql)
            .map_err(|_| DelegatedSchedulerError::Storage)?;
        let rows = statement
            .query_map([now], |row| row.get::<_, String>(0))
            .map_err(|_| DelegatedSchedulerError::Storage)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| DelegatedSchedulerError::Storage)
    }
}

fn runtime_error(_: DelegationRuntimeError) -> DelegatedSchedulerError {
    DelegatedSchedulerError::Runtime
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use rusqlite::{params, Connection};

    use super::*;
    use crate::agent::{
        delegation::{DelegationRepository, NewAttempt, NewCapabilityLease, NewDelegation},
        delegation_contract::{DelegationBrief, DelegationIdentity},
        delegation_runtime::{FixedAdmissionPolicy, StopReason, WorkerDispatch},
    };

    #[derive(Default)]
    struct FakeWorker {
        starts: Vec<WorkerDispatch>,
        stops: Vec<String>,
    }
    impl WorkerAdapter for FakeWorker {
        fn start(&mut self, dispatch: WorkerDispatch) -> Result<(), String> {
            self.starts.push(dispatch);
            Ok(())
        }
        fn stop(&mut self, attempt_id: &str, _: StopReason) -> Result<(), String> {
            self.stops.push(attempt_id.into());
            Ok(())
        }
    }
    struct FakeClock(Cell<i64>);
    impl Clock for FakeClock {
        fn now(&self) -> i64 {
            self.0.get()
        }
    }

    fn scheduler(
        max_running: usize,
    ) -> DelegatedAttemptScheduler<FakeWorker, FakeClock, FixedAdmissionPolicy> {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        let runtime = DelegationRuntime::new(
            conn,
            FakeWorker::default(),
            FakeClock(Cell::new(10)),
            FixedAdmissionPolicy {
                max_running_attempts: max_running,
            },
            30,
        )
        .unwrap();
        DelegatedAttemptScheduler::new(runtime)
    }

    fn seed(
        scheduler: &mut DelegatedAttemptScheduler<FakeWorker, FakeClock, FixedAdmissionPolicy>,
        delegation_id: &str,
        attempt_id: &str,
        package_status: &str,
        expires_at: i64,
    ) {
        let mut conn = scheduler.runtime_mut().connection_mut();
        let session = format!("s_{delegation_id}");
        let message = format!("m_{delegation_id}");
        let run = format!("r_{delegation_id}");
        let package = format!("wp_{delegation_id}");
        conn.execute(
            "INSERT INTO sessions (id,title,created_at,updated_at) VALUES (?1,?1,1,1)",
            [&session],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id,session_id,role,content,created_at) VALUES (?1,?2,'user','x',1)",
            params![message, session],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_runs (id,session_id,message_id,goal,status,plan,created_at,updated_at)
             VALUES (?1,?2,?3,'test','running','[]',1,1)",
            params![run, session, message],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO work_packages
             (id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,worker_policy_version,
              scope_digest,capability_scope_ref,capability_expires_at,candidate_version,status,created_at,updated_at)
             VALUES (?1,?2,1,'workspace','explore','explorer',1,'scope','source',100,0,?3,1,1)",
            params![package, session, package_status],
        )
        .unwrap();
        let brief = DelegationBrief {
            schema_version: 1,
            identity: DelegationIdentity {
                session_id: session.clone(),
                message_id: message.clone(),
                parent_run_id: run.clone(),
                delegation_id: delegation_id.into(),
                attempt_id: attempt_id.into(),
            },
            goal: "test".into(),
            background: vec![],
            constraints: vec![],
            allowed_references: vec!["evidence://test".into()],
            allowed_capabilities: vec![],
            completion_criteria: vec!["done".into()],
        };
        DelegationRepository::create_queued(
            &mut *conn,
            &NewDelegation {
                id: delegation_id.into(),
                session_id: session,
                message_id: message,
                parent_run_id: run,
                work_package_id: Some(package),
                idempotency_key: None,
                objective: "test".into(),
                brief_json: serde_json::to_string(&brief).unwrap(),
            },
            &NewAttempt {
                id: attempt_id.into(),
                attempt_number: 1,
                sandbox_ref: format!("sandbox_{attempt_id}"),
                outbox_id: format!("out_{attempt_id}"),
                dispatch_payload_json: "{}".into(),
            },
            &NewCapabilityLease {
                id: format!("lease_{attempt_id}"),
                read_roots_json: "[]".into(),
                write_roots_json: "[]".into(),
                tool_allowlist_json: "[]".into(),
                network_hosts_json: "[]".into(),
                budget_json: "{}".into(),
                expires_at,
            },
            1,
        )
        .unwrap();
    }

    #[test]
    fn starts_at_most_two_and_deduplicates_claimed_outbox() {
        let mut scheduler = scheduler(2);
        seed(&mut scheduler, "d1", "a1", "active", 100);
        seed(&mut scheduler, "d2", "a2", "active", 100);
        seed(&mut scheduler, "d3", "a3", "active", 100);

        let first_result = scheduler.tick();
        assert!(first_result.is_ok(), "{first_result:?}");
        let first = first_result.unwrap();
        assert_eq!(first.dispatched.len(), 2);
        assert_eq!(scheduler.runtime_mut().worker_mut().starts.len(), 2);
        assert!(scheduler.tick().unwrap().dispatched.is_empty());
    }

    #[test]
    fn frozen_package_and_expired_queued_lease_never_launch() {
        let mut scheduler = scheduler(2);
        seed(&mut scheduler, "frozen", "af", "frozen", 100);
        seed(&mut scheduler, "expired", "ae", "active", 10);

        let report = scheduler.tick().unwrap();
        assert_eq!(report.policy_ineligible, vec!["af"]);
        assert_eq!(report.expired, vec!["ae"]);
        assert!(scheduler.runtime_mut().worker_mut().starts.is_empty());
        let conn = scheduler.runtime().connection();
        let states: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM delegations WHERE status = 'needs_decision'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(states, 2);
    }

    #[test]
    fn recovery_seals_claimed_work_instead_of_replaying_it() {
        let mut scheduler = scheduler(2);
        seed(&mut scheduler, "d1", "a1", "active", 100);
        scheduler.runtime_mut().dispatch_one().unwrap();

        let report = scheduler.start().unwrap();
        assert_eq!(report.recovered_unknown_effects, vec!["a1"]);
        assert!(scheduler.tick().unwrap().dispatched.is_empty());
    }
}
