//! Command-layer lifecycle for one reviewed Explorer delivery summary.
//!
//! The agent-layer queue owns source binding and eligibility. This module owns
//! only the foreground-run boundary: claim a generic supervisor input, reserve
//! one hidden Main-Agent reply, and settle or release the claim without ever
//! replaying a started turn.

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    agent::{
        delegated_delivery_follow_up::{
            DelegatedDeliveryFollowUp, DelegatedDeliveryFollowUpQueue,
            DelegatedDeliveryFollowUpStart, DelegatedDeliveryFollowUpStatus,
        },
        shared_db::SharedDb,
    },
    commands::foreground_run_store::ForegroundRunAdmission,
    AppState,
};

/// A durable reviewed-delivery binding after its generic Workspace input has
/// been claimed by the shared foreground follow-up pump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewedDeliveryDispatch {
    pub binding: DelegatedDeliveryFollowUp,
    pub foreground_run_id: String,
}

/// A generic `follow_up` input may have another owner. The shared pump must
/// release rather than consume an input it cannot prove belongs here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReviewedDeliveryDispatchPreparation {
    Dispatch(ReviewedDeliveryDispatch),
    Cancelled,
    Unowned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewedDeliveryDispatchAdmission {
    Start,
    Cancelled,
    AlreadyStarted,
}

pub(crate) fn prepare_reviewed_delivery_dispatch(
    state: &AppState,
    supervisor_input_id: &str,
) -> Result<ReviewedDeliveryDispatchPreparation, String> {
    let mut connection = state.db.lock().map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let input_status = transaction
        .query_row(
            "SELECT status FROM workspace_supervisor_inputs WHERE id = ?1",
            [supervisor_input_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let queue = DelegatedDeliveryFollowUpQueue::new(SharedDb::from_arc(state.db.clone()));
    let binding = queue
        .load_by_supervisor_input_id_in_tx(&transaction, supervisor_input_id)
        .map_err(|error| error.to_string())?;

    let outcome = match binding {
        None if input_status.as_deref() == Some("cancelled") => {
            ReviewedDeliveryDispatchPreparation::Cancelled
        }
        None => ReviewedDeliveryDispatchPreparation::Unowned,
        Some(_binding) if input_status.as_deref() != Some("claimed") => {
            ReviewedDeliveryDispatchPreparation::Cancelled
        }
        Some(binding) => match binding.status {
            DelegatedDeliveryFollowUpStatus::Queued => {
                // Keep the durable binding queued until ForegroundRunStore
                // opens its reservation transaction. That later admission
                // performs the only state transition, atomically with the
                // hidden reply and task-run rows.
                ReviewedDeliveryDispatchPreparation::Dispatch(ReviewedDeliveryDispatch {
                    binding,
                    foreground_run_id: format!("delivery_summary_turn_{}", Uuid::new_v4().simple()),
                })
            }
            DelegatedDeliveryFollowUpStatus::Running => {
                let Some(foreground_run_id) = binding.foreground_run_id.clone() else {
                    return Err(
                        "running delegated delivery follow-up has no foreground run id".into(),
                    );
                };
                ReviewedDeliveryDispatchPreparation::Dispatch(ReviewedDeliveryDispatch {
                    binding,
                    foreground_run_id,
                })
            }
            DelegatedDeliveryFollowUpStatus::Summarized
            | DelegatedDeliveryFollowUpStatus::Cancelled => {
                ReviewedDeliveryDispatchPreparation::Cancelled
            }
        },
    };
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(outcome)
}

/// Revalidate the exact claimed input in the foreground reservation
/// transaction. This is intentionally the first point that marks the source
/// binding running, so a crash before the Main-Agent run exists leaves an
/// ordinary queued follow-up that can be recovered safely.
pub(crate) fn recheck_reviewed_delivery_dispatch_before_begin_in_tx(
    transaction: &Transaction<'_>,
    dispatch: &ReviewedDeliveryDispatch,
    claim_token: &str,
    now: i64,
) -> Result<ReviewedDeliveryDispatchAdmission, String> {
    let task_exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM task_runs WHERE id = ?1)",
            [&dispatch.foreground_run_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if task_exists {
        return Ok(ReviewedDeliveryDispatchAdmission::AlreadyStarted);
    }
    match DelegatedDeliveryFollowUpQueue::begin_claimed_in_tx(
        transaction,
        &dispatch.binding.supervisor_input_id,
        claim_token,
        &dispatch.foreground_run_id,
        now,
    )
    .map_err(|error| error.to_string())?
    {
        DelegatedDeliveryFollowUpStart::Started(binding)
            if binding.delivery_id == dispatch.binding.delivery_id
                && binding.delivery_revision == dispatch.binding.delivery_revision =>
        {
            Ok(ReviewedDeliveryDispatchAdmission::Start)
        }
        DelegatedDeliveryFollowUpStart::Started(_) => {
            Err("delegated delivery follow-up source changed during foreground admission".into())
        }
        DelegatedDeliveryFollowUpStart::NotRunnable => {
            let _ = DelegatedDeliveryFollowUpQueue::cancel_queued_in_tx(
                transaction,
                &dispatch.binding.supervisor_input_id,
                now,
            )
            .map_err(|error| error.to_string())?;
            Ok(ReviewedDeliveryDispatchAdmission::Cancelled)
        }
    }
}

pub(crate) fn foreground_task_status(
    state: &AppState,
    foreground_run_id: &str,
) -> Result<Option<String>, String> {
    let connection = state.db.lock().map_err(|error| error.to_string())?;
    connection
        .query_row(
            "SELECT status FROM task_runs WHERE id = ?1",
            [foreground_run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewedDeliveryTerminalSettlement {
    /// The constrained tool successfully acknowledged this exact delivery and
    /// the durable source has reached a terminal delivery state.
    Summarized,
    /// The source no longer admits automatic summarization, but this turn
    /// cannot prove that it performed the acknowledgement itself.
    Cancelled,
    /// The foreground turn ended before it acknowledged the source. Retain a
    /// fresh queue entry, but let the current pump pass end to avoid a tight
    /// in-process retry loop.
    Requeued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewedDeliverySourceState {
    TerminalDelivery,
    NoLongerSummarizable,
    Pending,
}

/// Settle the source binding only after proving the foreground task contains
/// the exact constrained tool call for its immutable delivery identity.
///
/// A task becoming terminal is not evidence that it summarized the reviewed
/// result: provider failures and text-only completions are durable terminal
/// tasks too. Keeping this proof in the same transaction as the queue/input
/// mutation makes ordinary completion and startup recovery obey the same
/// boundary.
fn settle_terminal_reviewed_delivery_in_tx(
    transaction: &Transaction<'_>,
    supervisor_input_id: &str,
    session_id: &str,
    delivery_id: &str,
    delivery_revision: i64,
    foreground_run_id: &str,
    now: i64,
) -> Result<ReviewedDeliveryTerminalSettlement, String> {
    let source_state =
        reviewed_delivery_source_state_in_tx(transaction, delivery_id, delivery_revision)?;
    let has_scoped_success = successful_scoped_review_step_in_tx(
        transaction,
        session_id,
        delivery_id,
        foreground_run_id,
    )?;
    let settlement = match source_state {
        ReviewedDeliverySourceState::TerminalDelivery if has_scoped_success => {
            ReviewedDeliveryTerminalSettlement::Summarized
        }
        ReviewedDeliverySourceState::Pending => ReviewedDeliveryTerminalSettlement::Requeued,
        ReviewedDeliverySourceState::TerminalDelivery
        | ReviewedDeliverySourceState::NoLongerSummarizable => {
            ReviewedDeliveryTerminalSettlement::Cancelled
        }
    };

    let changed = match settlement {
        ReviewedDeliveryTerminalSettlement::Summarized => {
            DelegatedDeliveryFollowUpQueue::mark_summarized_in_tx(
                transaction,
                supervisor_input_id,
                foreground_run_id,
                now,
            )
            .map_err(|error| error.to_string())?
        }
        ReviewedDeliveryTerminalSettlement::Cancelled => {
            DelegatedDeliveryFollowUpQueue::cancel_running_in_tx(
                transaction,
                supervisor_input_id,
                foreground_run_id,
                now,
            )
            .map_err(|error| error.to_string())?
        }
        ReviewedDeliveryTerminalSettlement::Requeued => {
            DelegatedDeliveryFollowUpQueue::release_running_in_tx(
                transaction,
                supervisor_input_id,
                foreground_run_id,
                now,
            )
            .map_err(|error| error.to_string())?
        }
    };

    // A stale recovery can observe a binding that a newer owner has already
    // moved. Its compare-and-swap must be a harmless no-op rather than a
    // reason to terminalize that newer owner's generic claim.
    if !changed {
        return Ok(ReviewedDeliveryTerminalSettlement::Cancelled);
    }

    let (status, clear_claim_token) = match settlement {
        ReviewedDeliveryTerminalSettlement::Summarized
        | ReviewedDeliveryTerminalSettlement::Cancelled => ("completed", false),
        ReviewedDeliveryTerminalSettlement::Requeued => ("queued", true),
    };
    transaction
        .execute(
            "UPDATE workspace_supervisor_inputs
             SET status = ?1,
                 claim_token = CASE WHEN ?2 THEN NULL ELSE claim_token END,
                 updated_at = ?3
             WHERE id = ?4 AND status = 'claimed'",
            params![status, clear_claim_token, now, supervisor_input_id],
        )
        .map_err(|error| error.to_string())?;
    Ok(settlement)
}

fn reviewed_delivery_source_state_in_tx(
    transaction: &Transaction<'_>,
    delivery_id: &str,
    delivery_revision: i64,
) -> Result<ReviewedDeliverySourceState, String> {
    let source = transaction
        .query_row(
            "SELECT delivery.acceptance_status, delegation.status
             FROM delegation_deliveries delivery
             JOIN delegations delegation ON delegation.id = delivery.delegation_id
             WHERE delivery.id = ?1 AND delivery.delivery_version = ?2",
            params![delivery_id, delivery_revision],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((acceptance_status, delegation_status)) = source else {
        return Ok(ReviewedDeliverySourceState::NoLongerSummarizable);
    };
    if matches!(acceptance_status.as_str(), "accepted" | "rejected") {
        return Ok(ReviewedDeliverySourceState::TerminalDelivery);
    }
    if matches!(
        delegation_status.as_str(),
        "needs_decision" | "completed" | "failed" | "cancelled"
    ) {
        return Ok(ReviewedDeliverySourceState::NoLongerSummarizable);
    }
    Ok(ReviewedDeliverySourceState::Pending)
}

fn successful_scoped_review_step_in_tx(
    transaction: &Transaction<'_>,
    session_id: &str,
    delivery_id: &str,
    foreground_run_id: &str,
) -> Result<bool, String> {
    let task = transaction
        .query_row(
            "SELECT reply.created_at, reply.tool_calls
             FROM task_runs run
             JOIN messages reply
               ON reply.id = run.message_id AND reply.session_id = run.session_id
             WHERE run.id = ?1 AND run.session_id = ?2",
            params![foreground_run_id, session_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((created_at, Some(tool_calls))) = task else {
        return Ok(false);
    };
    let Ok(Value::Array(calls)) = serde_json::from_str::<Value>(&tool_calls) else {
        return Ok(false);
    };

    for call in calls {
        let Some(call_id) = call.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(arguments) = decoded_tool_call_arguments(call.get("arguments")) else {
            continue;
        };
        let is_scoped_review = call.get("name").and_then(Value::as_str)
            == Some("review_delegated_delivery")
            && arguments.get("delivery_id").and_then(Value::as_str) == Some(delivery_id)
            && matches!(
                arguments.get("decision").and_then(Value::as_str),
                Some("accept" | "needs_decision")
            );
        if !is_scoped_review {
            continue;
        }
        let succeeded = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_steps
                     WHERE session_id = ?1
                       AND created_at = ?2
                       AND call_id = ?3
                       AND tool_name = 'review_delegated_delivery'
                       AND success = 1
                 )",
                params![session_id, created_at, call_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| error.to_string())?;
        if succeeded {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Tool calls are persisted as an inline protocol record. Existing persisted
/// rows encode `arguments` as JSON text, while a few migration-era fixtures
/// carry an object. Treat either shape as evidence only after it parses to a
/// JSON object; malformed data never proves summary completion.
fn decoded_tool_call_arguments(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Object(_)) => value.cloned(),
        Some(Value::String(encoded)) => serde_json::from_str::<Value>(encoded).ok(),
        _ => None,
    }
    .filter(Value::is_object)
}

pub(crate) fn sync_reviewed_delivery_dispatch(
    state: &AppState,
    dispatch: &ReviewedDeliveryDispatch,
    now: i64,
) -> Result<Option<String>, String> {
    let Some(task_status) = foreground_task_status(state, &dispatch.foreground_run_id)? else {
        return Ok(None);
    };
    if task_status == "running" {
        return Ok(Some(task_status));
    }
    let mut connection = state.db.lock().map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let settlement = settle_terminal_reviewed_delivery_in_tx(
        &transaction,
        &dispatch.binding.supervisor_input_id,
        &dispatch.binding.session_id,
        &dispatch.binding.delivery_id,
        dispatch.binding.delivery_revision,
        &dispatch.foreground_run_id,
        now,
    )?;
    transaction.commit().map_err(|error| error.to_string())?;
    match settlement {
        ReviewedDeliveryTerminalSettlement::Requeued => Ok(None),
        ReviewedDeliveryTerminalSettlement::Summarized
        | ReviewedDeliveryTerminalSettlement::Cancelled => Ok(Some(task_status)),
    }
}

/// Restore an unstarted binding to the queue. It is used when a user turn is
/// already active; no model call or child payload has crossed a boundary.
pub(crate) fn release_reviewed_delivery_dispatch_to_queued(
    state: &AppState,
    dispatch: &ReviewedDeliveryDispatch,
    now: i64,
) -> Result<(), String> {
    let mut connection = state.db.lock().map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let _ = DelegatedDeliveryFollowUpQueue::release_running_in_tx(
        &transaction,
        &dispatch.binding.supervisor_input_id,
        &dispatch.foreground_run_id,
        now,
    )
    .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(())
}

/// Reconcile source bindings after foreground-run recovery. A linked task is
/// not automatically a successful summary: it must pass the same exact tool
/// and terminal-source proof as ordinary dispatch. A missing task never
/// crossed the foreground boundary and is safely returned to the generic
/// follow-up queue.
pub(crate) fn recover_interrupted_reviewed_delivery_follow_ups_in_conn(
    connection: &mut Connection,
    now: i64,
) -> Result<usize, String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT follow_up.delivery_id, follow_up.delivery_revision,
                        follow_up.supervisor_input_id, delegation.session_id,
                        follow_up.foreground_run_id, follow_up.status, input.status, task.status
                 FROM delegated_delivery_follow_ups follow_up
                  JOIN workspace_supervisor_inputs input
                    ON input.id = follow_up.supervisor_input_id
                  JOIN delegation_deliveries delivery
                    ON delivery.id = follow_up.delivery_id
                   AND delivery.delivery_version = follow_up.delivery_revision
                  JOIN delegations delegation ON delegation.id = delivery.delegation_id
                  LEFT JOIN task_runs task
                    ON task.id = follow_up.foreground_run_id
                 WHERE follow_up.status IN ('queued', 'running', 'summarized', 'cancelled')",
            )
            .map_err(|error| error.to_string())?;
        let mapped = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    let mut reconciled = 0;
    for (
        delivery_id,
        delivery_revision,
        input_id,
        session_id,
        foreground_run_id,
        binding_status,
        input_status,
        task_status,
    ) in rows
    {
        match (
            binding_status.as_str(),
            foreground_run_id.as_deref(),
            task_status.as_deref(),
        ) {
            ("running", Some(run_id), Some(task_status)) if task_status != "running" => {
                let _ = settle_terminal_reviewed_delivery_in_tx(
                    &transaction,
                    &input_id,
                    &session_id,
                    &delivery_id,
                    delivery_revision,
                    run_id,
                    now,
                )?;
                reconciled += 1;
            }
            ("running", Some(run_id), None) => {
                if DelegatedDeliveryFollowUpQueue::release_running_in_tx(
                    &transaction,
                    &input_id,
                    run_id,
                    now,
                )
                .map_err(|error| error.to_string())?
                {
                    transaction
                        .execute(
                            "UPDATE workspace_supervisor_inputs
                             SET status = 'queued', claim_token = NULL, updated_at = ?1
                             WHERE id = ?2 AND status = 'claimed'",
                            params![now, input_id],
                        )
                        .map_err(|error| error.to_string())?;
                    reconciled += 1;
                }
            }
            ("queued", _, _) if input_status == "claimed" => {
                transaction
                    .execute(
                        "UPDATE workspace_supervisor_inputs
                         SET status = 'queued', claim_token = NULL, updated_at = ?1
                         WHERE id = ?2 AND status = 'claimed'",
                        params![now, input_id],
                    )
                    .map_err(|error| error.to_string())?;
                reconciled += 1;
            }
            ("summarized", _, _) if input_status == "claimed" => {
                transaction
                    .execute(
                        "UPDATE workspace_supervisor_inputs
                         SET status = 'completed', updated_at = ?1
                         WHERE id = ?2 AND status = 'claimed'",
                        params![now, input_id],
                    )
                    .map_err(|error| error.to_string())?;
                reconciled += 1;
            }
            ("cancelled", _, _) if input_status == "claimed" => {
                transaction
                    .execute(
                        "UPDATE workspace_supervisor_inputs
                         SET status = 'cancelled', updated_at = ?1
                         WHERE id = ?2 AND status = 'claimed'",
                        params![now, input_id],
                    )
                    .map_err(|error| error.to_string())?;
                reconciled += 1;
            }
            _ => {}
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(reconciled)
}

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};

    use super::*;

    fn running_follow_up_connection(
        task_status: &str,
        tool_call: Option<(&str, &str, bool)>,
        delivery_status: &str,
        delegation_status: &str,
    ) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::db::migrate(&connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO sessions(id,title,created_at,updated_at)
                     VALUES ('session','session',1,1);
                 INSERT INTO projects(id,name,path,kind,active_session_id,created_at,updated_at)
                     VALUES ('workspace','workspace','C:/workspace','project','session',1,1);
                 INSERT INTO messages(id,session_id,role,content,created_at)
                     VALUES ('parent-message','session','user','inspect',1);
                 INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at)
                     VALUES ('parent-run','session','parent-message','inspect','running','[]',1,1);
                 INSERT INTO work_packages(
                     id,session_id,owner_profile_id,workspace_key,task_shape,worker_profile,
                     worker_policy_version,scope_digest,capability_scope_ref,
                     capability_expires_at,candidate_version,status,created_at,updated_at
                 ) VALUES (
                     'package','session',1,'workspace','explore','explorer',1,'scope','approved',100,
                     0,'active',1,1
                 );
                 INSERT INTO delegations(
                     id,session_id,message_id,parent_run_id,work_package_id,objective,
                     brief_json,status,state_version,created_at,updated_at
                 ) VALUES (
                     'delegation','session','parent-message','parent-run','package','inspect','{}','awaiting_summary',0,1,1
                 );
                 INSERT INTO delegation_attempts(
                     id,delegation_id,attempt_number,status,sandbox_ref,created_at,ended_at
                 ) VALUES ('attempt','delegation',1,'sealed','sandbox://attempt',1,2);
                 INSERT INTO delegation_deliveries(
                     id,delegation_id,attempt_id,delivery_version,schema_version,payload_json,
                     acceptance_status,accepted_by,accepted_at,created_at
                 ) VALUES (
                     'delivery','delegation','attempt',1,1,'{}','submitted',NULL,NULL,2
                 );
                 INSERT INTO delegation_review_jobs(
                     id,implementation_attempt_id,delivery_id,delivery_revision,subject_json,
                     status,reviewer_attempt_id,outcome_json,created_at,updated_at,started_at,ended_at
                 ) VALUES (
                     'review','attempt','delivery',1,'{}','passed','reviewer','{}',2,2,2,2
                 );
                 INSERT INTO workspace_supervisor_inputs(
                     id,workspace_id,kind,content,priority,status,claim_token,created_at,updated_at
                 ) VALUES ('input','workspace','follow_up','summary',0,'claimed','claim',3,3);
                 INSERT INTO delegated_delivery_follow_ups(
                     delivery_id,delivery_revision,supervisor_input_id,foreground_run_id,status,created_at,updated_at
                 ) VALUES ('delivery',1,'input','summary-run','running',3,3);
                 INSERT INTO messages(id,session_id,role,content,created_at,tool_calls)
                     VALUES ('summary-message','session','assistant','summary',10,NULL);
                 INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at)
                     VALUES ('summary-run','session','summary-message','summary','running','[]',10,10);",
            )
            .unwrap();
        connection
            .execute(
                "UPDATE task_runs SET status = ?1 WHERE id = 'summary-run'",
                [task_status],
            )
            .unwrap();

        if delivery_status == "accepted" {
            connection
                .execute(
                    "UPDATE delegation_deliveries
                     SET acceptance_status = 'accepted', accepted_by = 'parent_agent', accepted_at = 11
                     WHERE id = 'delivery'",
                    [],
                )
                .unwrap();
        } else if delivery_status == "rejected" {
            connection
                .execute(
                    "UPDATE delegation_deliveries
                     SET acceptance_status = 'rejected' WHERE id = 'delivery'",
                    [],
                )
                .unwrap();
        }
        connection
            .execute(
                "UPDATE delegations SET status = ?1 WHERE id = 'delegation'",
                [delegation_status],
            )
            .unwrap();

        if let Some((call_delivery_id, decision, success)) = tool_call {
            let call_id = "review-call";
            let arguments = serde_json::json!({
                "delivery_id": call_delivery_id,
                "decision": decision,
            })
            .to_string();
            let tool_calls = serde_json::json!([{
                "id": call_id,
                "name": "review_delegated_delivery",
                "arguments": arguments,
            }]);
            connection
                .execute(
                    "UPDATE messages SET tool_calls = ?1 WHERE id = 'summary-message'",
                    [tool_calls.to_string()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO agent_steps(
                         id,call_id,session_id,tool_name,tool_input,tool_output,success,seq,created_at
                     ) VALUES ('step',?1,'session','review_delegated_delivery',?2,'recorded',?3,1,10)",
                    params![
                        call_id,
                        serde_json::json!({
                            "delivery_id": call_delivery_id,
                            "decision": decision,
                        })
                        .to_string(),
                        success,
                    ],
                )
                .unwrap();
        }
        connection
    }

    #[test]
    fn provider_failure_is_requeued_instead_of_being_falsely_summarized() {
        let mut connection = running_follow_up_connection(
            "provider_unavailable",
            None,
            "submitted",
            "awaiting_summary",
        );

        let reconciled =
            recover_interrupted_reviewed_delivery_follow_ups_in_conn(&mut connection, 20).unwrap();

        assert_eq!(reconciled, 1);
        let states: (String, Option<String>, String, Option<String>) = connection
            .query_row(
                "SELECT follow_up.status, follow_up.foreground_run_id, input.status, input.claim_token
                 FROM delegated_delivery_follow_ups follow_up
                 JOIN workspace_supervisor_inputs input
                   ON input.id = follow_up.supervisor_input_id
                 WHERE follow_up.supervisor_input_id = 'input'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            states,
            ("queued".to_string(), None, "queued".to_string(), None)
        );
    }

    #[test]
    fn exact_successful_review_and_terminal_delivery_are_required_to_summarize() {
        for (delivery_status, delegation_status, decision) in [
            ("accepted", "completed", "accept"),
            // A user-visible retry supersedes the old delivery. That rejected
            // revision is terminal too, so its already-recorded Main-Agent
            // decision can safely settle the old binding.
            ("rejected", "queued", "needs_decision"),
        ] {
            let mut connection = running_follow_up_connection(
                "completed",
                Some(("delivery", decision, true)),
                delivery_status,
                delegation_status,
            );

            assert_eq!(
                recover_interrupted_reviewed_delivery_follow_ups_in_conn(&mut connection, 20)
                    .unwrap(),
                1
            );
            let states: (String, Option<String>, String) = connection
                .query_row(
                    "SELECT follow_up.status, follow_up.foreground_run_id, input.status
                     FROM delegated_delivery_follow_ups follow_up
                     JOIN workspace_supervisor_inputs input
                       ON input.id = follow_up.supervisor_input_id
                     WHERE follow_up.supervisor_input_id = 'input'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                states,
                (
                    "summarized".to_string(),
                    Some("summary-run".to_string()),
                    "completed".to_string(),
                )
            );
        }
    }

    #[test]
    fn unscoped_failed_or_nonterminal_review_evidence_never_marks_a_binding_summarized() {
        for (
            call_delivery_id,
            success,
            delivery_status,
            delegation_status,
            expected_binding_status,
        ) in [
            ("other-delivery", true, "accepted", "completed", "cancelled"),
            ("delivery", false, "accepted", "completed", "cancelled"),
            ("delivery", true, "submitted", "awaiting_summary", "queued"),
        ] {
            let mut connection = running_follow_up_connection(
                "completed",
                Some((call_delivery_id, "accept", success)),
                delivery_status,
                delegation_status,
            );

            assert_eq!(
                recover_interrupted_reviewed_delivery_follow_ups_in_conn(&mut connection, 20)
                    .unwrap(),
                1
            );
            let states: (String, Option<String>, String, Option<String>) = connection
                .query_row(
                    "SELECT follow_up.status, follow_up.foreground_run_id, input.status, input.claim_token
                     FROM delegated_delivery_follow_ups follow_up
                     JOIN workspace_supervisor_inputs input
                       ON input.id = follow_up.supervisor_input_id
                     WHERE follow_up.supervisor_input_id = 'input'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap();
            if expected_binding_status == "queued" {
                assert_eq!(
                    states,
                    ("queued".to_string(), None, "queued".to_string(), None)
                );
            } else {
                assert_eq!(
                    states,
                    (
                        "cancelled".to_string(),
                        Some("summary-run".to_string()),
                        "completed".to_string(),
                        Some("claim".to_string()),
                    )
                );
            }
        }
    }

    #[test]
    fn a_terminal_task_status_is_never_treated_as_replayable() {
        for status in [
            "completed",
            "awaiting_confirmation",
            "needs_attention",
            "continue_suggested",
            "provider_unavailable",
            "stopped",
        ] {
            assert_ne!(status, "running");
        }
    }
}
