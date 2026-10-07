//! Agent event system for real-time UI updates
//!
//! Provides a comprehensive event stream following Pi's agent-core pattern:
//! ```text
//! agent_start → turn_start → message_start → message_update → message_end →
//! [tool_execution_start → tool_execution_end] → turn_end → agent_end
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::Emitter;

/// Unified content block types emitted in arrival order during streaming.
///
/// This replaces the previous parallel event model where `MessageDelta`
/// and `ToolExecutionStart/End` were emitted on separate tracks.
/// Now the runner emits each block as it arrives from the LLM stream,
/// giving the frontend a single, ordered sequence to render.
///
/// Design goals:
/// - Text blocks are the LLM's own reasoning/narration (not hidden internal chain).
/// - Tool blocks carry only enough info for the UI to show a compact inline card.
/// - Results appear as separate blocks *after* their corresponding tool block,
///   completing the interleaved "think → tool → result → think → tool → result" flow.
/// - Confirmation-required tools emit `NeedsApproval` and pause the stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum ContentBlockData {
    /// LLM text output (reasoning, narration, or final reply segment).
    Text { content: String },
    /// A tool invocation started. Emitted when the LLM finishes producing
    /// a tool_call response block. The UI renders this as an inline badge.
    ToolCallStart {
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    /// Tool execution completed successfully. Emitted after `ToolCallStart`.
    /// The `output` is shown only if the user expands the block.
    ToolCallResult {
        call_id: String,
        tool_name: String,
        success: bool,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Tool execution requires user confirmation. Stream pauses here;
    /// the frontend should render confirmation buttons inline.
    ToolCallNeedsApproval {
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        reason: String,
    },
    /// Tool execution failed without needing confirmation.
    ToolCallFailed {
        call_id: String,
        tool_name: String,
        error: String,
    },
}

/// All events emitted by the agent during execution
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AgentEvent {
    /// Agent processing started
    AgentStart { session_id: String, timestamp: i64 },
    /// New turn (user message) started
    TurnStart {
        turn_id: String,
        session_id: String,
        message: String,
    },
    /// LLM started generating a message
    MessageStart { turn_id: String, message_id: String },
    /// Streaming text delta received
    MessageDelta {
        turn_id: String,
        message_id: String,
        delta: String,
    },
    /// Unified content block stream — replaces parallel MessageDelta + ToolExecution*.
    /// Each block carries a sequential index so the frontend can render them
    /// in arrival order, interleaving reasoning text and tool invocations
    /// exactly as the LLM produced them.
    ContentBlock {
        turn_id: String,
        message_id: String,
        /// Monotonically increasing sequence number.
        index: u32,
        /// Identifies which main-loop iteration this block belongs to.
        /// The frontend groups consecutive tool blocks sharing the same
        /// `iteration_id` into one "execution slice" so reasoning text
        /// and tool batches stay visually separate.
        iteration_id: u32,
        block: ContentBlockData,
    },
    /// Issue #094: Complete message update (replaces accumulated deltas).
    /// Allows UI to replace the full message content atomically.
    MessageUpdate {
        turn_id: String,
        message_id: String,
        content: String,
        /// If true, this is the final update and the message is complete.
        is_final: bool,
    },
    /// LLM finished generating the message
    MessageEnd {
        turn_id: String,
        message_id: String,
        full_text: String,
    },
    /// Tool execution started
    ToolExecutionStart {
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    /// Tool execution progress update (for long-running tools)
    ToolExecutionProgress {
        call_id: String,
        tool_name: String,
        progress: String,
        #[serde(default)]
        percent: Option<f32>,
    },
    /// Tool execution completed
    ToolExecutionEnd {
        call_id: String,
        tool_name: String,
        success: bool,
        output: String,
        #[serde(default)]
        error: Option<String>,
    },
    /// Turn completed
    TurnEnd {
        turn_id: String,
        success: bool,
        tool_calls_count: usize,
    },
    /// Agent processing finished
    AgentEnd {
        session_id: String,
        summary: Option<String>,
        total_turns: usize,
        total_tool_calls: usize,
    },
    /// Error occurred during agent execution
    Error {
        code: ErrorCode,
        message: String,
        #[serde(default)]
        recoverable: bool,
        #[serde(default)]
        turn_id: Option<String>,
    },
    // ── Issue #059: Plan-and-Execute events ────────────────────────────
    /// Planner started generating a plan
    PlanStart {
        session_id: String,
        turn_id: String,
        complexity: String,
    },
    /// Planner finished with a structured plan
    PlanReady {
        session_id: String,
        turn_id: String,
        estimated_steps: usize,
        execution_mode: String,
    },
    /// A plan step started execution
    StepStart {
        session_id: String,
        turn_id: String,
        step_id: u32,
        description: String,
    },
    /// A plan step finished
    StepEnd {
        session_id: String,
        turn_id: String,
        step_id: u32,
        success: bool,
        actual_outcome: Option<String>,
    },
    /// Planner triggered a replan
    PlanReplan {
        session_id: String,
        turn_id: String,
        replan_count: u32,
        reason: String,
    },
    /// Plan execution completed
    PlanEnd {
        session_id: String,
        turn_id: String,
        success: bool,
        completed_steps: usize,
        total_steps: usize,
    },
    // ── Issue #060: Tree-of-Thoughts events ────────────────────────────
    /// ToT started exploring a branch
    ToTBranchStart {
        session_id: String,
        turn_id: String,
        branch_id: String,
        parent_branch_id: Option<String>,
        depth: usize,
    },
    /// ToT pruned a low-scoring branch
    ToTBranchPrune {
        session_id: String,
        turn_id: String,
        branch_id: String,
        score: f32,
        reason: String,
    },
    /// ToT selected a final best path
    ToTPathSelected {
        session_id: String,
        turn_id: String,
        best_branch_id: String,
        score: f32,
    },
    // ── Issue #061: Context compression events ────────────────────────
    /// Context was compressed mid-loop
    ContextCompressed {
        session_id: String,
        turn_id: String,
        before_tokens: usize,
        after_tokens: usize,
        /// Durable `context_summaries.id` that the UI can use to locate the source.
        #[serde(default)]
        summary_id: Option<String>,
        /// Whether the compression prompt included persisted task facts.
        #[serde(default)]
        includes_task_facts: bool,
    },
    /// Tool result was served from cache
    ToolCacheHit {
        session_id: String,
        tool_name: String,
        args_hash: u64,
    },
}

/// Error codes for agent errors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ErrorCode {
    /// LLM API call failed
    LlmError,
    /// Tool execution failed
    ToolError,
    /// Context length exceeded
    ContextLengthExceeded,
    /// Timeout during execution
    Timeout,
    /// Maximum iterations reached
    MaxIterationsReached,
    /// Configuration error
    ConfigError,
    /// Generic unknown error
    Unknown,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::LlmError => "LLM_ERROR",
            ErrorCode::ToolError => "TOOL_ERROR",
            ErrorCode::ContextLengthExceeded => "CONTEXT_LENGTH_EXCEEDED",
            ErrorCode::Timeout => "TIMEOUT",
            ErrorCode::MaxIterationsReached => "MAX_ITERATIONS_REACHED",
            ErrorCode::ConfigError => "CONFIG_ERROR",
            ErrorCode::Unknown => "UNKNOWN",
        }
    }
}

/// Event emitter trait for bridging agent events to frontend
pub trait AgentEventEmitter: Send + Sync {
    fn emit(&self, session_id: &str, event: AgentEvent);
}

/// Writes an append-only lifecycle record before forwarding an event to its UI
/// transport. The proactive `agent_events` inbox intentionally remains
/// separate: lifecycle records are audit facts and must never be consumed.
pub struct DurableEventEmitter {
    inner: Arc<dyn AgentEventEmitter>,
    journal: AgentRunJournal,
}

/// Append-only persistence for one run's lifecycle facts. It is intentionally
/// usable without a UI emitter so confirmation, cancellation and recovery
/// commands can journal their transitions too.
pub struct AgentRunJournal {
    db: Arc<Mutex<rusqlite::Connection>>,
    run_id: String,
    next_sequence: Mutex<i64>,
}

impl AgentRunJournal {
    pub fn new(db: Arc<Mutex<rusqlite::Connection>>, run_id: String) -> Self {
        let next_sequence = db
            .lock()
            .ok()
            .and_then(|conn| {
                conn.query_row(
                    "SELECT COALESCE(MAX(sequence) + 1, 0) FROM agent_run_events WHERE run_id = ?1",
                    rusqlite::params![&run_id],
                    |row| row.get(0),
                )
                .ok()
            })
            .unwrap_or(0);
        Self {
            db,
            run_id,
            next_sequence: Mutex::new(next_sequence),
        }
    }

    /// Persist one lifecycle fact. Callers that form a durability barrier must
    /// handle the result; legacy observational callers may deliberately ignore
    /// it after logging their own recovery path.
    pub fn append(&self, session_id: &str, event: &AgentEvent) -> Result<(), String> {
        self.append_batch(session_id, std::slice::from_ref(event))
    }

    /// Persist an ordered terminal batch atomically. Keeping sequence assignment
    /// and insertion together means a terminal UI event can never be exposed
    /// with only a prefix of its audit trail on disk.
    pub fn append_batch(&self, session_id: &str, events: &[AgentEvent]) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        let serialized = events
            .iter()
            .map(|event| {
                let payload = serde_json::to_string(event).map_err(|error| error.to_string())?;
                let event_type = serde_json::to_value(event)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("type")
                            .and_then(|event_type| event_type.as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "unknown".to_string());
                Ok::<_, String>((event_type, payload))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut next = self
            .next_sequence
            .lock()
            .map_err(|error| error.to_string())?;
        let mut conn = self.db.lock().map_err(|error| error.to_string())?;
        let transaction = conn.transaction().map_err(|error| error.to_string())?;
        for (offset, (event_type, payload)) in serialized.iter().enumerate() {
            transaction
                .execute(
                    "INSERT INTO agent_run_events (id, run_id, session_id, sequence, event_type, payload, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        uuid::Uuid::new_v4().to_string(),
                        &self.run_id,
                        session_id,
                        *next + offset as i64,
                        event_type,
                        payload,
                        chrono::Utc::now().timestamp(),
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())?;
        *next += events.len() as i64;
        Ok(())
    }
}

impl DurableEventEmitter {
    pub fn new(
        inner: Arc<dyn AgentEventEmitter>,
        db: Arc<Mutex<rusqlite::Connection>>,
        run_id: String,
    ) -> Self {
        Self {
            inner,
            journal: AgentRunJournal::new(db, run_id),
        }
    }
}

impl AgentEventEmitter for DurableEventEmitter {
    fn emit(&self, session_id: &str, event: AgentEvent) {
        if let Err(error) = self.journal.append(session_id, &event) {
            eprintln!("[AgentRunEvent] failed to persist event: {error}");
        }
        self.inner.emit(session_id, event);
    }
}

/// Pollable, per-session event store for the debug HTTP gateway. The dev
/// runner is not on Tauri's IPC path, so browser clients cannot receive its
/// window events directly.
#[derive(Default)]
pub struct DevEventStore {
    events: Mutex<HashMap<String, Vec<AgentEvent>>>,
}

impl DevEventStore {
    pub fn events_for(&self, session_id: &str) -> Vec<AgentEvent> {
        self.events
            .lock()
            .ok()
            .and_then(|events| events.get(session_id).cloned())
            .unwrap_or_default()
    }
}

impl AgentEventEmitter for DevEventStore {
    fn emit(&self, session_id: &str, event: AgentEvent) {
        if let Ok(mut events) = self.events.lock() {
            let session_events = events.entry(session_id.to_string()).or_default();
            session_events.push(event);
            const MAX_EVENTS_PER_SESSION: usize = 512;
            if session_events.len() > MAX_EVENTS_PER_SESSION {
                let excess = session_events.len() - MAX_EVENTS_PER_SESSION;
                session_events.drain(..excess);
            }
        }
    }
}

/// Tauri-based event emitter that forwards events to frontend via window.emit
#[derive(Clone)]
pub struct TauriEventEmitter {
    sender: std::sync::mpsc::Sender<(String, AgentEvent)>,
    event_store: Option<std::sync::Arc<DevEventStore>>,
}

impl TauriEventEmitter {
    pub fn new(
        app_handle: tauri::AppHandle,
        event_store: Option<std::sync::Arc<DevEventStore>>,
    ) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<(String, AgentEvent)>();

        // Keep UI dispatch off the agent command path while retaining strict
        // event ordering. Sending each event in an independent task can make
        // AgentEnd arrive before AgentStart; dispatching on the command thread
        // can instead block the IPC response when the WebView is busy.
        std::thread::spawn(move || {
            while let Ok((event_name, event)) = receiver.recv() {
                if let Err(e) = app_handle.emit(&event_name, &event) {
                    eprintln!("[AgentEvent] Failed to emit {}: {}", event_name, e);
                }
            }
        });

        Self {
            sender,
            event_store,
        }
    }
}

impl AgentEventEmitter for TauriEventEmitter {
    fn emit(&self, session_id: &str, event: AgentEvent) {
        if let Some(store) = &self.event_store {
            store.emit(session_id, event.clone());
        }
        let event_name = format!("agent_event_{}", session_id);
        if let Err(e) = self.sender.send((event_name, event)) {
            eprintln!("[AgentEvent] Event dispatcher unavailable: {}", e);
        }
    }
}

/// Null emitter for testing or when no frontend connection exists
pub struct NullEventEmitter;

impl AgentEventEmitter for NullEventEmitter {
    fn emit(&self, _session_id: &str, event: AgentEvent) {
        eprintln!("[AgentEvent (null)] {:?}", event);
    }
}

/// Wrapper that allows Option<dyn AgentEventEmitter>
pub struct OptionalEmitter(pub Option<std::sync::Arc<dyn AgentEventEmitter>>);

impl OptionalEmitter {
    pub fn new(emitter: Option<std::sync::Arc<dyn AgentEventEmitter>>) -> Self {
        Self(emitter)
    }

    pub fn emit(&self, session_id: &str, event: AgentEvent) {
        if let Some(emitter) = &self.0 {
            emitter.emit(session_id, event);
        }
    }
}

/// Thread-safe event emitter container for AppState
#[derive(Clone)]
pub struct EventEmitterState {
    inner: std::sync::Arc<dyn AgentEventEmitter>,
}

impl EventEmitterState {
    pub fn new(inner: impl AgentEventEmitter + 'static) -> Self {
        Self {
            inner: std::sync::Arc::new(inner),
        }
    }

    pub fn null() -> Self {
        Self {
            inner: std::sync::Arc::new(NullEventEmitter),
        }
    }

    pub fn emit(&self, session_id: &str, event: AgentEvent) {
        self.inner.emit(session_id, event);
    }
}

impl std::ops::Deref for EventEmitterState {
    type Target = dyn AgentEventEmitter;

    fn deref(&self) -> &Self::Target {
        &*self.inner
    }
}

/// Trait for objects that can receive agent events
pub trait EventEmitter: Send + Sync {
    /// Emit an event to subscribers
    fn emit(&self, event: AgentEvent);

    /// Subscribe to events
    fn on<F>(&self, handler: F)
    where
        F: Fn(AgentEvent) + Send + Sync + 'static;

    /// Subscribe to a specific event type
    fn on_type<F>(&self, event_type: &str, handler: F)
    where
        F: Fn(serde_json::Value) + Send + Sync + 'static;
}

/// Simple in-memory event channel for Tauri event emission
#[derive(Debug, Clone)]
pub struct EventChannel {
    events: std::sync::Arc<std::sync::Mutex<Vec<AgentEvent>>>,
}

impl EventChannel {
    pub fn new() -> Self {
        Self {
            events: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Emit an event and store it in the channel
    pub fn emit(&self, event: AgentEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event.clone());
        }
        eprintln!("[AgentEvent] {:?}", event);
    }

    /// Get all emitted events
    pub fn get_events(&self) -> Vec<AgentEvent> {
        self.events.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// Clear all events
    pub fn clear(&self) {
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
    }
}

impl Default for EventChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl EventEmitter for EventChannel {
    fn emit(&self, event: AgentEvent) {
        self.emit(event);
    }

    fn on<F>(&self, _handler: F)
    where
        F: Fn(AgentEvent) + Send + Sync + 'static,
    {
        // In-memory channel doesn't support dynamic handlers
        // Tauri event system handles this at the application level
    }

    fn on_type<F>(&self, _event_type: &str, _handler: F)
    where
        F: Fn(serde_json::Value) + Send + Sync + 'static,
    {
        // In-memory channel doesn't support dynamic handlers
    }
}

/// Helper to create agent event variants with common fields
#[macro_export]
macro_rules! agent_event {
    (AgentStart, $session_id:expr) => {
        AgentEvent::AgentStart {
            session_id: $session_id.to_string(),
            timestamp: chrono::Utc::now().timestamp(),
        }
    };
    (TurnStart, $turn_id:expr, $session_id:expr, $message:expr) => {
        AgentEvent::TurnStart {
            turn_id: $turn_id.to_string(),
            session_id: $session_id.to_string(),
            message: $message.to_string(),
        }
    };
    (MessageStart, $turn_id:expr, $message_id:expr) => {
        AgentEvent::MessageStart {
            turn_id: $turn_id.to_string(),
            message_id: $message_id.to_string(),
        }
    };
    (MessageDelta, $turn_id:expr, $message_id:expr, $delta:expr) => {
        AgentEvent::MessageDelta {
            turn_id: $turn_id.to_string(),
            message_id: $message_id.to_string(),
            delta: $delta.to_string(),
        }
    };
    (MessageEnd, $turn_id:expr, $message_id:expr, $full_text:expr) => {
        AgentEvent::MessageEnd {
            turn_id: $turn_id.to_string(),
            message_id: $message_id.to_string(),
            full_text: $full_text.to_string(),
        }
    };
    (ToolExecutionStart, $call_id:expr, $tool_name:expr, $arguments:expr) => {
        AgentEvent::ToolExecutionStart {
            call_id: $call_id.to_string(),
            tool_name: $tool_name.to_string(),
            arguments: $arguments,
        }
    };
    (ToolExecutionProgress, $call_id:expr, $tool_name:expr, $progress:expr) => {
        AgentEvent::ToolExecutionProgress {
            call_id: $call_id.to_string(),
            tool_name: $tool_name.to_string(),
            progress: $progress.to_string(),
            percent: None,
        }
    };
    (ToolExecutionEnd, $call_id:expr, $tool_name:expr, $success:expr, $output:expr) => {
        AgentEvent::ToolExecutionEnd {
            call_id: $call_id.to_string(),
            tool_name: $tool_name.to_string(),
            success: $success,
            output: $output.to_string(),
            error: if $success {
                None
            } else {
                Some($output.to_string())
            },
        }
    };
    (TurnEnd, $turn_id:expr, $success:expr, $tool_calls:expr) => {
        AgentEvent::TurnEnd {
            turn_id: $turn_id.to_string(),
            success: $success,
            tool_calls_count: $tool_calls,
        }
    };
    (AgentEnd, $session_id:expr, $summary:expr, $turns:expr, $tools:expr) => {
        AgentEvent::AgentEnd {
            session_id: $session_id.to_string(),
            summary: $summary.map(|s| s.to_string()),
            total_turns: $turns,
            total_tool_calls: $tools,
        }
    };
    (Error, $code:expr, $message:expr) => {
        AgentEvent::Error {
            code: $code,
            message: $message.to_string(),
            recoverable: false,
            turn_id: None,
        }
    };
    (PlanStart, $session_id:expr, $turn_id:expr, $complexity:expr) => {
        AgentEvent::PlanStart {
            session_id: $session_id.to_string(),
            turn_id: $turn_id.to_string(),
            complexity: $complexity.to_string(),
        }
    };
    (PlanReady, $session_id:expr, $turn_id:expr, $steps:expr, $mode:expr) => {
        AgentEvent::PlanReady {
            session_id: $session_id.to_string(),
            turn_id: $turn_id.to_string(),
            estimated_steps: $steps,
            execution_mode: $mode.to_string(),
        }
    };
    (StepStart, $session_id:expr, $turn_id:expr, $step_id:expr, $desc:expr) => {
        AgentEvent::StepStart {
            session_id: $session_id.to_string(),
            turn_id: $turn_id.to_string(),
            step_id: $step_id,
            description: $desc.to_string(),
        }
    };
    (StepEnd, $session_id:expr, $turn_id:expr, $step_id:expr, $success:expr, $outcome:expr) => {
        AgentEvent::StepEnd {
            session_id: $session_id.to_string(),
            turn_id: $turn_id.to_string(),
            step_id: $step_id,
            success: $success,
            actual_outcome: $outcome.map(|s| s.to_string()),
        }
    };
    (PlanReplan, $session_id:expr, $turn_id:expr, $count:expr, $reason:expr) => {
        AgentEvent::PlanReplan {
            session_id: $session_id.to_string(),
            turn_id: $turn_id.to_string(),
            replan_count: $count,
            reason: $reason.to_string(),
        }
    };
    (PlanEnd, $session_id:expr, $turn_id:expr, $success:expr, $done:expr, $total:expr) => {
        AgentEvent::PlanEnd {
            session_id: $session_id.to_string(),
            turn_id: $turn_id.to_string(),
            success: $success,
            completed_steps: $done,
            total_steps: $total,
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_channel() {
        let channel = EventChannel::new();

        channel.emit(agent_event!(AgentStart, "session-123"));
        channel.emit(agent_event!(TurnStart, "turn-1", "session-123", "Hello"));

        let events = channel.get_events();
        assert_eq!(events.len(), 2);

        if let AgentEvent::AgentStart { session_id, .. } = &events[0] {
            assert_eq!(session_id, "session-123");
        } else {
            panic!("Expected AgentStart event");
        }
    }

    #[test]
    fn test_error_code() {
        assert_eq!(ErrorCode::LlmError.as_str(), "LLM_ERROR");
        assert_eq!(ErrorCode::ToolError.as_str(), "TOOL_ERROR");
    }

    #[test]
    fn context_compression_event_exposes_its_durable_summary_source() {
        let event = AgentEvent::ContextCompressed {
            session_id: "session-1".to_string(),
            turn_id: "summary-1".to_string(),
            before_tokens: 12_000,
            after_tokens: 300,
            summary_id: Some("summary-1".to_string()),
            includes_task_facts: true,
        };

        let serialized = serde_json::to_value(event).unwrap();
        assert_eq!(serialized["type"], "ContextCompressed");
        assert_eq!(serialized["data"]["summary_id"], "summary-1");
        assert_eq!(serialized["data"]["includes_task_facts"], true);
    }

    #[test]
    fn durable_emitter_journals_ordered_events_before_forwarding() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY);
             INSERT INTO sessions VALUES ('session-1');
             CREATE TABLE agent_run_events (
                 id TEXT PRIMARY KEY,
                 run_id TEXT NOT NULL,
                 session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                 sequence INTEGER NOT NULL,
                 event_type TEXT NOT NULL,
                 payload TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 UNIQUE(run_id, sequence)
             );",
        )
        .unwrap();
        let db = Arc::new(Mutex::new(conn));
        let transport = Arc::new(DevEventStore::default());
        let emitter = DurableEventEmitter::new(transport.clone(), db.clone(), "run-1".to_string());

        emitter.emit(
            "session-1",
            AgentEvent::AgentStart {
                session_id: "session-1".to_string(),
                timestamp: 1,
            },
        );
        emitter.emit(
            "session-1",
            AgentEvent::TurnEnd {
                turn_id: "turn-1".to_string(),
                success: true,
                tool_calls_count: 0,
            },
        );

        let rows: Vec<(i64, String)> = db
            .lock()
            .unwrap()
            .prepare("SELECT sequence, event_type FROM agent_run_events ORDER BY sequence")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(0, "AgentStart".to_string()), (1, "TurnEnd".to_string())]
        );
        assert_eq!(transport.events_for("session-1").len(), 2);
    }
}
