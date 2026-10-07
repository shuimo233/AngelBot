-- Interaction input and executable work are intentionally separate queues.
-- Main Agent supervision creates these work records only after it has decided
-- that delegated work is appropriate for a Workspace.
CREATE TABLE IF NOT EXISTS workspace_supervisor_work (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    source_input_id TEXT REFERENCES workspace_supervisor_inputs(id) ON DELETE SET NULL,
    kind TEXT NOT NULL CHECK(kind IN ('explore', 'candidate_implementation', 'review')),
    objective TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued', 'claimed', 'completed', 'cancelled')),
    claim_token TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_workspace_supervisor_work_next
    ON workspace_supervisor_work(workspace_id, status, created_at ASC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_supervisor_work_claim
    ON workspace_supervisor_work(claim_token) WHERE claim_token IS NOT NULL;
