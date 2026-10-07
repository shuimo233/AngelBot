-- Durable, typed execution intent for explorer attempts.
--
-- `delegation_outbox.payload_json` remains an audit/compatibility envelope;
-- it is intentionally not an execution input. Explorer dispatch resolves this
-- row through the parent/package/attempt/lease binding instead.
CREATE TABLE IF NOT EXISTS delegation_explorer_plans (
    id TEXT PRIMARY KEY,
    delegation_id TEXT NOT NULL UNIQUE
        REFERENCES delegations(id) ON DELETE CASCADE,
    work_package_id TEXT NOT NULL
        REFERENCES work_packages(id) ON DELETE RESTRICT,
    scope_digest TEXT NOT NULL,
    worker_policy_version INTEGER NOT NULL CHECK (worker_policy_version > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    plan_json TEXT NOT NULL CHECK (length(plan_json) > 0),
    plan_digest TEXT NOT NULL CHECK (length(plan_digest) > 0),
    created_at INTEGER NOT NULL,
    UNIQUE(delegation_id, plan_digest)
);

CREATE INDEX IF NOT EXISTS idx_delegation_explorer_plans_package
    ON delegation_explorer_plans(work_package_id, created_at DESC);

-- SQLite cannot express the package/delegation composite relationship with a
-- foreign key alone. New plan rows are admitted only for an explorer package
-- while the delegation is still queued, and immutable scope/policy facts must
-- match the package row exactly.
CREATE TRIGGER IF NOT EXISTS trg_delegation_explorer_plan_binding
BEFORE INSERT ON delegation_explorer_plans
FOR EACH ROW BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM delegations d
        JOIN work_packages p ON p.id = d.work_package_id
        WHERE d.id = NEW.delegation_id
          AND d.work_package_id = NEW.work_package_id
          AND d.status = 'queued'
          AND p.task_shape = 'explore'
          AND p.worker_profile = 'explorer'
          AND p.scope_digest = NEW.scope_digest
          AND p.worker_policy_version = NEW.worker_policy_version
    ) THEN RAISE(ABORT, 'explorer plan binding mismatch') END;
END;

-- Plans are immutable after issuance. A replacement is a new delegation or
-- retry attempt, never an update to the authority of a queued attempt.
CREATE TRIGGER IF NOT EXISTS trg_delegation_explorer_plan_immutable
BEFORE UPDATE ON delegation_explorer_plans
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'explorer plan is immutable');
END;
