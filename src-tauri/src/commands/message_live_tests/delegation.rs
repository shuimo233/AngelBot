//! Real producer/Reviewer acceptance through the normal delegation graph.
//! Optional Main journey also exercises model-issued delegation, the normal
//! follow-up pump, and the durable confirmation continuation (not native UI).
//! It then stops another delegation and creates/cancels a one-off reminder;
//! the ordinary automation scheduler is never started.

use std::{path::Path, process::Command, time::Duration};

use crate::agent::{
    concrete_sandbox::production_components_with_provider,
    delegate_work::{DelegateWorkHandler, ForegroundRunIdentity},
    delegated_model_binding::DelegatedModelBindingRequest,
    delegated_model_host::KeychainDelegatedModelHostFactory,
    delegated_runtime_factory::DelegatedRuntimeFactory,
    delegation_pump::DelegationPumpHostHandle,
    explorer_runtime_config::{ExplorerOptIn, ExplorerRuntimeDependencies},
    resource_binding::{
        CleanupStatus as ResourceCleanupStatus, ResourceBindingRepository, ResourceKind,
    },
    shared_db::SharedDb,
    tool::{ToolCall, ToolHandler},
    ToolRegistry,
};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use tauri::Manager;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Journey {
    Direct,
    Main,
    Checkpoint,
    Resume,
    ProducerCrash,
    ProducerResume,
    ReviewerCrash,
    ReviewerResume,
    Concurrent,
}

pub(super) async fn verify_candidate(
    app: &tauri::AppHandle,
    config: &crate::llm::ProviderConfig,
    directory: &Path,
    receipt: &str,
    journey: Journey,
) {
    let through_main = journey != Journey::Direct;
    let after_restart = matches!(
        journey,
        Journey::Resume | Journey::ProducerResume | Journey::ReviewerResume
    );
    let checkpoint_path = directory.join("delegation-checkpoint.json");
    let checkpoint: Option<serde_json::Value> = after_restart
        .then(|| serde_json::from_slice(&std::fs::read(&checkpoint_path).unwrap()).unwrap());
    let workspace = directory.join("workspace");
    let input = if after_restart {
        let entries = std::fs::read_dir(workspace.join("briefs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        entries[0]
            .strip_prefix(&workspace)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    } else {
        std::fs::create_dir_all(workspace.join("briefs")).unwrap();
        format!("briefs/request-{}.txt", uuid::Uuid::new_v4())
    };
    let expected = format!("verified candidate: {receipt}\n");
    if !after_restart {
        std::fs::write(workspace.join(&input), &expected).unwrap();
        for args in [
            vec!["init"],
            vec!["config", "core.autocrlf", "false"],
            vec!["config", "user.name", "AngelBot Live Fixture"],
            vec!["config", "user.email", "fixture@example.invalid"],
            vec!["add", "briefs"],
            vec!["commit", "-m", "Synthetic input"],
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&workspace)
                .output()
                .unwrap()
                .status
                .success());
        }
    }
    let db = SharedDb::new(crate::db::init(&directory.join("data.db")).unwrap());
    db.with_conn(|conn| {
        crate::db::migrate(conn).unwrap();
        if !after_restart {
            conn.execute_batch("INSERT OR IGNORE INTO profile(id,updated_at) VALUES(1,1);
            INSERT INTO projects(id,name,path,kind,created_at) VALUES('live-delegate','Isolated delegation','','project',1);
            INSERT INTO sessions(id,title,work_dir,project_id,created_at,updated_at) VALUES('live-delegate','Isolated delegation','','live-delegate',1,1);
            UPDATE projects SET active_session_id='live-delegate' WHERE id='live-delegate';")?;
            if !through_main {
            conn.execute_batch("INSERT INTO messages(id,session_id,role,content,is_provisional,created_at) VALUES('live-parent','live-delegate','assistant','',1,1);
                INSERT INTO task_runs(id,session_id,message_id,goal,status,plan,created_at,updated_at) VALUES('live-parent','live-delegate','live-parent','Bounded candidate','running','[]',1,1);")?;
            }
            let root = crate::commands::file::display_work_dir(&workspace);
            conn.execute("UPDATE sessions SET work_dir=?1 WHERE id='live-delegate'", [&root])?;
            conn.execute("UPDATE projects SET path=?1 WHERE id='live-delegate'", [&root])?;
        }
        Ok::<_, rusqlite::Error>(())
    }).unwrap().unwrap();
    if after_restart {
        if journey == Journey::ReviewerResume {
            let checkpoint = checkpoint.as_ref().unwrap();
            let snapshot =
                review_snapshot(&db, checkpoint["issued"]["attempt_id"].as_str().unwrap());
            assert_eq!(
                snapshot["status"], "running",
                "reviewer must have been killed before a verdict, without shutdown requeueing it"
            );
            assert_eq!(
                snapshot["reviewer_attempt_id"],
                checkpoint["review"]["reviewer_attempt_id"]
            );
            assert_same_review_subject(&snapshot, &checkpoint["review"]);
            assert_eq!(snapshot["acceptance_status"], "submitted");
            assert!(!workspace.join("output/result.txt").exists());
        }
        if journey == Journey::ProducerResume {
            db.with_conn(|conn| {
                let attempt = checkpoint.as_ref().unwrap()["issued"]["attempt_id"]
                    .as_str()
                    .unwrap();
                let (status, deliveries): (String, i64) = conn.query_row(
                    "SELECT status,(SELECT count(*) FROM delegation_deliveries WHERE attempt_id=?1)
                     FROM delegation_attempts WHERE id=?1",
                    [attempt], |row| Ok((row.get(0)?,row.get(1)?)),
                ).unwrap();
                assert_eq!(
                    (status, deliveries),
                    ("running".into(), 0),
                    "the forced process loss must precede delivery, without graceful shutdown"
                );
            })
            .unwrap();
        }
        db.with_conn_mut(|conn| {
            let now = chrono::Utc::now().timestamp();
            crate::commands::foreground_run_store::recover_interrupted_runs_in_conn(conn, now)?;
            crate::commands::automation::recover_interrupted_agent_automation_dispatches_in_conn(conn, now)?;
            crate::commands::delegated_delivery_follow_up::recover_interrupted_reviewed_delivery_follow_ups_in_conn(conn, now)?;
            Ok::<_, String>(())
        }).unwrap().unwrap();
    }
    let (sandbox, cleanup, provider) =
        production_components_with_provider(db.clone(), directory).unwrap();
    let assembly = DelegatedRuntimeFactory::build(
        db.clone(),
        ExplorerOptIn::from_sources(Some("true"), None),
        ExplorerRuntimeDependencies::all_ready(),
        sandbox,
        cleanup,
        provider,
        KeychainDelegatedModelHostFactory::new(app.clone()),
    )
    .unwrap();
    assert!(assembly.enabled);
    let pump = assembly.into_pump();
    struct StopPump(DelegationPumpHostHandle);
    impl Drop for StopPump {
        fn drop(&mut self) {
            let _ = self.0.shutdown();
        }
    }
    let _stop = StopPump(pump.clone());
    const WORKER_GOAL: &str = "Find the one request file under briefs (its name is unknown), read it, and create output/result.txt with exactly those contents in the isolated candidate. After the final write, reread source and output to verify exact equality and cite actual readback evidence. Do not modify input, run commands, use network or request more work. Deliver the verified output candidate, not a promise to do it.";
    if through_main {
        let mut state = crate::tests::test_app_state();
        state.db = db.arc();
        state.delegation_pump = Some(pump.clone());
        state.delegation_available = true;
        crate::settings_cache::load_all(&state.db.lock().unwrap(), &state.cached_settings).unwrap();
        if !after_restart {
            crate::commands::message::set_agent_execution_permission_impl(&state, "ask").unwrap();
            crate::commands::memory::resolve_reply_memory_controls(
                &state.db.lock().unwrap(),
                "live-delegate",
                Some(false),
                Some(false),
            )
            .unwrap();
        } else {
            assert_eq!(
                crate::commands::memory::resolve_reply_memory_controls(
                    &state.db.lock().unwrap(),
                    "live-delegate",
                    None,
                    None
                )
                .unwrap(),
                (false, false)
            );
        }
        let follow_up = state.foreground_automation_pump.clone();
        let follow_up_app = app.clone();
        assert!(app.manage(state));
        assert_eq!(
            crate::commands::message::get_agent_execution_permission(app.state()).unwrap(),
            "ask"
        );
        pump.set_foreground_follow_up_waker(std::sync::Arc::new(move || {
            follow_up.wake(follow_up_app.clone());
        }));
        if journey != Journey::Concurrent {
            pump.start().unwrap();
        }
    }
    if journey == Journey::Concurrent {
        verify_concurrent(app, &db, &pump, directory, &workspace, receipt, WORKER_GOAL).await;
        return;
    }
    let issued = if let Some(checkpoint) = &checkpoint {
        assert_ne!(
            checkpoint["process_id"].as_u64().unwrap(),
            u64::from(std::process::id())
        );
        assert_eq!(checkpoint["provider"], config.provider);
        assert_eq!(checkpoint["model"], config.model);
        checkpoint["issued"].clone()
    } else if through_main {
        let reply = main_turn(app, &format!(
            "请把下面这项独立文件整理工作交给一个内部实现助手处理，你不要自己读写文件、执行命令或联网。只派发一次；派发后简洁告知已开始，等待结果回流，不要重复派发或直接写入正式项目。工作目标：{WORKER_GOAL}"
        )).await;
        assert_eq!(reply.task_run.as_ref().unwrap().status, "completed");
        let calls = reply.tool_results.as_ref().unwrap();
        let issued = calls
            .iter()
            .filter(|call| call.tool_name == "delegate_work")
            .collect::<Vec<_>>();
        assert_eq!(
            issued.len(),
            1,
            "Main must issue exactly one delegated task"
        );
        assert!(
            issued[0].success,
            "Main delegation failed: {}",
            issued[0].error.as_deref().unwrap_or("no diagnostic")
        );
        serde_json::from_str::<serde_json::Value>(&issued[0].output).unwrap()
    } else {
        let identity = db
            .with_conn(|conn| {
                ForegroundRunIdentity::prepare(
                    conn,
                    "live-delegate",
                    "live-parent",
                    workspace.clone(),
                )
            })
            .unwrap()
            .unwrap();
        let handler = DelegateWorkHandler::new(
            db.arc(),
            identity,
            directory.join("delegated/sandboxes"),
            DelegatedModelBindingRequest::new(
                config.profile_identity(),
                format!("model:{}", config.model),
                config.credential_identity(),
                1,
            )
            .unwrap(),
        );
        let issued = handler.execute(&serde_json::json!({
        "goal":WORKER_GOAL,
        "background_refs":["evidence://briefs"], "task_shape":"change", "worker_profile":"implementer",
    }), &workspace);
        assert!(
            issued.success,
            "bounded delegation must be issued: {}",
            issued.content
        );
        serde_json::from_str::<serde_json::Value>(&issued.content).unwrap()
    };
    let attempt = issued["attempt_id"].as_str().unwrap();
    pump.start().unwrap();
    if journey == Journey::ProducerCrash {
        // Observe only the fixture-owned provider root. Neither the worker nor
        // the production control plane receives a checkout path from this test.
        tokio::time::timeout(Duration::from_secs(420), async {
            loop {
                let (status,deliveries): (String,i64) = db.with_conn(|conn| conn.query_row(
                    "SELECT d.status,(SELECT count(*) FROM delegation_deliveries WHERE attempt_id=a.id)
                     FROM delegation_attempts a JOIN delegations d ON d.id=a.delegation_id WHERE a.id=?1", [attempt], |row| Ok((row.get(0)?,row.get(1)?)),
                )).unwrap().unwrap();
                assert_eq!(deliveries, 0, "producer completed before the intended crash point");
                assert!(matches!(status.as_str(),"queued"|"running"),"producer stopped before crash readiness: {status}");
                let active = directory.join("delegated/worktrees/active");
                let paths = std::fs::read_dir(&active).unwrap()
                    .map(|entry| entry.unwrap().path().join("output/result.txt"))
                    .filter(|path| path.is_file()).collect::<Vec<_>>();
                if paths.first().is_some_and(|path| std::fs::read_to_string(path).ok().as_deref() == Some(expected.as_str())) {
                    assert_eq!(paths.len(), 1);
                    assert!(!workspace.join("output/result.txt").exists());
                    hold_for_parent_kill(directory, serde_json::json!({
                        "process_id":std::process::id(), "provider":config.provider, "model":config.model,
                        "receipt":receipt, "crash_stage":"producer_after_candidate_write", "issued":issued,
                    })).await;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.expect("real producer did not reach the pre-delivery candidate-write crash point");
        unreachable!("the parent must forcibly kill this child, not return normally");
    }
    if journey == Journey::ProducerResume {
        verify_interrupted_producer(app, &db, &pump, &issued, &workspace, &input, &expected).await;
        return;
    }
    if journey == Journey::ReviewerResume {
        let checkpoint = checkpoint.as_ref().unwrap();
        let current = review_snapshot(&db, attempt);
        assert_same_review_subject(&current, &checkpoint["review"]);
        assert_ne!(
            current["reviewer_attempt_id"], checkpoint["review"]["reviewer_attempt_id"],
            "a restarted review must claim a fresh execution identity"
        );
        assert_ne!(current["reviewer_attempt_id"], attempt);
        // An old claim cannot replace the current review with even a failure.
        // This probe uses the actual coordinator and must make no state change.
        let coordinator = crate::agent::review_coordinator::ReviewCoordinator::new(db.clone());
        assert_eq!(
            coordinator
                .record_outcome(
                    current["job_id"].as_str().unwrap(),
                    checkpoint["review"]["reviewer_attempt_id"]
                        .as_str()
                        .unwrap(),
                    &crate::agent::review_contract::ReviewOutcome::execution_failed(
                        "synthetic stale reviewer"
                    ),
                    chrono::Utc::now().timestamp(),
                )
                .unwrap_err(),
            crate::agent::review_store::ReviewStoreError::InvalidTransition
        );
        assert!(matches!(
            review_snapshot(&db, attempt)["status"].as_str(),
            Some("running" | "passed")
        ));
    }
    let (delivery, reviewer) = tokio::time::timeout(Duration::from_secs(420), async {
        loop {
            let row = db.with_conn(|conn| conn.query_row(
                "SELECT delivery_id,status,reviewer_attempt_id FROM delegation_review_jobs WHERE implementation_attempt_id=?1",
                [attempt], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Option<String>>(2)?)),
            ).optional()).unwrap().unwrap();
            if let Some((delivery, status, reviewer)) = row {
                assert!(!matches!(status.as_str(), "failed" | "needs_decision"), "real Reviewer did not pass: {status}");
                if journey == Journey::ReviewerCrash && status == "running" {
                    let snapshot = review_snapshot(&db, attempt);
                    assert_eq!(snapshot["status"], "running", "review finished before the intended crash point");
                    assert_eq!(snapshot["acceptance_status"], "submitted");
                    assert_ne!(snapshot["reviewer_attempt_id"], attempt);
                    assert!(!workspace.join("output/result.txt").exists());
                    hold_for_parent_kill(directory, serde_json::json!({
                        "process_id":std::process::id(), "provider":config.provider, "model":config.model,
                        "receipt":receipt, "crash_stage":"reviewer_running", "issued":issued, "review":snapshot,
                    })).await;
                }
                assert!(journey != Journey::ReviewerCrash || status != "passed", "review completed before the intended crash point");
                if status == "passed" { break (delivery, reviewer.unwrap()); }
            }
            let state: String = db.with_conn(|conn| conn.query_row(
                "SELECT status FROM delegations WHERE id=?1", [issued["delegation_id"].as_str().unwrap()], |row| row.get(0),
            )).unwrap().unwrap();
            assert!(!matches!(state.as_str(), "failed" | "cancelled" | "needs_decision"), "real worker stopped without a passed review: {state}");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }).await.expect("real bounded producer/reviewer timed out");
    assert_ne!(reviewer, attempt, "producer must not audit itself");
    if journey == Journey::ReviewerResume {
        let current = review_snapshot(&db, attempt);
        assert_same_review_subject(&current, &checkpoint.as_ref().unwrap()["review"]);
        assert_eq!(current["status"], "passed");
        assert_eq!(current["acceptance_status"], "submitted");
    }
    let target = workspace.join("output/result.txt");
    assert!(
        !target.exists(),
        "passed review is not permission to write the real project"
    );
    if through_main {
        let pending = if journey == Journey::Resume {
            let checkpoint = checkpoint.as_ref().unwrap();
            assert_eq!(checkpoint["delivery_id"], delivery);
            assert_eq!(checkpoint["reviewer_attempt_id"], reviewer);
            let messages = crate::commands::foreground_run_store::ForegroundRunStore::new(db.arc())
                .project_session("live-delegate")
                .unwrap();
            assert_eq!(
                messages.len() as u64,
                checkpoint["message_count"].as_u64().unwrap(),
                "reopening must not invent a turn or summarize the same delivery again"
            );
            let pending = messages
                .into_iter()
                .find(|message| message.id == checkpoint["message_id"].as_str().unwrap())
                .unwrap();
            let call = pending
                .tool_results
                .iter()
                .flatten()
                .find(|call| call.confirmation_required)
                .unwrap();
            assert_eq!(call.call_id, checkpoint["call_id"].as_str().unwrap());
            assert_eq!(
                pending.task_run.as_ref().unwrap().status,
                "awaiting_confirmation"
            );
            pending
        } else {
            prepare_main_confirmation(app, &db, &delivery, &target).await
        };
        if journey == Journey::Checkpoint {
            pump.shutdown().unwrap();
            let messages = crate::commands::foreground_run_store::ForegroundRunStore::new(db.arc())
                .project_session("live-delegate")
                .unwrap();
            let call = pending
                .tool_results
                .iter()
                .flatten()
                .find(|call| call.confirmation_required)
                .unwrap();
            assert!(
                !target.exists(),
                "shutdown must not commit the pending candidate"
            );
            std::fs::write(&checkpoint_path, serde_json::to_vec(&serde_json::json!({
                "process_id":std::process::id(), "provider":config.provider, "model":config.model,
                "issued":{"delegation_id":issued["delegation_id"],"attempt_id":attempt},
                "delivery_id":delivery, "reviewer_attempt_id":reviewer,
                "message_id":pending.id, "call_id":call.call_id, "message_count":messages.len()
            })).unwrap()).unwrap();
            println!("LIVE_MAIN_CHECKPOINT verified: passed independent review, durable pending approval, canonical file absent, production service shut down; pid={}", std::process::id());
            return;
        }
        accept_main_confirmation(app, &db, &pending, &target, &expected).await;
        if after_restart {
            let counts = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT (SELECT count(*) FROM delegations),
                        (SELECT count(*) FROM delegation_attempts WHERE delegation_id=?1)",
                        [issued["delegation_id"].as_str().unwrap()],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                    )
                })
                .unwrap()
                .unwrap();
            assert_eq!(
                counts,
                (1, 1),
                "restart must not rerun producer or create a new work package"
            );
        }
    } else {
        let mut tools = ToolRegistry::empty();
        crate::commands::foreground_delegation_control::register(
            &mut tools,
            "live-delegate".into(),
            Some(pump.service().clone()),
        );
        assert!(
            tools
                .get("materialize_delegated_change")
                .unwrap()
                .requires_confirmation
        );
        // Explicit approval inside the disposable fixture, through the exact Main
        // control handler. No native UI or model-driven Main approval is implied.
        for (call_id, repeated) in [("approved-once", false), ("approval-replay", true)] {
            let result = tools.execute(
                &ToolCall::new(
                    "materialize_delegated_change".into(),
                    serde_json::json!({"delivery_id":delivery}),
                ),
                call_id,
                &workspace,
            );
            assert!(
                result.result.success,
                "confirmed candidate failed: {}",
                result.result.content
            );
            assert_eq!(result.result.content.contains("already_accepted"), repeated);
            assert_eq!(std::fs::read_to_string(&target).unwrap(), expected);
        }
    }
    assert_eq!(
        std::fs::read_to_string(workspace.join(&input)).unwrap(),
        expected
    );
    if journey == Journey::ReviewerResume {
        let current = review_snapshot(&db, attempt);
        assert_same_review_subject(&current, &checkpoint.as_ref().unwrap()["review"]);
        assert_eq!(current["acceptance_status"], "accepted");
    }
    let repository = ResourceBindingRepository::new(db);
    for kind in [ResourceKind::Sandbox, ResourceKind::Worktree] {
        assert_eq!(
            repository
                .load_for_attempt(attempt, kind)
                .unwrap()
                .cleanup_status,
            ResourceCleanupStatus::Cleaned
        );
    }
    if journey == Journey::Main {
        verify_daily_controls(app, &workspace, &target, &expected).await;
    }
    if journey == Journey::Resume {
        println!("LIVE_MAIN_RESTART verified: independent process, normal startup recovery, exact persisted approval identity, no automatic write or duplicate dispatch/summary, approved readback, replay refusal and terminal resource cleanup; pid={}", std::process::id());
    }
    if journey == Journey::ReviewerResume {
        println!("LIVE_MAIN_REVIEW_CRASH_RECOVERY verified: forced loss with running review, fresh reviewer identity, rejected stale claim, immutable delivery/subject, no producer replay, normal Main follow-up, explicit approval/readback/replay refusal and resource cleanup; pid={}", std::process::id());
    }
    println!("LIVE_DELEGATION verified: real producer discovery/read/candidate, distinct real Reviewer, confirmed exact materialization, replay-safe receipt, terminal cleanup; through_main={through_main}");
}

/// Shared crash rendezvous for this test's two genuinely different kill points.
/// Publish atomically; never unwind the pump or gracefully stop the service.
async fn hold_for_parent_kill(directory: &Path, checkpoint: serde_json::Value) {
    let temporary = directory.join("delegation-checkpoint.tmp");
    std::fs::write(&temporary, serde_json::to_vec(&checkpoint).unwrap()).unwrap();
    std::fs::rename(temporary, directory.join("delegation-checkpoint.json")).unwrap();
    println!(
        "LIVE_CRASH_READY stage={} canonical absent; waiting for parent kill; pid={}",
        checkpoint["crash_stage"],
        std::process::id()
    );
    std::future::pending::<()>().await;
}

/// Only fixture-owned identities/digests cross the process boundary. Payloads
/// and review subjects must stay immutable while the ephemeral claim changes.
fn review_snapshot(db: &SharedDb, attempt: &str) -> serde_json::Value {
    db.with_conn(|conn| {
        conn.query_row(
            "SELECT j.id,j.status,j.reviewer_attempt_id,j.delivery_id,j.delivery_revision,
                j.subject_json,d.payload_json,d.acceptance_status,
                (SELECT count(*) FROM delegation_review_jobs WHERE implementation_attempt_id=?1)
         FROM delegation_review_jobs j JOIN delegation_deliveries d
           ON d.id=j.delivery_id AND d.delivery_version=j.delivery_revision
         WHERE j.implementation_attempt_id=?1 ORDER BY j.delivery_revision DESC LIMIT 1",
            [attempt],
            |row| {
                let subject: String = row.get(5)?;
                let payload: String = row.get(6)?;
                Ok(serde_json::json!({
                    "job_id":row.get::<_,String>(0)?, "status":row.get::<_,String>(1)?,
                    "reviewer_attempt_id":row.get::<_,Option<String>>(2)?,
                    "delivery_id":row.get::<_,String>(3)?, "delivery_revision":row.get::<_,i64>(4)?,
                    "subject_sha256":format!("{:x}",Sha256::digest(subject.as_bytes())),
                    "delivery_sha256":format!("{:x}",Sha256::digest(payload.as_bytes())),
                    "acceptance_status":row.get::<_,String>(7)?, "job_count":row.get::<_,i64>(8)?,
                }))
            },
        )
    })
    .unwrap()
    .unwrap()
}

fn assert_same_review_subject(current: &serde_json::Value, previous: &serde_json::Value) {
    for field in [
        "job_id",
        "delivery_id",
        "delivery_revision",
        "subject_sha256",
        "delivery_sha256",
    ] {
        assert_eq!(
            current[field], previous[field],
            "review restart changed {field}"
        );
    }
    assert_eq!(
        current["job_count"], 1,
        "recovery must reclaim the same review job, not enqueue a duplicate"
    );
}

/// A fresh process recovers production state; it never patches task rows or
/// replays the old producer to make the acceptance pass.
async fn verify_interrupted_producer(
    app: &tauri::AppHandle,
    db: &SharedDb,
    pump: &DelegationPumpHostHandle,
    issued: &serde_json::Value,
    workspace: &Path,
    input: &str,
    expected: &str,
) {
    let attempt = issued["attempt_id"].as_str().unwrap();
    let delegation = issued["delegation_id"].as_str().unwrap();
    let assert_recovery = || {
        db.with_conn(|conn| {
            let state: (String,String,i64,i64,i64,i64,i64) = conn.query_row(
                "SELECT d.status,a.status,
                    (SELECT count(*) FROM delegation_capability_leases WHERE attempt_id=a.id AND status='active'),
                    (SELECT count(*) FROM delegation_deliveries WHERE attempt_id=a.id),
                    (SELECT count(*) FROM delegation_attempts WHERE delegation_id=d.id),
                    (SELECT count(*) FROM delegations WHERE session_id=d.session_id),
                    (SELECT count(*) FROM workspace_admissions WHERE attempt_id=a.id AND status='active')
                 FROM delegations d JOIN delegation_attempts a ON a.delegation_id=d.id
                 WHERE d.id=?1 AND a.id=?2",
                (delegation,attempt), |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
            ).unwrap();
            assert_eq!(state, ("needs_decision".into(),"sealed".into(),0,0,1,1,0));
            assert_eq!(conn.query_row::<i64,_,_>(
                "SELECT count(*) FROM delegation_attempt_events WHERE attempt_id=?1 AND event_type='recovery_unknown_effects'",
                [attempt], |row| row.get(0),
            ).unwrap(),1);
            let failures = crate::agent::supervision::delegation_activity_adapter::runtime_failures_for_session(conn,"live-delegate").unwrap();
            assert_eq!(failures.len(),1);
            assert_eq!(failures[0].reason,"execution_interrupted");
            let visible = crate::agent::supervision::delegation_activity_adapter::get_for_session(conn,"live-delegate").unwrap();
            assert_eq!(visible.delegations[0].summary.as_deref(),Some(failures[0].summary));
            assert!(!visible.pending_decisions[0].questions.is_empty());
        }).unwrap();
        assert_eq!(pump.service().active_worker_attempts().unwrap(), 0);
        assert!(!workspace.join("output/result.txt").exists());
        assert_eq!(
            std::fs::read_to_string(workspace.join(input)).unwrap(),
            expected
        );
        let resources = ResourceBindingRepository::new(db.clone());
        for kind in [ResourceKind::Sandbox, ResourceKind::Worktree] {
            assert_eq!(
                resources
                    .load_for_attempt(attempt, kind)
                    .unwrap()
                    .cleanup_status,
                ResourceCleanupStatus::Cleaned
            );
        }
    };
    assert_recovery();
    let report = main_turn(app,
        "刚才应用中断并重新打开了。请核对原来那项文件整理委派的状态，说明是否有已核验的结果、下一步该怎么处理。不要重新派发、重试、写入或应用旧候选。"
    ).await;
    assert_eq!(report.task_run.as_ref().unwrap().status, "completed");
    assert!(
        ["中断", "未核验", "尚未", "不确定", "无法确认", "未知"]
            .iter()
            .any(|word| report.content.contains(word)),
        "Main must describe uncertainty, not claim the interrupted task completed"
    );
    let next = main_turn(
        app,
        &format!(
        "先不处理旧委派。现在请只读打开 {input}，告诉我文件实际内容，不改文件，也不要派发助手。"
    ),
    )
    .await;
    assert_eq!(next.task_run.as_ref().unwrap().status, "completed");
    assert!(next
        .tool_results
        .iter()
        .flatten()
        .any(|call| call.tool_name == "read_file" && call.success));
    assert!(next.content.contains(expected.trim()));
    for reply in [&report, &next] {
        for call in reply.tool_results.iter().flatten() {
            assert!(
                matches!(
                    call.tool_name.as_str(),
                    "read_file"
                        | "list_workspace_items"
                        | "list_delegated_work"
                        | "think"
                        | "check_constraint"
                        | "verify_result"
                ),
                "restart follow-up must remain read-only: {}",
                call.tool_name
            );
        }
    }
    assert_recovery();
    println!("LIVE_MAIN_CRASH_RECOVERY verified: distinct process after forced producer loss, one sealed attempt/no delivery, leases revoked, admission released, no replay or canonical write, Main explains uncertainty, next read task completes, isolated resources cleaned; pid={}",std::process::id());
}

/// Natural user requests, with normal model selection of the control tools.
/// All approvals concern this disposable DB/project; no scheduler is started.
async fn verify_daily_controls(
    app: &tauri::AppHandle,
    workspace: &Path,
    target: &Path,
    expected: &str,
) {
    let state = app.state::<crate::AppState>();
    let queued = main_turn(app,
        "请另派一个内部实现助手做一项独立工作，任务叫“临时备用副本”：找到 briefs 中的申请说明，把内容原样保存为候选 output/discard.txt，保存后重新读取来源和结果核对。由助手处理，你不要自己读写或执行命令；只派发一次，先告诉我已开始，等结果回来再决定，不要写入正式项目。"
    ).await;
    assert_eq!(queued.task_run.as_ref().unwrap().status, "completed");
    let issued = queued
        .tool_results
        .iter()
        .flatten()
        .filter(|call| call.tool_name == "delegate_work")
        .collect::<Vec<_>>();
    assert_eq!(
        issued.len(),
        1,
        "Main must dispatch one bounded task, not perform or duplicate it"
    );
    assert!(issued[0].success);
    let issued: serde_json::Value = serde_json::from_str(&issued[0].output).unwrap();
    let delegation_id = issued["delegation_id"].as_str().unwrap();
    let attempt_id = issued["attempt_id"].as_str().unwrap();
    let stopped = main_turn(app,
        "先停掉刚派发的“临时备用副本”，这项工作不需要了。不要另派助手，也不要把它的候选写入项目；保留之前已经完成的 output/result.txt。"
    ).await;
    assert_eq!(stopped.task_run.as_ref().unwrap().status, "completed");
    let stop = stopped
        .tool_results
        .iter()
        .flatten()
        .find(|call| call.tool_name == "stop_delegated_work")
        .expect("a natural stop request must reach the actual control, not merely promise to stop");
    assert!(
        stop.success,
        "{}",
        stop.error.as_deref().unwrap_or("stop failed")
    );
    let stop: serde_json::Value = serde_json::from_str(&stop.output).unwrap();
    assert_eq!(stop["delegation_id"], delegation_id);
    assert_eq!(stop["attempt_id"], attempt_id);
    assert_eq!(stop["status"], "stopped");
    assert_eq!(stop["cleanup_pending"], false);
    {
        let conn = state.db.lock().unwrap();
        let status: (String, String, i64) = conn.query_row(
            "SELECT d.status,a.status,(SELECT count(*) FROM delegation_capability_leases WHERE attempt_id=a.id AND status='active')
             FROM delegations d JOIN delegation_attempts a ON a.delegation_id=d.id WHERE d.id=?1 AND a.id=?2",
            (delegation_id, attempt_id), |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).unwrap();
        assert_eq!(status, ("cancelled".into(), "sealed".into(), 0));
        assert_eq!(conn.query_row::<i64,_,_>(
            "SELECT count(*) FROM delegation_review_jobs WHERE implementation_attempt_id=?1 AND status IN ('queued','running')",
            [attempt_id], |row| row.get(0)).unwrap(), 0);
    }
    let resources = ResourceBindingRepository::new(SharedDb::from_arc(state.db.clone()));
    for kind in [ResourceKind::Sandbox, ResourceKind::Worktree] {
        assert_eq!(
            resources
                .load_for_attempt(attempt_id, kind)
                .unwrap()
                .cleanup_status,
            ResourceCleanupStatus::Cleaned
        );
    }
    assert!(!workspace.join("output/discard.txt").exists());
    assert_eq!(std::fs::read_to_string(target).unwrap(), expected);
    let next = main_turn(app,
        "备用副本取消后，接着帮我重新打开 output/result.txt 核对内容，把实际内容告诉我。不要修改文件，也不要重新派发已取消的工作。"
    ).await;
    assert_eq!(next.task_run.as_ref().unwrap().status, "completed");
    assert!(next
        .tool_results
        .iter()
        .flatten()
        .any(|call| call.tool_name == "read_file" && call.success));
    assert!(next.content.contains(expected.trim()));
    println!("LIVE_MAIN_STOP verified: natural target selection, exact current attempt, revocation, reviewer cancellation, isolated cleanup, completed output retained, next file task");

    let started = chrono::Utc::now().timestamp();
    let reminder = main_turn(app,
        "一小时后提醒我核对办公支出，标题用“核对办公支出”，内容是“检查已付和待付金额”。只设置一次提醒，不要重复日程。"
    ).await;
    assert_eq!(
        state
            .db
            .lock()
            .unwrap()
            .query_row::<i64, _, _>("SELECT count(*) FROM automations", [], |row| row.get(0))
            .unwrap(),
        0,
        "no reminder may exist before its normal confirmation"
    );
    approve_daily_action(app, &reminder, "schedule_reminder").await;
    let (id, next_run): (String, i64) = state.db.lock().unwrap().query_row(
        "SELECT id,next_run_at FROM automations WHERE executor_kind='notification' AND trigger_kind='once' AND enabled=1",
        [], |row| Ok((row.get(0)?,row.get(1)?)),
    ).unwrap();
    assert!(
        next_run >= started + 3500 && next_run <= chrono::Utc::now().timestamp() + 3700,
        "one hour must use the current time and timezone, not a guessed timestamp"
    );
    let cancelled = main_turn(
        app,
        "刚才那个“核对办公支出”的提醒也取消吧，不需要再提醒了。不要改动文件或创建别的日程。",
    )
    .await;
    assert!(
        state
            .db
            .lock()
            .unwrap()
            .query_row::<bool, _, _>(
                "SELECT enabled FROM automations WHERE id=?1",
                [&id],
                |row| row.get(0)
            )
            .unwrap(),
        "cancellation must not execute before confirmation"
    );
    approve_daily_action(app, &cancelled, "cancel_reminder").await;
    assert_eq!(
        state
            .db
            .lock()
            .unwrap()
            .query_row::<bool, _, _>(
                "SELECT enabled FROM automations WHERE id=?1",
                [&id],
                |row| row.get(0)
            )
            .unwrap(),
        false
    );
    assert_eq!(
        state
            .db
            .lock()
            .unwrap()
            .query_row::<i64, _, _>("SELECT count(*) FROM automation_runs", [], |row| row.get(0))
            .unwrap(),
        0,
        "this journey tests reminder setup/cancel, never actual notification delivery"
    );
    println!("LIVE_MAIN_REMINDER verified: relative time, one-off notification intent, normal approval, lookup/cancel by title, no scheduler or OS notification");
}

async fn approve_daily_action(
    app: &tauri::AppHandle,
    pending: &crate::commands::message::Message,
    tool_name: &str,
) {
    assert_eq!(
        pending.task_run.as_ref().unwrap().status,
        "awaiting_confirmation"
    );
    let call = pending
        .tool_results
        .iter()
        .flatten()
        .find(|call| call.confirmation_required)
        .unwrap();
    assert_eq!(call.tool_name, tool_name);
    let state = app.state::<crate::AppState>();
    let result = tokio::time::timeout(
        Duration::from_secs(180),
        crate::commands::message::resolve_agent_confirmation_and_continue(
            &state,
            Some(app),
            crate::commands::message::ResolveAgentConfirmationRequest {
                session_id: "live-delegate".into(),
                message_id: pending.id.clone(),
                call_id: call.call_id.clone(),
                decision: "approved".into(),
                preview_id: None,
            },
        ),
    )
    .await
    .expect("daily action confirmation timed out")
    .unwrap();
    assert!(result.success);
}

async fn verify_concurrent(
    app: &tauri::AppHandle,
    db: &SharedDb,
    pump: &DelegationPumpHostHandle,
    directory: &Path,
    workspace: &Path,
    receipt: &str,
    worker_goal: &str,
) {
    let other = directory.join("other-workspace");
    std::fs::create_dir_all(other.join("briefs")).unwrap();
    let other_expected = format!("other isolated project: {receipt}\n");
    std::fs::write(other.join("briefs/request.txt"), &other_expected).unwrap();
    for args in [
        vec!["init"],
        vec!["config", "core.autocrlf", "false"],
        vec!["config", "user.name", "AngelBot Live Fixture"],
        vec!["config", "user.email", "fixture@example.invalid"],
        vec!["add", "briefs"],
        vec!["commit", "-m", "Other synthetic input"],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&other)
            .output()
            .unwrap()
            .status
            .success());
    }
    db.with_conn(|conn| {
        let path = crate::commands::file::display_work_dir(&other);
        conn.execute("INSERT INTO projects(id,name,path,kind,active_session_id,created_at) VALUES('live-other','Other isolated project',?1,'project',NULL,1)", [&path])?;
        conn.execute("INSERT INTO sessions(id,title,work_dir,project_id,created_at,updated_at) VALUES('live-other','Other isolated project',?1,'live-other',1,1)", [&path])?;
        conn.execute("UPDATE projects SET active_session_id='live-other' WHERE id='live-other'", [])?;
        crate::commands::memory::resolve_reply_memory_controls(conn, "live-other", Some(false), Some(false)).unwrap();
        Ok::<_, rusqlite::Error>(())
    }).unwrap().unwrap();
    let mut issued = Vec::new();
    for session in ["live-delegate", "live-other"] {
        let reply = main_turn_for_session(app, session, &format!("将以下工作只派发一次给内部实现助手，你不要自行读写或执行命令。派发后告知已开始即可，等待结果回流，不重复派发、不写入正式项目。工作目标：{worker_goal}")).await;
        assert_eq!(reply.task_run.as_ref().unwrap().status, "completed");
        let calls = reply
            .tool_results
            .iter()
            .flatten()
            .filter(|call| call.tool_name == "delegate_work")
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].success,
            "independent Main dispatch failed: {:?}",
            calls[0].error
        );
        issued.push(serde_json::from_str::<serde_json::Value>(&calls[0].output).unwrap());
    }
    // Both real Main turns enqueue before the normal host starts, eliminating
    // dispatch timing as the reason the second worker would be missing.
    pump.start().unwrap();
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let active: i64 = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT count(*) FROM delegation_attempts WHERE status='running'",
                        [],
                        |row| row.get(0),
                    )
                })
                .unwrap()
                .unwrap();
            if active == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("two independent project workers must overlap in running state");
    let service = pump.service();
    let stopped = service
        .stop_for_session(
            "live-other",
            issued[1]["delegation_id"].as_str().unwrap(),
            issued[1]["attempt_id"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(
        stopped.status,
        crate::agent::delegation_service::DelegationStopStatus::Stopped
    );
    assert!(
        service
            .stop_for_session(
                "live-other",
                issued[0]["delegation_id"].as_str().unwrap(),
                issued[0]["attempt_id"].as_str().unwrap()
            )
            .is_err(),
        "one project's stop cannot address the other project's attempt"
    );
    let attempt = issued[0]["attempt_id"].as_str().unwrap();
    let delivery: String = tokio::time::timeout(Duration::from_secs(420), async {
        loop {
            let row = db.with_conn(|conn| conn.query_row("SELECT d.id FROM delegation_deliveries d JOIN delegation_review_jobs j ON j.delivery_id=d.id AND j.delivery_revision=d.delivery_version WHERE d.attempt_id=?1 AND j.status='passed'", [attempt], |row| row.get::<_, String>(0)).optional()).unwrap().unwrap();
            if let Some(id) = row { break id; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }).await.expect("the surviving producer and its independent Reviewer must finish");
    let target = workspace.join("output/result.txt");
    let pending = prepare_main_confirmation(app, db, &delivery, &target).await;
    accept_main_confirmation(
        app,
        db,
        &pending,
        &target,
        &format!("verified candidate: {receipt}\n"),
    )
    .await;
    assert!(
        !other.join("output/result.txt").exists(),
        "stopping another project must not apply its candidate"
    );
    assert_eq!(
        std::fs::read_to_string(other.join("briefs/request.txt")).unwrap(),
        other_expected
    );
    let counts: (i64, i64, i64) = db.with_conn(|conn| conn.query_row("SELECT (SELECT count(*) FROM delegations),(SELECT count(*) FROM delegation_attempts),(SELECT count(*) FROM workspace_admissions WHERE status='active')", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))).unwrap().unwrap();
    assert_eq!(counts, (2, 2, 0));
    for task in &issued {
        for kind in [ResourceKind::Sandbox, ResourceKind::Worktree] {
            let binding = ResourceBindingRepository::new(db.clone())
                .load_for_attempt(task["attempt_id"].as_str().unwrap(), kind)
                .unwrap();
            assert_eq!(binding.cleanup_status, ResourceCleanupStatus::Cleaned);
        }
    }
    println!("LIVE_MAIN_CONCURRENT verified: two Main-scoped project dispatches, overlapping real workers, scoped cancellation without harming the survivor, independent review, normal Main follow-up, explicit apply/readback/replay refusal and both resource cleanups");
}

async fn main_turn(app: &tauri::AppHandle, content: &str) -> crate::commands::message::Message {
    main_turn_for_session(app, "live-delegate", content).await
}

async fn main_turn_for_session(
    app: &tauri::AppHandle,
    session: &str,
    content: &str,
) -> crate::commands::message::Message {
    let state = app.state::<crate::AppState>();
    tokio::time::timeout(
        Duration::from_secs(180),
        crate::commands::message::send_message_impl(
            &state,
            Some(app),
            crate::commands::message::SendMessageRequest {
                session_id: session.into(),
                client_message_id: None,
                role: "user".into(),
                content: content.into(),
                text_attachments: vec![],
                personality: None,
                preferences: None,
                thinking_effort: Some("low".into()),
                web_search_setup: None,
            },
            None,
            crate::commands::message::ForegroundUserTurn::Append,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("bounded Main turn timed out")
    .expect("Main turn must execute")
}

async fn prepare_main_confirmation(
    app: &tauri::AppHandle,
    db: &SharedDb,
    delivery: &str,
    target: &Path,
) -> crate::commands::message::Message {
    let run_id = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let row = db.with_conn(|conn| conn.query_row(
                "SELECT status,foreground_run_id FROM delegated_delivery_follow_ups WHERE delivery_id=?1",
                [delivery], |row| Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?)),
            ).optional()).unwrap().unwrap();
            if let Some((status, run_id)) = row {
                assert_ne!(status, "cancelled", "automatic Main summary was cancelled");
                if status == "summarized" { break run_id.unwrap(); }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }).await.expect("reviewed candidate must wake the normal Main follow-up pump");
    let store = crate::commands::foreground_run_store::ForegroundRunStore::new(db.arc());
    let messages = store.project_session("live-delegate").unwrap();
    let summary = messages
        .iter()
        .find(|message| message.id == run_id)
        .unwrap();
    assert!(!summary.content.trim().is_empty());
    assert_eq!(summary.task_run.as_ref().unwrap().status, "completed");
    assert!(
        summary
            .tool_results
            .as_ref()
            .is_none_or(|calls| calls.is_empty()),
        "automatic Change summaries must remain text-only"
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1,
        "delivery follow-up must not invent another user turn"
    );
    assert!(
        !target.exists(),
        "automatic summary must not materialize candidates"
    );
    let pending = main_turn(app, "请将刚才通过独立审校的 output/result.txt 候选应用到当前项目。只应用这份审校过的候选，不另派助手、不自行重新生成文件。应用后重新打开结果并核对原始内容，汇报结果。").await;
    assert_eq!(
        pending.task_run.as_ref().unwrap().status,
        "awaiting_confirmation"
    );
    assert!(
        !target.exists(),
        "canonical write must wait for the durable confirmation"
    );
    let call = pending
        .tool_results
        .as_ref()
        .unwrap()
        .iter()
        .find(|call| call.confirmation_required)
        .unwrap();
    assert_eq!(call.tool_name, "materialize_delegated_change");
    pending
}

async fn accept_main_confirmation(
    app: &tauri::AppHandle,
    db: &SharedDb,
    pending: &crate::commands::message::Message,
    target: &Path,
    expected: &str,
) {
    let state = app.state::<crate::AppState>();
    let store = crate::commands::foreground_run_store::ForegroundRunStore::new(db.arc());
    let call = pending
        .tool_results
        .iter()
        .flatten()
        .find(|call| call.confirmation_required)
        .unwrap();
    assert_eq!(call.tool_name, "materialize_delegated_change");
    assert!(
        !target.exists(),
        "confirmation must precede the canonical write"
    );
    let approval = crate::commands::message::ResolveAgentConfirmationRequest {
        session_id: "live-delegate".into(),
        message_id: pending.id.clone(),
        call_id: call.call_id.clone(),
        decision: "approved".into(),
        preview_id: None,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(180),
        crate::commands::message::resolve_agent_confirmation_and_continue(
            &state,
            Some(app),
            approval.clone(),
        ),
    )
    .await
    .expect("Main confirmation continuation timed out")
    .unwrap();
    assert!(result.success);
    assert_eq!(std::fs::read_to_string(target).unwrap(), expected);
    let messages = store.project_session("live-delegate").unwrap();
    let resumed_id: String = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT id FROM messages WHERE session_id='live-delegate' AND role='assistant'
         AND json_extract(metadata,'$.angelbotContinuation.parentMessageId')=?1",
                [&pending.id],
                |row| row.get(0),
            )
        })
        .unwrap()
        .unwrap();
    let completed = messages
        .iter()
        .find(|message| message.id == resumed_id)
        .unwrap();
    assert_eq!(completed.task_run.as_ref().unwrap().status, "completed");
    assert!(completed
        .tool_results
        .as_ref()
        .unwrap()
        .iter()
        .any(|call| call.tool_name == "read_file" && call.success));
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        2
    );
    assert!(
        crate::commands::message::resolve_agent_confirmation_and_continue(
            &state,
            Some(app),
            approval
        )
        .await
        .is_err()
    );
    assert_eq!(
        store.project_session("live-delegate").unwrap().len(),
        messages.len()
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), expected);
    println!("LIVE_MAIN_DELEGATION verified: model-issued once, automatic text-only follow-up, durable approval continuation, readback, no synthetic user turn or replay");
}
