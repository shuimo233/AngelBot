//! Application-owned delivery of durable follow-ups into the one visible
//! Main-Agent conversation for each Workspace.
//!
//! This is intentionally not a second agent runner. It only claims a stored
//! follow-up, reserves its reply identity, and invokes `message`'s existing
//! foreground turn seam. The durable claim and reply id make retry/restart
//! behavior explicit: work that never crossed `ForegroundRunStore::begin` may
//! be retried; work that did is surfaced for the user instead of replayed.

use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use chrono::Utc;
use tauri::{AppHandle, Manager};
use tokio::task::JoinSet;

use crate::{
    agent::{
        shared_db::SharedDb,
        supervision::{SupervisorError, SupervisorInputStatus, WorkspaceSupervisor},
    },
    commands::{
        automation, delegated_delivery_follow_up,
        foreground_run_store::{ForegroundRunAdmission, ForegroundRunStore},
        message,
    },
    AppState,
};

/// A coalesced wake signal. The desktop heartbeat, manual "run now", and
/// startup recovery may all call `wake`; only one dispatcher loop runs at a
/// time, while independent workspaces are still delivered concurrently.
#[derive(Clone, Default)]
pub(crate) struct ForegroundAutomationPump {
    running: Arc<AtomicBool>,
    pending: Arc<AtomicBool>,
    /// Sessions whose durable follow-up was requeued because a foreground
    /// turn already owned the session. This is intentionally an in-memory
    /// liveness hint: the durable queue remains the source of truth, while
    /// the hint prevents a busy release from waiting for an unrelated wake.
    deferred_session_releases: Arc<Mutex<HashSet<String>>>,
}

/// The generic Workspace supervisor deliberately stores only `follow_up`.
/// This private routing enum binds an already-claimed input to its one durable
/// owner before any Main-Agent turn starts. Adding an owner therefore reuses
/// the same claim, steering, run-store, and recovery loop rather than adding a
/// competing scheduler.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ForegroundFollowUpDispatch {
    Automation(automation::AgentAutomationDispatch),
    ReviewedDelivery(delegated_delivery_follow_up::ReviewedDeliveryDispatch),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ForegroundFollowUpPreparation {
    Dispatch(ForegroundFollowUpDispatch),
    Cancelled,
    Unowned,
}

impl ForegroundFollowUpDispatch {
    fn foreground_run_id(&self) -> &str {
        match self {
            Self::Automation(dispatch) => &dispatch.foreground_run_id,
            Self::ReviewedDelivery(dispatch) => &dispatch.foreground_run_id,
        }
    }

    fn reserved_before_dispatch(&self) -> bool {
        matches!(
            self,
            Self::ReviewedDelivery(dispatch)
                if dispatch.binding.status
                    == crate::agent::delegated_delivery_follow_up::DelegatedDeliveryFollowUpStatus::Running
        )
    }

    fn foreground_task_status(&self, state: &AppState) -> Result<Option<String>, String> {
        match self {
            Self::Automation(dispatch) => {
                automation::foreground_task_status(state, &dispatch.foreground_run_id)
            }
            Self::ReviewedDelivery(dispatch) => {
                delegated_delivery_follow_up::foreground_task_status(
                    state,
                    &dispatch.foreground_run_id,
                )
            }
        }
    }

    fn sync(&self, state: &AppState, now: i64) -> Result<Option<String>, String> {
        match self {
            Self::Automation(dispatch) => {
                automation::sync_agent_automation_dispatch(state, dispatch, now)
            }
            Self::ReviewedDelivery(dispatch) => {
                delegated_delivery_follow_up::sync_reviewed_delivery_dispatch(state, dispatch, now)
            }
        }
    }

    fn reset_to_queued(&self, state: &AppState, now: i64) -> Result<(), String> {
        match self {
            Self::Automation(dispatch) => {
                automation::reset_agent_automation_dispatch_to_queued(state, dispatch, now)
            }
            Self::ReviewedDelivery(dispatch) => {
                delegated_delivery_follow_up::release_reviewed_delivery_dispatch_to_queued(
                    state, dispatch, now,
                )
            }
        }
    }

    fn admission(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        claim_token: &str,
        now: i64,
    ) -> Result<ForegroundRunAdmission, String> {
        match self {
            Self::Automation(dispatch) => match automation::recheck_agent_automation_dispatch_before_begin_in_tx(
                transaction,
                dispatch,
                claim_token,
                now,
            )? {
                automation::AgentAutomationDispatchAdmission::Start => Ok(ForegroundRunAdmission::Start),
                automation::AgentAutomationDispatchAdmission::Cancelled
                | automation::AgentAutomationDispatchAdmission::AlreadyStarted => {
                    Ok(ForegroundRunAdmission::Skip)
                }
            },
            Self::ReviewedDelivery(dispatch) => match delegated_delivery_follow_up::recheck_reviewed_delivery_dispatch_before_begin_in_tx(
                transaction,
                dispatch,
                claim_token,
                now,
            )? {
                delegated_delivery_follow_up::ReviewedDeliveryDispatchAdmission::Start => {
                    Ok(ForegroundRunAdmission::Start)
                }
                delegated_delivery_follow_up::ReviewedDeliveryDispatchAdmission::Cancelled
                | delegated_delivery_follow_up::ReviewedDeliveryDispatchAdmission::AlreadyStarted => {
                    Ok(ForegroundRunAdmission::Skip)
                }
            },
        }
    }
}

fn prepare_foreground_follow_up_dispatch(
    state: &AppState,
    supervisor_input_id: &str,
) -> Result<ForegroundFollowUpPreparation, String> {
    match automation::prepare_agent_automation_dispatch(state, supervisor_input_id)? {
        automation::AgentAutomationDispatchPreparation::Dispatch(dispatch) => {
            Ok(ForegroundFollowUpPreparation::Dispatch(
                ForegroundFollowUpDispatch::Automation(dispatch),
            ))
        }
        automation::AgentAutomationDispatchPreparation::Cancelled => {
            Ok(ForegroundFollowUpPreparation::Cancelled)
        }
        automation::AgentAutomationDispatchPreparation::Unowned => {
            match delegated_delivery_follow_up::prepare_reviewed_delivery_dispatch(
                state,
                supervisor_input_id,
            )? {
                delegated_delivery_follow_up::ReviewedDeliveryDispatchPreparation::Dispatch(
                    dispatch,
                ) => Ok(ForegroundFollowUpPreparation::Dispatch(
                    ForegroundFollowUpDispatch::ReviewedDelivery(dispatch),
                )),
                delegated_delivery_follow_up::ReviewedDeliveryDispatchPreparation::Cancelled => {
                    Ok(ForegroundFollowUpPreparation::Cancelled)
                }
                delegated_delivery_follow_up::ReviewedDeliveryDispatchPreparation::Unowned => {
                    Ok(ForegroundFollowUpPreparation::Unowned)
                }
            }
        }
    }
}

impl ForegroundAutomationPump {
    pub(crate) fn wake(&self, app: AppHandle) {
        self.pending.store(true, Ordering::Release);
        if self.running.swap(true, Ordering::AcqRel) {
            return;
        }
        let pump = self.clone();
        tauri::async_runtime::spawn(async move {
            pump.run(app).await;
        });
    }

    /// Remember that this session needs one retry when its current foreground
    /// turn releases its execution slot. Call only after the durable input has
    /// been returned to the queue; this method never schedules work itself.
    fn defer_until_session_release(&self, session_id: &str) {
        self.deferred_session_releases
            .lock()
            .expect("foreground follow-up deferred session lock poisoned")
            .insert(session_id.to_string());
    }

    /// Wake the durable follow-up pump only when this exact session had been
    /// deferred for a busy foreground turn. Ordinary turn completion does not
    /// poll or retry unrelated queued work.
    pub(crate) fn wake_after_session_release(&self, app: AppHandle, session_id: &str) {
        if self.take_deferred_session_release(session_id) {
            self.wake(app);
        }
    }

    fn take_deferred_session_release(&self, session_id: &str) -> bool {
        self.deferred_session_releases
            .lock()
            .expect("foreground follow-up deferred session lock poisoned")
            .remove(session_id)
    }

    async fn run(self, app: AppHandle) {
        loop {
            self.pending.store(false, Ordering::Release);
            if let Err(error) = dispatch_ready_workspaces(app.clone()).await {
                eprintln!("[Automation] foreground dispatch failed: {error}");
            }
            if self.pending.swap(false, Ordering::AcqRel) {
                continue;
            }
            self.running.store(false, Ordering::Release);
            // A wake can land between the previous exchange and releasing the
            // running flag. Re-acquire here so it cannot be lost.
            if self.pending.swap(false, Ordering::AcqRel)
                && !self.running.swap(true, Ordering::AcqRel)
            {
                continue;
            }
            break;
        }
    }
}

async fn dispatch_ready_workspaces(app: AppHandle) -> Result<(), String> {
    let workspace_ids = {
        let state = app.state::<AppState>();
        WorkspaceSupervisor::new(SharedDb::from_arc(state.db.clone()))
            .queued_follow_up_workspace_ids()
            .map_err(|error| error.to_string())?
    };
    let mut tasks = JoinSet::new();
    for workspace_id in workspace_ids {
        let app = app.clone();
        tasks.spawn(async move { dispatch_workspace_follow_ups(app, workspace_id).await });
    }
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("[Automation] workspace dispatch failed: {error}"),
            Err(error) => eprintln!("[Automation] workspace dispatcher stopped: {error}"),
        }
    }
    Ok(())
}

async fn dispatch_workspace_follow_ups(app: AppHandle, workspace_id: String) -> Result<(), String> {
    loop {
        let state = app.state::<AppState>();
        let supervisor = WorkspaceSupervisor::new(SharedDb::from_arc(state.db.clone()));
        let now = Utc::now().timestamp();
        let Some(follow_up) = supervisor
            .claim_next_follow_up(&workspace_id, now)
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };

        let dispatch =
            match prepare_foreground_follow_up_dispatch(&state, &follow_up.claim.input.id) {
                Ok(ForegroundFollowUpPreparation::Dispatch(dispatch)) => dispatch,
                Ok(ForegroundFollowUpPreparation::Cancelled) => {
                    settle_claim_if_still_owned(
                        &supervisor,
                        &follow_up.claim,
                        SupervisorInputStatus::Cancelled,
                        now,
                    )?;
                    continue;
                }
                Ok(ForegroundFollowUpPreparation::Unowned) => {
                    release_claim_if_still_owned(&supervisor, &follow_up.claim, now)?;
                    return Ok(());
                }
                Err(error) => {
                    // Nothing has crossed the Main-Agent start boundary yet. A
                    // failed prepare must never strand this Workspace input.
                    release_claim_if_still_owned(&supervisor, &follow_up.claim, now)?;
                    return Err(error);
                }
            };

        let existing_status = match dispatch.foreground_task_status(&state) {
            Ok(status) => status,
            Err(error) => {
                reset_and_release_unstarted_dispatch(
                    &state,
                    &supervisor,
                    &follow_up.claim,
                    &dispatch,
                    now,
                )?;
                return Err(error);
            }
        };
        if let Some(status) = existing_status {
            if let Err(error) = dispatch.sync(&state, now) {
                if status != "running" {
                    settle_claim_if_still_owned(
                        &supervisor,
                        &follow_up.claim,
                        SupervisorInputStatus::Completed,
                        now,
                    )?;
                }
                return Err(error);
            }
            if status != "running" {
                settle_claim_if_still_owned(
                    &supervisor,
                    &follow_up.claim,
                    SupervisorInputStatus::Completed,
                    now,
                )?;
                continue;
            }
            // Another in-process owner has already crossed the non-replay
            // boundary. Keep the claim until startup recovery or that owner
            // finalizes it; never make a duplicate Main-Agent turn.
            return Ok(());
        }

        // A stored running binding without its task row predates a process
        // interruption. It may never be replayed with the same reply id;
        // return it to the durable queue first and let a fresh claim create a
        // new bounded foreground turn.
        if dispatch.reserved_before_dispatch() {
            reset_and_release_unstarted_dispatch(
                &state,
                &supervisor,
                &follow_up.claim,
                &dispatch,
                now,
            )?;
            continue;
        }

        let admission_dispatch = dispatch.clone();
        let admission_claim_token = follow_up.claim.claim_token.clone();
        let admission: crate::commands::foreground_run_store::ForegroundRunAdmissionGuard =
            Box::new(move |transaction| {
                admission_dispatch.admission(transaction, &admission_claim_token, now)
            });
        let follow_up_outcome = match &dispatch {
            ForegroundFollowUpDispatch::Automation(_) => {
                message::run_supervised_follow_up_with_admission(
                    &state,
                    &app,
                    follow_up.session_id.clone(),
                    follow_up.claim.input.content.clone(),
                    dispatch.foreground_run_id().to_string(),
                    admission,
                )
                .await
            }
            ForegroundFollowUpDispatch::ReviewedDelivery(reviewed) => {
                message::run_reviewed_delivery_follow_up_with_admission(
                    &state,
                    &app,
                    follow_up.session_id.clone(),
                    follow_up.claim.input.content.clone(),
                    dispatch.foreground_run_id().to_string(),
                    reviewed.binding.delivery_id.clone(),
                    admission,
                )
                .await
            }
        };
        match follow_up_outcome {
            Ok(message::SupervisedFollowUpOutcome::Completed(_)) => {
                let status = match dispatch.sync(&state, now) {
                    Ok(status) => status,
                    Err(error) => {
                        // The turn has already started, so settle the delivery
                        // rather than retrying it. The durable task remains the
                        // source of truth for any later reconciliation.
                        settle_claim_if_still_owned(
                            &supervisor,
                            &follow_up.claim,
                            SupervisorInputStatus::Completed,
                            now,
                        )?;
                        return Err(error);
                    }
                };
                if status.as_deref() == Some("running") || status.is_none() {
                    settle_claim_if_still_owned(
                        &supervisor,
                        &follow_up.claim,
                        SupervisorInputStatus::Completed,
                        now,
                    )?;
                    return Err(
                        "completed foreground automation turn has no terminal task state".into(),
                    );
                }
                settle_claim_if_still_owned(
                    &supervisor,
                    &follow_up.claim,
                    SupervisorInputStatus::Completed,
                    now,
                )?;
            }
            Ok(message::SupervisedFollowUpOutcome::Skipped) => {
                // The admission hook is a true no-op: it either observed a
                // lifecycle cancellation or a task that another owner had
                // already started. Reconcile that durable state without ever
                // calling the model a second time.
                match dispatch.foreground_task_status(&state) {
                    Ok(Some(status)) => {
                        dispatch.sync(&state, now)?;
                        if status != "running" {
                            settle_claim_if_still_owned(
                                &supervisor,
                                &follow_up.claim,
                                SupervisorInputStatus::Completed,
                                now,
                            )?;
                            continue;
                        }
                        return Ok(());
                    }
                    Ok(None) => {
                        if dispatch.reserved_before_dispatch() {
                            reset_and_release_unstarted_dispatch(
                                &state,
                                &supervisor,
                                &follow_up.claim,
                                &dispatch,
                                now,
                            )?;
                        } else {
                            settle_claim_if_still_owned(
                                &supervisor,
                                &follow_up.claim,
                                SupervisorInputStatus::Cancelled,
                                now,
                            )?;
                        }
                        continue;
                    }
                    Err(error) => {
                        reset_and_release_unstarted_dispatch(
                            &state,
                            &supervisor,
                            &follow_up.claim,
                            &dispatch,
                            now,
                        )?;
                        return Err(error);
                    }
                }
            }
            Err(error) => {
                let store = ForegroundRunStore::new(state.db.clone());
                if store.recover_known_provisional_run(
                    &follow_up.session_id,
                    dispatch.foreground_run_id(),
                    now,
                )? {
                    dispatch.sync(&state, now)?;
                    settle_claim_if_still_owned(
                        &supervisor,
                        &follow_up.claim,
                        SupervisorInputStatus::Completed,
                        now,
                    )?;
                    continue;
                }

                // If a task row exists but is already non-provisional, it has
                // crossed the non-replay boundary through another error path.
                // Surface its saved state instead of releasing a duplicate.
                if let Some(status) = dispatch.foreground_task_status(&state)? {
                    dispatch.sync(&state, now)?;
                    if status != "running" {
                        settle_claim_if_still_owned(
                            &supervisor,
                            &follow_up.claim,
                            SupervisorInputStatus::Completed,
                            now,
                        )?;
                    }
                    return Ok(());
                }

                if is_session_busy_error(&error) {
                    dispatch.reset_to_queued(&state, now)?;
                    release_claim_if_still_owned(&supervisor, &follow_up.claim, now)?;
                    state
                        .foreground_automation_pump
                        .defer_until_session_release(&follow_up.session_id);
                    return Ok(());
                }

                match &dispatch {
                    ForegroundFollowUpDispatch::Automation(dispatch) => {
                        automation::mark_agent_automation_dispatch_needs_attention(
                            &state, dispatch, now, &error,
                        )?;
                        settle_claim_if_still_owned(
                            &supervisor,
                            &follow_up.claim,
                            SupervisorInputStatus::Completed,
                            now,
                        )?;
                    }
                    ForegroundFollowUpDispatch::ReviewedDelivery(_) => {
                        // No task row was created, so preserving the durable
                        // reviewed delivery is more useful than silently
                        // terminalizing it. A later explicit wake or user
                        // turn can safely pick it up without replaying work.
                        dispatch.reset_to_queued(&state, now)?;
                        release_claim_if_still_owned(&supervisor, &follow_up.claim, now)?;
                    }
                }
                return Ok(());
            }
        }
    }
}

fn reset_and_release_unstarted_dispatch(
    state: &AppState,
    supervisor: &WorkspaceSupervisor,
    claim: &crate::agent::supervision::ClaimedInput,
    dispatch: &ForegroundFollowUpDispatch,
    now: i64,
) -> Result<(), String> {
    dispatch.reset_to_queued(state, now)?;
    release_claim_if_still_owned(supervisor, claim, now)
}

fn release_claim_if_still_owned(
    supervisor: &WorkspaceSupervisor,
    claim: &crate::agent::supervision::ClaimedInput,
    now: i64,
) -> Result<(), String> {
    match supervisor.release(claim, now) {
        Ok(()) | Err(SupervisorError::ClaimNotFound) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn settle_claim_if_still_owned(
    supervisor: &WorkspaceSupervisor,
    claim: &crate::agent::supervision::ClaimedInput,
    status: SupervisorInputStatus,
    now: i64,
) -> Result<(), String> {
    match supervisor.settle(claim, status, now) {
        Ok(()) | Err(SupervisorError::ClaimNotFound) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn is_session_busy_error(error: &str) -> bool {
    error.contains("already running in this session")
}

#[cfg(test)]
mod tests {
    use super::{is_session_busy_error, ForegroundAutomationPump};

    #[test]
    fn busy_session_error_is_retried_without_replaying_work() {
        assert!(is_session_busy_error(
            "An agent task is already running in this session. Send an in-progress instruction or wait for it to finish."
        ));
        assert!(!is_session_busy_error(
            "provider credentials are unavailable"
        ));
    }

    #[test]
    fn deferred_session_release_is_consumed_only_by_that_session() {
        let pump = ForegroundAutomationPump::default();
        pump.defer_until_session_release("session-a");
        pump.defer_until_session_release("session-a");

        assert!(!pump.take_deferred_session_release("session-b"));
        assert!(pump.take_deferred_session_release("session-a"));
        assert!(!pump.take_deferred_session_release("session-a"));
    }
}
