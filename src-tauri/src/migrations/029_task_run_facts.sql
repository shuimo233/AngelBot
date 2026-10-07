-- Migration 029: structured, durable facts for bounded foreground agent tasks.
-- Tool execution remains in agent_steps; this table stores only the compact
-- facts needed to derive task state and construct a future user-authorized slice.
CREATE TABLE task_run_facts (
    task_run_id TEXT PRIMARY KEY NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    facts_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_task_run_facts_session_message
    ON task_run_facts(session_id, message_id);
