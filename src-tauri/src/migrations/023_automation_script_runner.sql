-- Migration 023: deterministic local script automations and auditable output.
ALTER TABLE automations ADD COLUMN executor_kind TEXT NOT NULL DEFAULT 'agent';
ALTER TABLE automations ADD COLUMN script_path TEXT;
ALTER TABLE automations ADD COLUMN script_args TEXT NOT NULL DEFAULT '[]';
ALTER TABLE automations ADD COLUMN working_dir TEXT;
ALTER TABLE automations ADD COLUMN timeout_seconds INTEGER NOT NULL DEFAULT 300;

ALTER TABLE automation_runs ADD COLUMN exit_code INTEGER;
ALTER TABLE automation_runs ADD COLUMN output TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_automations_executor_kind ON automations(executor_kind);
