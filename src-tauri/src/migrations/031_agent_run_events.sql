-- Migration 031: durable Agent runtime event journal.
-- `agent_events` is the proactive-event inbox; lifecycle events are immutable
-- audit facts and must not be consumed or deleted as inbox work.

CREATE TABLE IF NOT EXISTS agent_run_events (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(run_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_agent_run_events_session_created
    ON agent_run_events(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_agent_run_events_run_sequence
    ON agent_run_events(run_id, sequence);
