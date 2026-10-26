//! Opt-in model-boundary fault: real provider before/after, no proxy or auth edits.
use super::*;
use crate::commands::foreground_context_assembler::{
    ForegroundContextAssembler, ForegroundContextBudget, ForegroundContextRequest,
};
use crate::commands::foreground_model_factory::{ForegroundModelFactory, ForegroundModelRequest};
use crate::commands::foreground_turn_coordinator::{
    ForegroundTurnCoordinator, ForegroundTurnInput, ForegroundTurnToolMode,
};

pub(super) async fn send_after_known_effect(
    app: &AppHandle,
    state: &AppState,
    request: SendMessageRequest,
    receipt: &str,
) -> Message {
    let failure =
        std::env::var("ANGELBOT_LIVE_MODEL_FAILURE").unwrap_or_else(|_| "synthetic".into());
    assert!(matches!(
        failure.as_str(),
        "synthetic" | "auth-reject" | "stop-pending"
    ));
    let stop_pending = failure == "stop-pending";
    let pending_started = Arc::new(tokio::sync::Notify::new());
    let pending_dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let rejected_provider: Option<Box<dyn crate::llm::LlmProvider>> = if failure == "auth-reject" {
        let mut config =
            crate::commands::foreground_model_factory::resolve_model_config(Some(app)).unwrap();
        assert_eq!(config.auth_mode, crate::llm::ModelAuthMode::ChatgptPlan);
        // An intentionally invalid, task-owned source, not a modified/revoked
        // user credential. The different binding also excludes opaque account
        // continuation data from this rejected request. Endpoint stays public.
        config.api_key.clear();
        config.credential_ref = Some("owned-rejected-credential".into());
        Some(Box::new(
            crate::llm::OpenAiResponsesProvider::new(
                &config,
                crate::llm::auth::StaticCredentialSource::new(
                    "owned-rejected-token-canary".into(),
                    "owned-rejected-credential".into(),
                ),
            )
            .unwrap(),
        ))
    } else {
        None
    };
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct FailAfterReceipt {
        inner: Box<dyn crate::llm::LlmProvider>,
        receipt: String,
        rejected_provider: Option<Box<dyn crate::llm::LlmProvider>>,
        attempts: Arc<std::sync::atomic::AtomicUsize>,
        stop_pending: bool,
        pending_started: Arc<tokio::sync::Notify>,
        pending_dropped: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait::async_trait]
    impl crate::llm::LlmProvider for FailAfterReceipt {
        fn name(&self) -> &str {
            self.inner.name()
        }
        fn supports_tool_images(&self) -> bool {
            self.inner.supports_tool_images()
        }
        fn latest_usage(&self) -> Option<crate::llm::ProviderUsage> {
            self.inner.latest_usage()
        }
        async fn chat(
            &self,
            messages: &[LlmMessage],
            tools: Option<&[crate::llm::ToolSchema]>,
            thinking: Option<&crate::llm::ThinkingSettings>,
        ) -> Result<crate::llm::LlmResponse, String> {
            if messages.iter().any(|message| {
                message.role == "tool"
                    && message.content.contains(&self.receipt)
                    && message.content.contains("\"count\":1")
            }) {
                assert_eq!(
                    self.attempts
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                    0,
                    "the rejected inference must not be retried"
                );
                if self.stop_pending {
                    struct PendingDrop(Arc<std::sync::atomic::AtomicBool>);
                    impl Drop for PendingDrop {
                        fn drop(&mut self) {
                            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    let _pending = PendingDrop(self.pending_dropped.clone());
                    self.pending_started.notify_one();
                    return std::future::pending().await;
                }
                if let Some(provider) = &self.rejected_provider {
                    let error = match provider.chat(messages, tools, thinking).await {
                        Err(error) => error,
                        Ok(_) => panic!("owned invalid token unexpectedly accepted"),
                    };
                    assert!(
                        error.contains("HTTP 401"),
                        "expected actual public API admission rejection"
                    );
                    assert!(!error.contains("owned-rejected-token-canary"));
                    println!("LIVE_MODEL_AUTH_REJECT status=401 attempts=1 public_responses=true owned_invalid_source=true");
                    return Err(error);
                }
                return Err("synthetic model outage: Bearer private-model-fault https://private.example/?input=secret".into());
            }
            self.inner.chat(messages, tools, thinking).await
        }
    }

    // Exercise the existing coordinator seam with one decorated provider. No
    // production config switch, global fault hook, transport proxy or copied key.
    let (_permit, cancellation) = state
        .steering
        .try_start_running_run(&request.session_id)
        .unwrap();
    let now = Utc::now().timestamp();
    let reply_id = Uuid::new_v4().to_string();
    let session_id = request.session_id.clone();
    let goal = request.content.clone();
    let store = ForegroundRunStore::new(state.db.clone());
    let run = store
        .begin(BeginForegroundTurn {
            session_id: session_id.clone(),
            user_message_id: Uuid::new_v4().to_string(),
            reply_id: reply_id.clone(),
            role: "user".into(),
            content: goal.clone(),
            text_attachments: vec![],
            user_turn: ForegroundUserTurn::Append,
            now,
        })
        .unwrap();
    let prepared_context = ForegroundContextAssembler::prepare(ForegroundContextRequest {
        state,
        app: Some(app),
        request: &request,
        history_before_created_at: run.history_before_created_at,
        budget: ForegroundContextBudget {
            estimated_tool_count: ESTIMATED_TOOL_COUNT,
        },
        summary_delivery_id: None,
    })
    .await
    .unwrap();
    let mut resolved_model = ForegroundModelFactory::new(Some(app), state)
        .resolve(ForegroundModelRequest {
            session_id: &session_id,
            requested_thinking_effort: Some("low"),
            now,
        })
        .unwrap();
    resolved_model.provider = Box::new(FailAfterReceipt {
        inner: resolved_model.provider,
        receipt: receipt.into(),
        rejected_provider,
        attempts: attempts.clone(),
        stop_pending,
        pending_started: pending_started.clone(),
        pending_dropped: pending_dropped.clone(),
    });
    let events = Arc::new(crate::agent::event::DevEventStore::default());
    let lifecycle = Arc::new(ForegroundLifecycleControl::new(
        events.clone(),
        state.db.clone(),
        reply_id.clone(),
    ));
    let coordinator = ForegroundTurnCoordinator::new(state, Some(app), lifecycle, store);
    let execution = coordinator.run(ForegroundTurnInput {
        request,
        goal,
        session_id: session_id.clone(),
        message_id: reply_id.clone(),
        reply_id,
        prepared_context,
        resolved_model,
        foreground_run: run,
        delegation_identity: None,
        sandbox_root: None,
        delegated_model_binding: None,
        tool_mode: ForegroundTurnToolMode::Standard,
        after_completion: None,
        confirmed_output: None,
        cancellation,
    });
    tokio::pin!(execution);
    if stop_pending {
        tokio::time::timeout(Duration::from_secs(180), async {
            tokio::select! {
                result = &mut execution => panic!("pending inference completed before Stop: {result:?}"),
                _ = pending_started.notified() => {},
            }
        }).await.expect("real Luna must first produce the owned effect receipt");
        crate::commands::steering::interrupt_agent(app.state::<AppState>(), session_id.clone())
            .unwrap();
    }
    let reply = tokio::time::timeout(
        Duration::from_secs(if stop_pending { 2 } else { 180 }),
        &mut execution,
    )
    .await
    .expect("model-fault coordinator timed out")
    .unwrap();
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    let visible = format!(
        "{}{}",
        serde_json::to_string(&reply).unwrap(),
        serde_json::to_string(&events.events_for(&session_id)).unwrap()
    );
    for secret in [
        "private-model-fault",
        "private.example",
        "input=secret",
        "owned-rejected-token-canary",
        "owned-rejected-credential",
    ] {
        assert!(
            !visible.contains(secret),
            "raw provider error reached a visible projection"
        );
    }
    if stop_pending {
        assert!(pending_dropped.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(reply.content, crate::agent::STOPPED_RUN_NOTICE);
        assert_eq!(
            reply.task_facts.as_ref().unwrap().terminal_reason,
            Some(crate::agent::task_facts::TaskTerminalReason::Stopped)
        );
        println!("LIVE_MODEL_STOP actual_stop_command=true pending_future_dropped=true retry_count=0 known_receipt_retained=true");
    } else {
        assert!(reply.content.contains("新消息"));
    }
    reply
}
