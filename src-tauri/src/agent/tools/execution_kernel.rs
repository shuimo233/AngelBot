//! Shared, host-agnostic mechanics for a bounded Agent execution protocol.
//!
//! The kernel deliberately does not know about sessions, UI events,
//! confirmation, persistence, or a tool registry.  A host owns those policy
//! concerns and uses this module for the repeatable mechanics common to a
//! foreground conversation and a future delegated worker: cancellation checks,
//! bounded model retries, ordered execution milestones, and controlled-batch
//! boundaries.

use crate::agent::tool::ToolCancellation;
use crate::llm::{
    LlmProvider, LlmResponse, Message as LlmMessage, ThinkingSettings, ToolCall, ToolSchema,
};
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelEventKind {
    ModelTurnStarted,
    ModelTurnRetrying,
    ModelTurnFinished,
    ToolBatchStarted,
    ToolBatchFinished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelEvent {
    pub kind: KernelEventKind,
    /// One-based model attempt number. Tool-batch events use zero.
    pub attempt: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelStopReason {
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelModelTurnError {
    Provider(String),
    TimedOut { attempts: usize, timeout: Duration },
}

/// Explicit execution limits supplied by a host or its control plane.
///
/// Every limit is optional: the kernel has no concealed iteration cap. A host
/// that needs bounded execution must opt in with a concrete policy, while an
/// existing foreground host can migrate without a semantic change and add its
/// policy in a later slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BudgetPolicy {
    pub max_model_turns: Option<usize>,
    pub max_tool_batches: Option<usize>,
    pub wall_clock_limit: Option<Duration>,
}

impl BudgetPolicy {
    pub const fn new(
        max_model_turns: Option<usize>,
        max_tool_batches: Option<usize>,
        wall_clock_limit: Option<Duration>,
    ) -> Self {
        Self {
            max_model_turns,
            max_tool_batches,
            wall_clock_limit,
        }
    }

    pub const fn unbounded() -> Self {
        Self::new(None, None, None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelBudgetExceeded {
    ModelTurns { limit: usize },
    ToolBatches { limit: usize },
    WallClock { limit: Duration },
}

/// A fully-owned model request assembled by a host. Owning the request keeps
/// the kernel independent from session state and lets foreground and worker
/// hosts use different context sources safely.
pub struct ModelTurnRequest {
    pub provider: Arc<dyn LlmProvider>,
    pub messages: Vec<LlmMessage>,
    pub tools: Option<Vec<ToolSchema>>,
    pub thinking: Option<ThinkingSettings>,
    pub deltas: Option<UnboundedSender<String>>,
}

impl ModelTurnRequest {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        messages: Vec<LlmMessage>,
        tools: Option<Vec<ToolSchema>>,
        thinking: Option<ThinkingSettings>,
        deltas: Option<UnboundedSender<String>>,
    ) -> Self {
        Self {
            provider,
            messages,
            tools,
            thinking,
            deltas,
        }
    }
}

/// The host either supplies the next model request or adapts the current
/// state into a final host-owned output.
pub enum KernelNext<T> {
    Model(ModelTurnRequest),
    Complete(T),
}

/// The host settles a model response and either requests another turn or
/// produces the final output. Tool-call responses are wrapped in a kernel
/// batch boundary before this method is called.
pub enum KernelFlow<T> {
    Continue,
    Complete(T),
}

/// The only data a controlled tool host may return to the next model turn.
/// It deliberately contains no lease, policy, filesystem, or audit object.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSettlement {
    pub call_id: String,
    pub tool_name: String,
    pub result: Value,
    pub trust: ToolSettlementTrust,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSettlementTrust {
    UntrustedNetwork,
    UntrustedWorkspace,
}

#[derive(Debug, Clone)]
pub struct ToolSettlementBatch {
    pub calls: Vec<ToolCall>,
    pub settlements: Vec<ToolSettlement>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ToolSettlementError {
    #[error("tool settlements are not supported by this execution host")]
    Unsupported,
    #[error("tool settlement batch is invalid")]
    Invalid,
}

pub const MAX_TOOL_SETTLEMENTS_PER_BATCH: usize = 2;
pub const MAX_TOOL_SETTLEMENT_RESULT_BYTES: usize = 16 * 1024;

impl ToolSettlementBatch {
    pub fn new(
        calls: Vec<ToolCall>,
        settlements: Vec<ToolSettlement>,
    ) -> Result<Self, ToolSettlementError> {
        if calls.is_empty()
            || calls.len() > MAX_TOOL_SETTLEMENTS_PER_BATCH
            || calls.len() != settlements.len()
            || calls.iter().zip(&settlements).any(|(call, settlement)| {
                call.id.trim().is_empty()
                    || call.name.trim().is_empty()
                    || call.id != settlement.call_id
                    || call.name != settlement.tool_name
                    || serde_json::to_vec(&settlement.result)
                        .map(|value| value.len() > MAX_TOOL_SETTLEMENT_RESULT_BYTES)
                        .unwrap_or(true)
            })
        {
            return Err(ToolSettlementError::Invalid);
        }
        Ok(Self { calls, settlements })
    }
}

impl From<ToolSettlementError> for String {
    fn from(value: ToolSettlementError) -> Self {
        value.to_string()
    }
}

/// Host port for the multi-turn protocol. It deliberately exposes no session,
/// UI, confirmation, registry, audit, or permission API: those remain owned
/// by the concrete foreground or worker host.
#[async_trait::async_trait]
pub trait ExecutionHost: Send {
    type Output: Send;
    type Error: Send;

    async fn next_model_turn(
        &mut self,
        turn: usize,
    ) -> Result<KernelNext<Self::Output>, Self::Error>;

    async fn settle_model_response(
        &mut self,
        turn: usize,
        response: LlmResponse,
    ) -> Result<KernelFlow<Self::Output>, Self::Error>;

    /// Accept an already-authorized, ordered tool settlement.  Hosts decline
    /// this by default; only a dedicated tool host can bridge evidence into
    /// its next minimal model context.
    async fn settle_tool_calls(
        &mut self,
        _turn: usize,
        _batch: ToolSettlementBatch,
    ) -> Result<KernelFlow<Self::Output>, Self::Error>
    where
        Self::Error: From<ToolSettlementError>,
    {
        Err(ToolSettlementError::Unsupported.into())
    }
}

#[derive(Debug)]
pub enum KernelRunError<E> {
    Host(E),
    Model(KernelModelTurnError),
    BudgetExceeded(KernelBudgetExceeded),
}

type KernelEventSink = Arc<dyn Fn(KernelEvent) + Send + Sync>;

/// A host-owned boundary for one ordered tool-batch settlement.
///
/// Creating the boundary records that a batch has begun. The host retains all
/// policy decisions and performs every tool action itself; it calls
/// [`ControlledToolBatch::complete`] only after it has settled the batch in
/// its own event/audit order. Dropping an uncompleted boundary still emits the
/// finish milestone so terminal and recovery exits cannot leave the kernel's
/// lifecycle stream unbalanced.
pub struct ControlledToolBatch {
    event_sink: Option<KernelEventSink>,
    completed: bool,
}

impl ControlledToolBatch {
    /// Mark the host's ordered settlement complete.
    pub fn complete(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        if self.completed {
            return;
        }
        if let Some(sink) = &self.event_sink {
            sink(KernelEvent {
                kind: KernelEventKind::ToolBatchFinished,
                attempt: 0,
            });
        }
        self.completed = true;
    }
}

impl Drop for ControlledToolBatch {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Reusable mechanics only. Hosts translate [`KernelEvent`] into their own
/// durable/UI event representation and retain all policy and ownership.
#[derive(Clone)]
pub struct ExecutionKernel {
    cancellation: ToolCancellation,
    model_timeout: Duration,
    max_model_attempts: usize,
    budget_policy: BudgetPolicy,
    event_sink: Option<KernelEventSink>,
}

impl ExecutionKernel {
    pub fn new(
        cancellation: ToolCancellation,
        model_timeout: Duration,
        max_model_attempts: usize,
    ) -> Self {
        Self {
            cancellation,
            model_timeout,
            max_model_attempts: max_model_attempts.max(1),
            budget_policy: BudgetPolicy::unbounded(),
            event_sink: None,
        }
    }

    pub fn with_event_sink(mut self, sink: impl Fn(KernelEvent) + Send + Sync + 'static) -> Self {
        self.event_sink = Some(Arc::new(sink));
        self
    }

    pub fn with_budget_policy(mut self, budget_policy: BudgetPolicy) -> Self {
        self.budget_policy = budget_policy;
        self
    }

    /// Own the host-agnostic multi-turn protocol:
    /// `next request -> model -> ordered controlled batch settlement -> next`.
    ///
    /// The host controls all task-specific semantics. In particular it owns
    /// confirmation, audit, tool-registry dispatch, persistence, final-output
    /// adaptation, and the construction of the next minimal context.
    pub async fn run<H>(&self, host: &mut H) -> Result<H::Output, KernelRunError<H::Error>>
    where
        H: ExecutionHost,
    {
        let started_at = Instant::now();
        let mut model_turns = 0usize;
        let mut tool_batches = 0usize;

        loop {
            self.check_wall_clock_budget(started_at)
                .map_err(KernelRunError::BudgetExceeded)?;
            let next = host
                .next_model_turn(model_turns)
                .await
                .map_err(KernelRunError::Host)?;
            let request = match next {
                KernelNext::Complete(output) => return Ok(output),
                KernelNext::Model(request) => request,
            };

            self.check_model_turn_budget(model_turns + 1)
                .map_err(KernelRunError::BudgetExceeded)?;
            let response = self
                .run_model_turn(
                    request.provider.as_ref(),
                    &request.messages,
                    request.tools.as_deref(),
                    request.thinking.as_ref(),
                    request.deltas,
                )
                .await
                .map_err(KernelRunError::Model)?;
            model_turns += 1;

            let uses_tools = crate::agent::lifecycle::ResponseKind::classify(&response)
                == crate::agent::lifecycle::ResponseKind::ToolCalls;
            if uses_tools {
                self.check_tool_batch_budget(tool_batches + 1)
                    .map_err(KernelRunError::BudgetExceeded)?;
                tool_batches += 1;
            }

            let flow = if uses_tools {
                self.run_controlled_tool_batch(|| {
                    host.settle_model_response(model_turns - 1, response)
                })
                .await
            } else {
                host.settle_model_response(model_turns - 1, response).await
            }
            .map_err(KernelRunError::Host)?;

            match flow {
                KernelFlow::Continue => continue,
                KernelFlow::Complete(output) => return Ok(output),
            }
        }
    }

    pub fn check_before_model_turn(&self) -> Result<(), KernelStopReason> {
        if self.cancellation.is_cancelled() {
            Err(KernelStopReason::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Execute one model turn using the current foreground-compatible retry
    /// policy. The host remains responsible for translating terminal errors
    /// into user-visible responses and durable events.
    pub async fn run_model_turn(
        &self,
        provider: &dyn LlmProvider,
        messages: &[LlmMessage],
        tools: Option<&[ToolSchema]>,
        thinking: Option<&ThinkingSettings>,
        deltas: Option<UnboundedSender<String>>,
    ) -> Result<LlmResponse, KernelModelTurnError> {
        self.check_before_model_turn()
            .map_err(|_| KernelModelTurnError::Provider("cancelled".to_string()))?;

        for attempt in 1..=self.max_model_attempts {
            self.emit(KernelEventKind::ModelTurnStarted, attempt);
            match tokio::time::timeout(
                self.model_timeout,
                provider.chat_stream(messages, tools, thinking, deltas.clone()),
            )
            .await
            {
                Ok(Ok(response)) => {
                    self.emit(KernelEventKind::ModelTurnFinished, attempt);
                    return Ok(response);
                }
                Ok(Err(_error)) if attempt < self.max_model_attempts => {
                    self.emit(KernelEventKind::ModelTurnRetrying, attempt);
                    tokio::time::sleep(Duration::from_millis(750)).await;
                }
                Ok(Err(error)) => return Err(KernelModelTurnError::Provider(error)),
                Err(_) if attempt < self.max_model_attempts => {
                    self.emit(KernelEventKind::ModelTurnRetrying, attempt);
                    tokio::time::sleep(Duration::from_millis(750)).await;
                }
                Err(_) => {
                    return Err(KernelModelTurnError::TimedOut {
                        attempts: self.max_model_attempts,
                        timeout: self.model_timeout,
                    });
                }
            }
        }
        unreachable!("a bounded model turn always returns from its final attempt")
    }

    /// Bound a host-controlled batch with stable milestones. The host still
    /// owns authorization, confirmation, registry dispatch and persistence.
    pub async fn run_controlled_tool_batch<T, E, F, Fut>(&self, batch: F) -> Result<T, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        self.emit(KernelEventKind::ToolBatchStarted, 0);
        let result = batch().await;
        self.emit(KernelEventKind::ToolBatchFinished, 0);
        result
    }

    /// Start a host-controlled tool batch without exposing host-owned policy
    /// to the kernel. This is suitable for hosts whose existing settlement
    /// body contains early terminal/recovery returns: the boundary remains
    /// balanced even on those paths.
    pub fn begin_controlled_tool_batch(&self) -> ControlledToolBatch {
        self.emit(KernelEventKind::ToolBatchStarted, 0);
        ControlledToolBatch {
            event_sink: self.event_sink.clone(),
            completed: false,
        }
    }

    fn emit(&self, kind: KernelEventKind, attempt: usize) {
        if let Some(sink) = &self.event_sink {
            sink(KernelEvent { kind, attempt });
        }
    }

    fn check_model_turn_budget(&self, next_turn: usize) -> Result<(), KernelBudgetExceeded> {
        if self
            .budget_policy
            .max_model_turns
            .is_some_and(|limit| next_turn > limit)
        {
            return Err(KernelBudgetExceeded::ModelTurns {
                limit: self.budget_policy.max_model_turns.expect("checked above"),
            });
        }
        Ok(())
    }

    fn check_tool_batch_budget(&self, next_batch: usize) -> Result<(), KernelBudgetExceeded> {
        if self
            .budget_policy
            .max_tool_batches
            .is_some_and(|limit| next_batch > limit)
        {
            return Err(KernelBudgetExceeded::ToolBatches {
                limit: self.budget_policy.max_tool_batches.expect("checked above"),
            });
        }
        Ok(())
    }

    fn check_wall_clock_budget(&self, started_at: Instant) -> Result<(), KernelBudgetExceeded> {
        if self
            .budget_policy
            .wall_clock_limit
            .is_some_and(|limit| started_at.elapsed() > limit)
        {
            return Err(KernelBudgetExceeded::WallClock {
                limit: self.budget_policy.wall_clock_limit.expect("checked above"),
            });
        }
        Ok(())
    }
}
