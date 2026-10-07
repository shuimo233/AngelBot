-- Migration 018: Durable personal automations and run history.
CREATE TABLE IF NOT EXISTS automations (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    prompt TEXT NOT NULL,
    trigger_kind TEXT NOT NULL,
    trigger_value TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    permission_summary TEXT NOT NULL DEFAULT '',
    next_run_at INTEGER,
    last_run_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS automation_runs (
    id TEXT PRIMARY KEY,
    automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_automations_enabled_next_run ON automations(enabled, next_run_at);
CREATE INDEX IF NOT EXISTS idx_automation_runs_automation ON automation_runs(automation_id, started_at DESC);
