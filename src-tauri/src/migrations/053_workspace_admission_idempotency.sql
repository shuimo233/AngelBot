-- Workspace admission recovery/idempotency indexes.
--
-- A retry of one attempt must observe at most one active admission. Historical
-- released/expired rows remain append-only audit facts and may be followed by
-- a fresh lease after recovery.
CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_admissions_attempt_active
    ON workspace_admissions(attempt_id)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS idx_workspace_admissions_lease_status
    ON workspace_admissions(lease_id, status, expires_at);
