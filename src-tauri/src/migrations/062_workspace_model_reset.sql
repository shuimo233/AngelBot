-- Workspace model reset
--
-- User-visible conversations are now owned by a Workspace. The former
-- session-first model is intentionally discarded: legacy conversations and
-- their dependent agent runtime records are removed before a clean Personal
-- workspace is created. User profile, durable memories, automations, and
-- settings are outside this reset.

-- Add the workspace ownership columns before clearing legacy rows. Historical
-- migration-recovery fixtures can have an older `projects` shape, while the
-- reset below must detach the new reverse session pointer safely.
ALTER TABLE projects ADD COLUMN kind TEXT NOT NULL DEFAULT 'project';
ALTER TABLE projects ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE projects ADD COLUMN active_session_id TEXT REFERENCES sessions(id);
ALTER TABLE sessions ADD COLUMN project_id TEXT REFERENCES projects(id);

-- Remove the delegation graph first. Several historical tables predate
-- complete cascading foreign keys, so relying on DELETE sessions alone would
-- leave migration behavior dependent on old schema versions.
DELETE FROM delegation_change_handoffs;
DELETE FROM delegation_review_jobs;
DELETE FROM delegation_review_artifacts;
DELETE FROM delegation_materialization_receipts;
DELETE FROM delegation_resource_bindings;
DELETE FROM delegation_explorer_plans;
DELETE FROM workspace_admissions;
DELETE FROM delegated_model_bindings;
DELETE FROM work_package_confirmations;
DELETE FROM work_package_change_sets;
DELETE FROM work_package_candidate_sets;
DELETE FROM delegations;
DELETE FROM work_packages;
DELETE FROM attention_diagnostic_evidence;
DELETE FROM attention_states;
DELETE FROM agent_run_events;
DELETE FROM task_run_facts;
DELETE FROM task_runs;
DELETE FROM agent_events;
DELETE FROM agent_steps;
DELETE FROM usage_stats;
DELETE FROM evolution_proposals;
DELETE FROM smart_zone_log;
DELETE FROM context_summaries;
-- `sessions.leaf_message_id` is an historical NO ACTION reference. Clear the
-- cached pointer before removing messages; normal session deletion cannot do
-- this after its dependent-cleanup trigger has started.
UPDATE sessions SET leaf_message_id = NULL;
DELETE FROM messages;
-- The established sessions cleanup trigger clears self-referential goals and
-- branches in the required order.
-- Workspace pointers are a reverse reference to sessions, so detach them
-- before the legacy sessions are removed.
UPDATE projects SET active_session_id = NULL;
DELETE FROM sessions;
DELETE FROM projects;

CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_unique_project_path
    ON projects(path) WHERE kind = 'project';
CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_single_personal
    ON projects(kind) WHERE kind = 'personal';
CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id, updated_at DESC);

INSERT INTO projects (id, name, path, created_at, kind, updated_at, active_session_id)
VALUES ('personal', 'AngelBot 日常', '', unixepoch(), 'personal', unixepoch(), NULL);

INSERT INTO sessions (
    id, title, created_at, updated_at, context_version, work_dir, project_id
) VALUES ('personal-main', '', unixepoch(), unixepoch(), 0, NULL, 'personal');

UPDATE projects SET active_session_id = 'personal-main' WHERE id = 'personal';
