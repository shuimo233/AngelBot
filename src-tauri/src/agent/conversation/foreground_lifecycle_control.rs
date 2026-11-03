//! Foreground dispatch and terminal lifecycle control.
//!
//! The runner may stream ordinary progress immediately, but terminal UI facts
//! are held until the command host has persisted the assistant reply and its
//! task-run projection. `commit_terminal` then atomically writes the queued
//! audit facts before forwarding them to the UI transport.

use std::sync::{Arc, Mutex};

use super::event::{AgentEvent, AgentEventEmitter, AgentRunJournal};
use super::lifecycle::{BeforeToolCallResult, ToolCallContext, ToolLifecycleHook};

/// Owns external dispatch reservations and terminal durability for one run.
///
/// The existing tool hook reserves actual MCP dispatches in the same journal;
/// ordinary progress remains best-effort. Callers stream through
/// `AgentEventEmitter`, persist the reply/projection, then call
/// `commit_terminal`. A failed commit keeps terminal events hidden and leaves
/// a recoverable, uncommitted terminal state for the caller to report.
pub struct ForegroundLifecycleControl {
    transport: Arc<dyn AgentEventEmitter>,
    journal: AgentRunJournal,
    /// Main's admitted reply, not the generic runner's ephemeral message ID.
    reply_id: String,
    terminal_events: Mutex<Vec<(String, AgentEvent)>>,
}

impl ForegroundLifecycleControl {
    pub fn new(
        transport: Arc<dyn AgentEventEmitter>,
        db: Arc<Mutex<rusqlite::Connection>>,
        reply_id: String,
    ) -> Self {
        Self {
            transport,
            journal: AgentRunJournal::new(db, reply_id.clone()),
            reply_id,
            terminal_events: Mutex::new(Vec::new()),
        }
    }

    /// Make terminal UI facts visible only after the caller has durably
    /// projected the reply. Audit persistence is committed first, as one
    /// ordered transaction; failure deliberately suppresses every queued
    /// terminal event.
    pub fn commit_terminal(&self) -> Result<(), String> {
        let queued = self
            .terminal_events
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        if queued.is_empty() {
            return Ok(());
        }
        let session_id = &queued[0].0;
        if queued.iter().any(|(session, _)| session != session_id) {
            return Err("foreground terminal events span multiple sessions".to_string());
        }
        let events = queued
            .iter()
            .map(|(_, event)| event.clone())
            .collect::<Vec<_>>();
        self.journal.append_batch(session_id, &events)?;
        for (session_id, event) in &queued {
            self.transport.emit(session_id, event.clone());
        }
        self.terminal_events
            .lock()
            .map_err(|error| error.to_string())?
            .clear();
        Ok(())
    }

    fn is_terminal_ui_event(event: &AgentEvent) -> bool {
        matches!(
            event,
            AgentEvent::MessageEnd { .. }
                | AgentEvent::TurnEnd { .. }
                | AgentEvent::AgentEnd { .. }
        )
    }
}

impl ToolLifecycleHook for ForegroundLifecycleControl {
    fn before_tool_call(&self, ctx: &ToolCallContext) -> BeforeToolCallResult {
        if !ctx.tool_name.starts_with("mcp_") {
            return BeforeToolCallResult::default();
        }
        let reserved = match (ctx.session_id.as_deref(), ctx.tool_call.id.as_deref()) {
            (Some(session_id), Some(call_id)) if !session_id.is_empty() && !call_id.is_empty() => {
                self.journal
                    .append(
                        session_id,
                        &AgentEvent::ExternalToolDispatchReserved {
                            call_id: call_id.to_string(),
                            tool_name: ctx.tool_name.clone(),
                        },
                    )
                    .is_ok()
            }
            _ => false,
        };
        BeforeToolCallResult {
            block: !reserved,
            reason: (!reserved).then(|| {
                "无法安全记录外部工具调用，本次未派发。请检查本地存储后再发起。".to_string()
            }),
        }
    }
}

impl AgentEventEmitter for ForegroundLifecycleControl {
    fn emit(&self, session_id: &str, mut event: AgentEvent) {
        // UI previews and confirmations address durable replies. Keep runner
        // turn/call identities intact while binding every message projection
        // to the same host-admitted parent, including journaled terminal facts.
        match &mut event {
            AgentEvent::MessageStart { message_id, .. }
            | AgentEvent::MessageDelta { message_id, .. }
            | AgentEvent::ContentBlock { message_id, .. }
            | AgentEvent::MessageUpdate { message_id, .. }
            | AgentEvent::MessageEnd { message_id, .. } => message_id.clone_from(&self.reply_id),
            _ => {}
        }
        if Self::is_terminal_ui_event(&event) {
            if let Ok(mut terminal_events) = self.terminal_events.lock() {
                terminal_events.push((session_id.to_string(), event));
            }
            return;
        }
        // Progress remains best-effort, preserving existing streaming behavior.
        if let Err(error) = self.journal.append(session_id, &event) {
            eprintln!("[AgentRunEvent] failed to persist event: {error}");
        }
        self.transport.emit(session_id, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingEmitter(Mutex<Vec<AgentEvent>>);

    impl AgentEventEmitter for RecordingEmitter {
        fn emit(&self, _session_id: &str, event: AgentEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn database() -> Arc<Mutex<rusqlite::Connection>> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_run_events (
                id TEXT PRIMARY KEY, run_id TEXT NOT NULL, session_id TEXT NOT NULL,
                sequence INTEGER NOT NULL, event_type TEXT NOT NULL, payload TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        Arc::new(Mutex::new(conn))
    }

    fn terminal_event() -> AgentEvent {
        AgentEvent::AgentEnd {
            session_id: "session-1".to_string(),
            summary: Some("done".to_string()),
            total_turns: 1,
            total_tool_calls: 0,
        }
    }

    #[test]
    fn terminal_event_is_persisted_before_it_is_exposed() {
        let db = database();
        let transport = Arc::new(RecordingEmitter::default());
        let control =
            ForegroundLifecycleControl::new(transport.clone(), db.clone(), "run-1".into());
        control.emit("session-1", terminal_event());

        assert!(transport.0.lock().unwrap().is_empty());
        assert_eq!(
            db.lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM agent_run_events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );

        control.commit_terminal().unwrap();
        assert_eq!(
            db.lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM agent_run_events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(transport.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn foreground_message_events_use_the_durable_reply_identity() {
        let db = database();
        let transport = Arc::new(RecordingEmitter::default());
        let control =
            ForegroundLifecycleControl::new(transport.clone(), db.clone(), "saved-reply".into());
        let events = vec![
            AgentEvent::MessageStart {
                turn_id: "runner-turn".into(),
                message_id: "ephemeral".into(),
            },
            AgentEvent::MessageDelta {
                turn_id: "runner-turn".into(),
                message_id: "ephemeral".into(),
                delta: "内容".into(),
            },
            AgentEvent::ContentBlock {
                turn_id: "runner-turn".into(),
                message_id: "ephemeral".into(),
                index: 0,
                iteration_id: 0,
                block: crate::agent::event::ContentBlockData::Text {
                    content: "内容".into(),
                },
            },
            AgentEvent::MessageUpdate {
                turn_id: "runner-turn".into(),
                message_id: "ephemeral".into(),
                content: "内容".into(),
                is_final: true,
            },
            AgentEvent::MessageEnd {
                turn_id: "runner-turn".into(),
                message_id: "ephemeral".into(),
                full_text: "内容".into(),
            },
        ];
        for event in events {
            control.emit("session-1", event);
        }
        assert_eq!(
            transport.0.lock().unwrap().len(),
            4,
            "terminal visibility barrier still applies"
        );
        control.commit_terminal().unwrap();
        for event in transport.0.lock().unwrap().iter() {
            let value = serde_json::to_value(event).unwrap();
            assert_eq!(value["data"]["message_id"], "saved-reply");
            assert_eq!(value["data"]["turn_id"], "runner-turn");
        }
        let conn = db.lock().unwrap();
        let mut statement = conn
            .prepare("SELECT payload FROM agent_run_events ORDER BY sequence")
            .unwrap();
        let payloads = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap();
        for payload in payloads {
            let value: serde_json::Value = serde_json::from_str(&payload.unwrap()).unwrap();
            assert_eq!(value["data"]["message_id"], "saved-reply");
        }
    }

    #[test]
    fn persistence_failure_suppresses_terminal_completion_events() {
        let db = database();
        let transport = Arc::new(RecordingEmitter::default());
        let control =
            ForegroundLifecycleControl::new(transport.clone(), db.clone(), "run-1".into());
        control.emit("session-1", terminal_event());
        db.lock()
            .unwrap()
            .execute_batch("DROP TABLE agent_run_events;")
            .unwrap();

        assert!(control.commit_terminal().is_err());
        assert!(transport.0.lock().unwrap().is_empty());
    }
}
