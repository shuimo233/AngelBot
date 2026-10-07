-- Durable admission leases for delegated workspaces.
--
-- The admission row is an execution lease, not a copy of WorkPackage or
-- capability-lease facts.  It references those authoritative records while
-- keeping the concurrency boundary (workspace key + mode) explicit.
CREATE TABLE IF NOT EXISTS workspace_admissions (
    id TEXT PRIMARY KEY,
    workspace_key TEXT NOT NULL,
    work_package_id TEXT NOT NULL REFERENCES work_packages(id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    lease_id TEXT NOT NULL REFERENCES delegation_capability_leases(id) ON DELETE CASCADE,
    mode TEXT NOT NULL CHECK(mode IN ('read', 'review', 'write', 'materialize')),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK(status IN ('active', 'released', 'expired', 'revoked')),
    state_version INTEGER NOT NULL DEFAULT 0 CHECK(state_version >= 0),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    released_at INTEGER,
    UNIQUE(id, work_package_id, attempt_id, lease_id),
    CHECK(expires_at > created_at),
    CHECK((status = 'active' AND released_at IS NULL)
        OR (status != 'active' AND released_at IS NOT NULL))
);

CREATE INDEX IF NOT EXISTS idx_workspace_admissions_scope
    ON workspace_admissions(workspace_key, status, mode, expires_at);
CREATE INDEX IF NOT EXISTS idx_workspace_admissions_package
    ON workspace_admissions(work_package_id, created_at DESC);

-- A canonical workspace may have multiple observers, but only one candidate
-- writer/materializer at a time. The admission service additionally enforces
-- the global max-active boundary in one BEGIN IMMEDIATE transaction.
CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_admissions_writer_serial
    ON workspace_admissions(workspace_key)
    WHERE status = 'active' AND mode IN ('write', 'materialize');

CREATE TRIGGER IF NOT EXISTS trg_workspace_admission_scope_matches_package
BEFORE INSERT ON workspace_admissions
FOR EACH ROW
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM work_packages p
        WHERE p.id = NEW.work_package_id
          AND p.workspace_key = NEW.workspace_key
    ) THEN RAISE(ABORT, 'workspace admission scope must match work package') END;
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM delegation_attempts a
        JOIN delegations d ON d.id = a.delegation_id
        WHERE a.id = NEW.attempt_id
          AND d.work_package_id = NEW.work_package_id
    ) THEN RAISE(ABORT, 'workspace admission attempt must belong to work package') END;
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM delegation_capability_leases l
        WHERE l.id = NEW.lease_id
          AND l.attempt_id = NEW.attempt_id
    ) THEN RAISE(ABORT, 'workspace admission lease must belong to attempt') END;
END;
