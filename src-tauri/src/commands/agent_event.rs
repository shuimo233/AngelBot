//! Agent event commands for real-time UI updates
//!
//! Provides Tauri commands to subscribe to agent events and manage event channels.

use crate::agent::event::{AgentEvent, EventChannel};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

/// Response from get_agent_events
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentEventsResponse {
    pub events: Vec<AgentEvent>,
    pub count: usize,
}

/// Immutable lifecycle record for one durable Agent run.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentRunEventRecord {
    pub id: String,
    pub run_id: String,
    pub session_id: String,
    pub sequence: i64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: i64,
}

pub(crate) fn get_agent_run_events_impl(
    state: &crate::AppState,
    session_id: &str,
    run_id: &str,
    after_sequence: Option<i64>,
) -> Result<Vec<AgentRunEventRecord>, String> {
    let conn = state.db.lock().map_err(|error| error.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, run_id, session_id, sequence, event_type, payload, created_at
             FROM agent_run_events
             WHERE session_id = ?1 AND run_id = ?2 AND sequence > COALESCE(?3, -1)
             ORDER BY sequence ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = stmt
        .query_map(
            rusqlite::params![session_id, run_id, after_sequence],
            |row| {
                let payload: String = row.get(5)?;
                Ok(AgentRunEventRecord {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    session_id: row.get(2)?,
                    sequence: row.get(3)?,
                    event_type: row.get(4)?,
                    payload: serde_json::from_str(&payload)
                        .unwrap_or_else(|_| serde_json::Value::String(payload)),
                    created_at: row.get(6)?,
                })
            },
        )
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Read the append-only lifecycle journal for a single Agent run. Consumers
/// resume with `after_sequence` rather than relying on an in-memory event count.
#[tauri::command]
pub fn get_agent_run_events(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    run_id: String,
    after_sequence: Option<i64>,
) -> Result<Vec<AgentRunEventRecord>, String> {
    get_agent_run_events_impl(&state, &session_id, &run_id, after_sequence)
}

/// Get all events from the dev event store for a session. The
/// frontend polls this on the `channelId = "session-<id>"` form to
/// recover events that arrived while no `TauriEventEmitter` listener
/// was attached. This is the only agent-event command the frontend
/// actually invokes; the `create_event_channel`,
/// `subscribe_agent_events`, and `clear_agent_events` shims were
/// removed as dead surface area (#101).
#[tauri::command]
pub fn get_agent_events(
    state: tauri::State<'_, crate::AppState>,
    channel_id: String,
) -> Result<AgentEventsResponse, String> {
    let session_id = channel_id.strip_prefix("session-").unwrap_or(&channel_id);
    Ok(AgentEventsResponse {
        events: state
            .dev_event_store
            .as_ref()
            .map(|store| store.events_for(session_id))
            .unwrap_or_default(),
        count: state
            .dev_event_store
            .as_ref()
            .map(|store| store.events_for(session_id).len())
            .unwrap_or_default(),
    })
}

/// Emit an agent event to the frontend
pub fn emit_agent_event(app: &AppHandle, session_id: &str, event: AgentEvent) {
    let event_name = format!("agent_event_{}", session_id);
    if let Err(e) = app.emit(&event_name, &event) {
        eprintln!("[EventChannel] Failed to emit event: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_channel_creation() {
        let channel = EventChannel::new();
        channel.emit(AgentEvent::AgentStart {
            session_id: "test-session".to_string(),
            timestamp: 1234567890,
        });
        let events = channel.get_events();
        assert_eq!(events.len(), 1);
    }
}
