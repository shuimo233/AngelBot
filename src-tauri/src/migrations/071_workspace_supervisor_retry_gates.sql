-- A retry remains the same user-visible delegation, but receives a fresh
-- attempt and a fresh Workspace Supervisor authorization gate.  v065 made
-- the stable delegation and work-package identities unique, which prevented
-- that valid retry topology.  Keep the execution identities unique while
-- preserving every existing gate during the rebuild.
DROP INDEX IF EXISTS idx_workspace_supervisor_delegations_attempt_gate;
ALTER TABLE workspace_supervisor_delegations
    RENAME TO workspace_supervisor_delegations_v070;

CREATE TABLE workspace_supervisor_delegations_v071 (
    supervisor_work_id TEXT PRIMARY KEY
        REFERENCES workspace_supervisor_work(id) ON DELETE CASCADE,
    delegation_id TEXT NOT NULL
        REFERENCES delegations(id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL UNIQUE
        REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    work_package_id TEXT NOT NULL
        REFERENCES work_packages(id) ON DELETE CASCADE,
    authorized_at INTEGER
);

INSERT INTO workspace_supervisor_delegations_v071 (
    supervisor_work_id, delegation_id, attempt_id, work_package_id, authorized_at
)
SELECT
    supervisor_work_id, delegation_id, attempt_id, work_package_id, authorized_at
FROM workspace_supervisor_delegations_v070;

DROP TABLE workspace_supervisor_delegations_v070;
ALTER TABLE workspace_supervisor_delegations_v071
    RENAME TO workspace_supervisor_delegations;

-- attempt_id has a unique lookup index already.  Index the stable delegation
-- identity instead, so a retry chain can efficiently find its pending gate.
CREATE INDEX idx_workspace_supervisor_delegations_delegation_gate
    ON workspace_supervisor_delegations(delegation_id, authorized_at);
