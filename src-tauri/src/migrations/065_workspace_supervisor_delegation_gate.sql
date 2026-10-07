-- A delegated outbox may be created only after Main Agent admission, but it
-- becomes executable only after the Workspace Supervisor has claimed and
-- authorized its corresponding work item.  The link is inserted in the same
-- transaction as the delegation/outbox facts, so a concurrent runtime can
-- never observe an ungated supervisor-owned attempt.
CREATE TABLE IF NOT EXISTS workspace_supervisor_delegations (
    supervisor_work_id TEXT PRIMARY KEY
        REFERENCES workspace_supervisor_work(id) ON DELETE CASCADE,
    delegation_id TEXT NOT NULL UNIQUE
        REFERENCES delegations(id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL UNIQUE
        REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    work_package_id TEXT NOT NULL UNIQUE
        REFERENCES work_packages(id) ON DELETE CASCADE,
    authorized_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_workspace_supervisor_delegations_attempt_gate
    ON workspace_supervisor_delegations(attempt_id, authorized_at);
