-- Migration 007: Agent thought chain logging
-- Persists every tool execution step from the agent runner loop.

CREATE TABLE IF NOT EXISTS agent_steps (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    thought TEXT,
    tool_name TEXT NOT NULL,
    tool_input TEXT,
    tool_output TEXT,
    success INTEGER NOT NULL DEFAULT 1,
    latency_ms INTEGER,
    seq INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_steps_session ON agent_steps(session_id);
