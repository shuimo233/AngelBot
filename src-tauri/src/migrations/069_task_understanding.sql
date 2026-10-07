-- Durable, attributable task-understanding records.  These records describe
-- why the Main Agent is taking an action; they never grant execution authority.
CREATE TABLE IF NOT EXISTS task_understanding_records (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    source TEXT NOT NULL,
    scope TEXT NOT NULL,
    status TEXT NOT NULL,
    body TEXT NOT NULL,
    affects_json TEXT NOT NULL DEFAULT '[]',
    replaces_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (workspace_id) REFERENCES projects(id) ON DELETE CASCADE,
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (replaces_id) REFERENCES task_understanding_records(id)
);

CREATE INDEX IF NOT EXISTS idx_task_understanding_active_scope
    ON task_understanding_records(workspace_id, session_id, status, updated_at DESC);
