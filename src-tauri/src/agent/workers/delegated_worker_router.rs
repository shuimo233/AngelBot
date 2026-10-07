//! One runtime ownership boundary for Explorer and Change workers.
//!
//! The scheduler may dispatch several profiles, but it must still have one
//! `WorkerAdapter`, one async bridge, and one poll/stop lifecycle. This router
//! chooses the profile from durable `work_packages` state, resolves the opaque
//! binding, then hands the typed request to the corresponding launcher. It
//! never parses the outbox payload and never exposes a second worker loop.

use super::{
    change_worker_runtime::resolve_change_launch,
    delegated_worker_launcher::{
        AsyncWorkerLauncher, AsyncWorkerLauncherBridge, WorkerAttemptBinding, WorkerEventCallback,
        WorkerLaunchError, WorkerLaunchRequest,
    },
    delegation_runtime::{StopReason, WorkerAdapter, WorkerDispatch, WorkerEvent},
    explorer_plan::ExplorerBindingResolver,
    shared_db::SharedDb,
    worker_policy::WorkerProfile,
};

pub struct RoutedAsyncWorkerLauncher {
    explorer: Box<dyn AsyncWorkerLauncher>,
    change: Box<dyn AsyncWorkerLauncher>,
}

impl RoutedAsyncWorkerLauncher {
    pub fn new(
        explorer: Box<dyn AsyncWorkerLauncher>,
        change: Box<dyn AsyncWorkerLauncher>,
    ) -> Self {
        Self { explorer, change }
    }
}

impl AsyncWorkerLauncher for RoutedAsyncWorkerLauncher {
    fn launch(
        &mut self,
        request: WorkerLaunchRequest,
    ) -> Result<Box<dyn super::delegated_worker_launcher::WorkerTask>, WorkerLaunchError> {
        // A typed Explorer plan owns the network-only launcher. A local
        // Explorer has no network plan and therefore runs through the
        // read-only model/worktree host. The scheduler still has one bridge.
        if request.plan.is_some() {
            return self.explorer.launch(request);
        }
        self.change.launch(request)
    }
}

pub struct RoutedWorkerAdapter {
    db: SharedDb,
    explorer_resolver: ExplorerBindingResolver,
    bridge: AsyncWorkerLauncherBridge<RoutedAsyncWorkerLauncher>,
    now: fn() -> i64,
}

impl RoutedWorkerAdapter {
    pub fn new(
        db: SharedDb,
        explorer_resolver: ExplorerBindingResolver,
        explorer: Box<dyn AsyncWorkerLauncher>,
        change: Box<dyn AsyncWorkerLauncher>,
        callback: WorkerEventCallback,
        now: fn() -> i64,
    ) -> Self {
        let launcher = RoutedAsyncWorkerLauncher::new(explorer, change);
        Self {
            db,
            explorer_resolver,
            bridge: AsyncWorkerLauncherBridge::new(launcher, callback),
            now,
        }
    }

    fn profile(&self, dispatch: &WorkerDispatch) -> Result<WorkerProfile, String> {
        let profile = self
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT p.worker_profile FROM work_packages p
                     JOIN delegations d ON d.work_package_id=p.id
                     WHERE d.id=?1",
                    [dispatch.delegation_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
            })
            .map_err(|_| "worker profile unavailable".to_string())?
            .map_err(|_| "worker profile unavailable".to_string())?;
        WorkerProfile::from_str(&profile).ok_or_else(|| "unknown worker profile".into())
    }

    fn binding_for(
        &self,
        dispatch: &WorkerDispatch,
    ) -> Result<
        (
            WorkerAttemptBinding,
            Option<super::explorer_worker_runtime::ExplorerWorkPlan>,
        ),
        String,
    > {
        match self.profile(dispatch)? {
            WorkerProfile::Explorer => match self.explorer_resolver.resolve(dispatch) {
                Ok((binding, plan)) => Ok((binding, Some(plan))),
                Err(super::explorer_plan::ExplorerPlanError::NotFound) => {
                    let context = resolve_change_launch(&self.db, dispatch, (self.now)())
                        .map_err(|error| format!("local explorer binding rejected: {error:?}"))?;
                    let identity = context.binding.identity();
                    Ok((
                        WorkerAttemptBinding {
                            attempt_id: identity.attempt_id().to_string(),
                            delegation_id: identity.delegation_id().to_string(),
                            work_package_id: identity.work_package_id().to_string(),
                            lease_epoch: context.binding.lease_epoch(),
                            capability_ref: context.binding.capability_ref().to_string(),
                            sandbox_ref: format!("sandbox://{}", identity.attempt_id()),
                        },
                        None,
                    ))
                }
                Err(error) => Err(format!("explorer binding rejected: {error:?}")),
            },
            WorkerProfile::Implementer | WorkerProfile::Verifier => {
                let context = resolve_change_launch(&self.db, dispatch, (self.now)())
                    .map_err(|error| format!("change binding rejected: {error:?}"))?;
                let identity = context.binding.identity();
                Ok((
                    WorkerAttemptBinding {
                        attempt_id: identity.attempt_id().to_string(),
                        delegation_id: identity.delegation_id().to_string(),
                        work_package_id: identity.work_package_id().to_string(),
                        lease_epoch: context.binding.lease_epoch(),
                        capability_ref: context.binding.capability_ref().to_string(),
                        sandbox_ref: format!("sandbox://{}", identity.attempt_id()),
                    },
                    None,
                ))
            }
        }
    }
}

impl WorkerAdapter for RoutedWorkerAdapter {
    fn start(&mut self, dispatch: WorkerDispatch) -> Result<(), String> {
        let (binding, plan) = self.binding_for(&dispatch)?;
        self.bridge
            .bind(binding)
            .map_err(|error| format!("worker binding rejected: {error:?}"))?;
        self.bridge
            .start_with_plan(dispatch, plan)
            .map_err(|error| format!("worker launch rejected: {error:?}"))
    }

    fn stop(&mut self, attempt_id: &str, reason: StopReason) -> Result<(), String> {
        self.bridge
            .stop(attempt_id, reason)
            .map_err(|error| format!("worker stop rejected: {error:?}"))
    }

    fn poll_workers(&mut self) -> Result<Vec<WorkerEvent>, String> {
        self.bridge
            .poll_workers()
            .map_err(|error| format!("worker poll rejected: {error:?}"))
    }

    fn active_worker_attempts(&self) -> usize {
        self.bridge.active_attempts()
    }
}
