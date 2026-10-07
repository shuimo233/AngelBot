-- Migration 011: Proactive agent events and self-scheduling

CREATE TABLE IF NOT EXISTS agent_events (
    id TEXT PRIMARY KEY,
    session_id TEXT REFERENCES sessions(id),
    event_type TEXT NOT NULL CHECK(event_type IN ('file_change','scheduled','idle','user_return')),
    payload TEXT,
    processed INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_events_processed ON agent_events(processed, created_at);
