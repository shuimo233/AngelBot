-- Workspace Supervisor control plane.
--
-- Input sequencing and user-safe activity projections belong to the Workspace,
-- not to an implementation session or worker attempt. Existing work-package
-- tables remain the execution adapters during the staged migration.
CREATE TABLE IF NOT EXISTS workspace_supervisor_inputs (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('steer', 'follow_up', 'user_task')),
    content TEXT NOT NULL,
    priority INTEGER NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued', 'claimed', 'completed', 'cancelled')),
    claim_token TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_workspace_supervisor_inputs_next
    ON workspace_supervisor_inputs(workspace_id, status, priority DESC, created_at ASC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_supervisor_inputs_claim
    ON workspace_supervisor_inputs(claim_token) WHERE claim_token IS NOT NULL;

CREATE TABLE IF NOT EXISTS workspace_activity_projections (
    workspace_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    work_package_id TEXT NOT NULL,
    objective TEXT NOT NULL,
    stage TEXT NOT NULL CHECK(stage IN (
        'understanding', 'processing', 'delegated_exploration',
        'candidate_implementation', 'review', 'waiting_decision', 'completed'
    )),
    summary TEXT NOT NULL,
    artifact_refs_json TEXT NOT NULL DEFAULT '[]',
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(workspace_id, work_package_id)
);

CREATE INDEX IF NOT EXISTS idx_workspace_activity_projections_recent
    ON workspace_activity_projections(workspace_id, updated_at DESC);
