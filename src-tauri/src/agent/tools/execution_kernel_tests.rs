use super::execution_kernel::{
    BudgetPolicy, ExecutionHost, ExecutionKernel, KernelEvent, KernelEventKind, KernelFlow,
    KernelModelTurnError, KernelNext, KernelRunError, KernelStopReason, ModelTurnRequest,
};
use crate::agent::tool::ToolCancellation;
use crate::llm::{LlmProvider, LlmResponse, Message, StopReason, ThinkingSettings, ToolSchema};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct FailsOnceProvider {
    calls: AtomicUsize,
    error: &'static str,
}

struct ToolThenTextProvider {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl LlmProvider for ToolThenTextProvider {
    fn name(&self) -> &str {
        "tool-then-text"
    }

    async fn chat(
        &self,
        _messages: &[Message],
        _tools: Option<&[ToolSchema]>,
        _thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(LlmResponse::ToolCalls {
                calls: vec![crate::llm::ToolCall {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({ "path": "note.txt" }),
                }],
                text: Some("I will inspect the note.".to_string()),
                stop_reason: StopReason::ToolUse,
            }),
            _ => Ok(LlmResponse::Text {
                text: "The note is ready.".to_string(),
                stop_reason: StopReason::EndTurn,
            }),
        }
    }
}

struct ToolThenTextHost {
    provider: Arc<dyn LlmProvider>,
    settled: Vec<&'static str>,
}

#[async_trait::async_trait]
impl ExecutionHost for ToolThenTextHost {
    type Output = String;
    type Error = String;

    async fn next_model_turn(
        &mut self,
        _turn: usize,
    ) -> Result<KernelNext<Self::Output>, Self::Error> {
        Ok(KernelNext::Model(ModelTurnRequest::new(
            self.provider.clone(),
            vec![Message {
                role: "user".to_string(),
                content: "inspect the note".to_string(),
                tool_calls: None,
                tool_call_id: None,
                tool_images: Vec::new(),
                protocol_state: None,
            }],
            None,
            None,
            None,
        )))
    }

    async fn settle_model_response(
        &mut self,
        _turn: usize,
        response: LlmResponse,
    ) -> Result<KernelFlow<Self::Output>, Self::Error> {
        match response {
            LlmResponse::ToolCalls { calls, .. } => {
                self.settled.push("tools");
                assert_eq!(calls[0].id, "call-1");
                Ok(KernelFlow::Continue)
            }
            LlmResponse::Text { text, .. } => {
                self.settled.push("text");
                Ok(KernelFlow::Complete(text))
            }
            other => Err(format!("unexpected response: {other:?}")),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for FailsOnceProvider {
    fn name(&self) -> &str {
        "fails-once"
    }

    async fn chat(
        &self,
        _messages: &[Message],
        _tools: Option<&[ToolSchema]>,
        _thinking: Option<&ThinkingSettings>,
    ) -> Result<LlmResponse, String> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(self.error.to_string())
        } else {
            Ok(LlmResponse::Text {
                text: "recovered".to_string(),
                stop_reason: StopReason::EndTurn,
            })
        }
    }
}

#[test]
fn cancelled_kernel_rejects_a_model_turn_before_any_host_work() {
    let cancellation = ToolCancellation::new();
    cancellation.cancel();
    let kernel = ExecutionKernel::new(cancellation, Duration::from_secs(45), 2);

    assert_eq!(
        kernel.check_before_model_turn(),
        Err(KernelStopReason::Cancelled)
    );
}

#[tokio::test]
async fn cancellation_drops_an_inflight_model_without_retry_or_finish() {
    use std::sync::atomic::AtomicBool;
    struct PendingProvider {
        entered: tokio::sync::Notify,
        calls: AtomicUsize,
        dropped: AtomicBool,
    }
    #[async_trait::async_trait]
    impl LlmProvider for PendingProvider {
        fn name(&self) -> &str {
            "owned-pending-model"
        }
        async fn chat(
            &self,
            _: &[Message],
            _: Option<&[ToolSchema]>,
            _: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            struct OnDrop<'a>(&'a AtomicBool);
            impl Drop for OnDrop<'_> {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _drop = OnDrop(&self.dropped);
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            std::future::pending().await
        }
    }
    let cancellation = ToolCancellation::new();
    let events = Arc::new(Mutex::new(Vec::<KernelEvent>::new()));
    let kernel = ExecutionKernel::new(cancellation.clone(), Duration::from_secs(45), 2)
        .with_event_sink({
            let events = events.clone();
            move |event| events.lock().unwrap().push(event)
        });
    let provider = PendingProvider {
        entered: tokio::sync::Notify::new(),
        calls: AtomicUsize::new(0),
        dropped: AtomicBool::new(false),
    };
    let mut call = Box::pin(kernel.run_model_turn(&provider, &[], None, None, None));
    tokio::select! {
        _ = &mut call => panic!("owned pending model unexpectedly completed"),
        _ = provider.entered.notified() => {},
    }
    cancellation.cancel();
    let outcome = tokio::time::timeout(Duration::from_millis(200), &mut call).await;
    drop(call); // Reap the owned future even when the regression fails.
    assert!(
        outcome.is_ok(),
        "stop must not wait for the 45-second model timeout"
    );
    assert!(matches!(
        outcome.unwrap(),
        Err(KernelModelTurnError::Cancelled)
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(provider.dropped.load(Ordering::SeqCst));
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, KernelEventKind::ModelTurnStarted);
}

#[tokio::test]
async fn cancellation_wins_response_and_timeout_backoff_races() {
    struct CancelsOnResponse(ToolCancellation);
    #[async_trait::async_trait]
    impl LlmProvider for CancelsOnResponse {
        fn name(&self) -> &str {
            "owned-cancel-race"
        }
        async fn chat(
            &self,
            _: &[Message],
            _: Option<&[ToolSchema]>,
            _: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            self.0.cancel();
            Ok(LlmResponse::ToolCalls {
                calls: vec![crate::llm::ToolCall {
                    id: "must-not-dispatch".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({"path":"owned-late-output.txt","content":"not authorized after Stop"}),
                }],
                text: None,
                stop_reason: StopReason::ToolUse,
            })
        }
    }
    let cancellation = ToolCancellation::new();
    let kernel = ExecutionKernel::new(cancellation.clone(), Duration::from_secs(1), 2);
    let mut host = ToolThenTextHost {
        provider: Arc::new(CancelsOnResponse(cancellation)),
        settled: vec![],
    };
    assert!(matches!(
        kernel.run(&mut host).await,
        Err(KernelRunError::Model(KernelModelTurnError::Cancelled))
    ));
    assert!(
        host.settled.is_empty(),
        "a late tool response cannot reach the host"
    );

    struct NeverResponds(AtomicUsize);
    #[async_trait::async_trait]
    impl LlmProvider for NeverResponds {
        fn name(&self) -> &str {
            "owned-timeout-cancel"
        }
        async fn chat(
            &self,
            _: &[Message],
            _: Option<&[ToolSchema]>,
            _: Option<&ThinkingSettings>,
        ) -> Result<LlmResponse, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        }
    }
    let cancellation = ToolCancellation::new();
    let kernel = ExecutionKernel::new(cancellation.clone(), Duration::from_millis(1), 2)
        .with_event_sink(move |event| {
            if event.kind == KernelEventKind::ModelTurnRetrying {
                cancellation.cancel();
            }
        });
    let provider = NeverResponds(AtomicUsize::new(0));
    let outcome = tokio::time::timeout(
        Duration::from_millis(200),
        kernel.run_model_turn(&provider, &[], None, None, None),
    )
    .await
    .expect("stop must interrupt timeout backoff");
    assert!(matches!(outcome, Err(KernelModelTurnError::Cancelled)));
    assert_eq!(provider.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn controlled_tool_batch_emits_stable_boundary_events_around_host_work() {
    let events = Arc::new(Mutex::new(Vec::<KernelEvent>::new()));
    let kernel = ExecutionKernel::new(ToolCancellation::new(), Duration::from_secs(45), 2)
        .with_event_sink({
            let events = events.clone();
            move |event| events.lock().expect("event lock").push(event)
        });

    let result = kernel
        .run_controlled_tool_batch(|| async { Ok::<_, &'static str>("executed") })
        .await;

    assert_eq!(result, Ok("executed"));
    let kinds: Vec<_> = events
        .lock()
        .expect("event lock")
        .iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            KernelEventKind::ToolBatchStarted,
            KernelEventKind::ToolBatchFinished
        ]
    );
}

#[test]
fn controlled_tool_batch_balances_an_early_host_exit_once() {
    let events = Arc::new(Mutex::new(Vec::<KernelEvent>::new()));
    let kernel = ExecutionKernel::new(ToolCancellation::new(), Duration::from_secs(45), 2)
        .with_event_sink({
            let events = events.clone();
            move |event| events.lock().expect("event lock").push(event)
        });

    // Foreground confirmation/terminal paths can return before normal batch
    // settlement. The kernel boundary must still close without a duplicate
    // finish event when the host later calls `complete` on normal paths.
    {
        let _batch = kernel.begin_controlled_tool_batch();
    }
    kernel.begin_controlled_tool_batch().complete();

    let kinds: Vec<_> = events
        .lock()
        .expect("event lock")
        .iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            KernelEventKind::ToolBatchStarted,
            KernelEventKind::ToolBatchFinished,
            KernelEventKind::ToolBatchStarted,
            KernelEventKind::ToolBatchFinished,
        ]
    );
}

#[tokio::test]
async fn provider_error_stops_without_retry_and_a_new_turn_can_recover() {
    for error in [
        "Responses HTTP 401",
        "Responses HTTP 400",
        "unclassified provider failure",
    ] {
        let events = Arc::new(Mutex::new(Vec::<KernelEvent>::new()));
        let kernel = ExecutionKernel::new(ToolCancellation::new(), Duration::from_secs(1), 2)
            .with_event_sink({
                let events = events.clone();
                move |event| events.lock().expect("event lock").push(event)
            });
        let provider = FailsOnceProvider {
            calls: AtomicUsize::new(0),
            error,
        };

        let failure = kernel
            .run_model_turn(&provider, &[], None, None, None)
            .await
            .err()
            .expect("a returned provider error must stop, not consume another response");
        assert_eq!(
            failure,
            super::execution_kernel::KernelModelTurnError::Provider(error.into())
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(events.lock().unwrap().len(), 1);
        let response = kernel
            .run_model_turn(&provider, &[], None, None, None)
            .await
            .expect("a new explicit model turn can recover");

        assert!(matches!(response, LlmResponse::Text { text, .. } if text == "recovered"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        let kinds: Vec<_> = events
            .lock()
            .expect("event lock")
            .iter()
            .map(|event| event.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                KernelEventKind::ModelTurnStarted,
                KernelEventKind::ModelTurnStarted,
                KernelEventKind::ModelTurnFinished,
            ]
        );
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .all(|event| event.attempt == 1));
    }
}

#[tokio::test]
async fn run_owns_model_tool_model_protocol_and_preserves_host_settlement_order() {
    let events = Arc::new(Mutex::new(Vec::<KernelEvent>::new()));
    let kernel = ExecutionKernel::new(ToolCancellation::new(), Duration::from_secs(1), 1)
        .with_event_sink({
            let events = events.clone();
            move |event| events.lock().expect("event lock").push(event)
        });
    let mut host = ToolThenTextHost {
        provider: Arc::new(ToolThenTextProvider {
            calls: AtomicUsize::new(0),
        }),
        settled: Vec::new(),
    };

    let output = kernel
        .run(&mut host)
        .await
        .expect("kernel run should finish");

    assert_eq!(output, "The note is ready.");
    assert_eq!(host.settled, vec!["tools", "text"]);
    let kinds: Vec<_> = events
        .lock()
        .expect("event lock")
        .iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            KernelEventKind::ModelTurnStarted,
            KernelEventKind::ModelTurnFinished,
            KernelEventKind::ToolBatchStarted,
            KernelEventKind::ToolBatchFinished,
            KernelEventKind::ModelTurnStarted,
            KernelEventKind::ModelTurnFinished,
        ]
    );
}

#[tokio::test]
async fn run_enforces_explicit_model_turn_budget_without_an_implicit_iteration_limit() {
    let kernel = ExecutionKernel::new(ToolCancellation::new(), Duration::from_secs(1), 1)
        .with_budget_policy(BudgetPolicy::new(Some(1), None, None));
    let mut host = ToolThenTextHost {
        provider: Arc::new(ToolThenTextProvider {
            calls: AtomicUsize::new(0),
        }),
        settled: Vec::new(),
    };

    let error = kernel
        .run(&mut host)
        .await
        .expect_err("second turn exceeds budget");

    assert!(matches!(error, KernelRunError::BudgetExceeded(_)));
    assert_eq!(host.settled, vec!["tools"]);
}
