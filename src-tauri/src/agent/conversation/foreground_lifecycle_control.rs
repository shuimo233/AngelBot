//! Foreground terminal lifecycle control.
//!
//! The runner may stream ordinary progress immediately, but terminal UI facts
//! are held until the command host has persisted the assistant reply and its
//! task-run projection. `commit_terminal` then atomically writes the queued
//! audit facts before forwarding them to the UI transport.

use std::sync::{Arc, Mutex};

use super::event::{AgentEvent, AgentEventEmitter, AgentRunJournal};

/// Owns the foreground terminal durability barrier for one run.
///
/// Its interface intentionally has one commit operation: callers stream into
/// it through `AgentEventEmitter`, persist the reply/projection, then call
/// `commit_terminal`. A failed commit keeps terminal events hidden and leaves
/// a recoverable, uncommitted terminal state for the caller to report.
pub struct ForegroundLifecycleControl {
    transport: Arc<dyn AgentEventEmitter>,
    journal: AgentRunJournal,
    terminal_events: Mutex<Vec<(String, AgentEvent)>>,
}

impl ForegroundLifecycleControl {
    pub fn new(
        transport: Arc<dyn AgentEventEmitter>,
        db: Arc<Mutex<rusqlite::Connection>>,
        run_id: String,
    ) -> Self {
        Self {
            transport,
            journal: AgentRunJournal::new(db, run_id),
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

impl AgentEventEmitter for ForegroundLifecycleControl {
    fn emit(&self, session_id: &str, event: AgentEvent) {
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
