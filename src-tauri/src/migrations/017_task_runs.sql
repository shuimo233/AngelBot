-- Migration 017: Durable task runs
-- Stores task-level summaries for agent runs so UI state can be restored without
-- re-deriving every workflow from message-local steps.

CREATE TABLE IF NOT EXISTS task_runs (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    goal TEXT NOT NULL,
    status TEXT NOT NULL,
    plan TEXT NOT NULL,
    confirmation_state TEXT NOT NULL DEFAULT 'none',
    resumable INTEGER NOT NULL DEFAULT 0,
    step_count INTEGER NOT NULL DEFAULT 0,
    completed_step_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_task_runs_session ON task_runs(session_id);
CREATE INDEX IF NOT EXISTS idx_task_runs_message ON task_runs(message_id);
