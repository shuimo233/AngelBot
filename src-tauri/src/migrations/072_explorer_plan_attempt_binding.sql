-- Explorer plans are execution input for one immutable attempt, not a stable
-- delegation.  A retry keeps its delegation and package identities, but must
-- receive a fresh plan binding for its fresh lease epoch.
--
-- SQLite cannot add the non-null attempt foreign key or remove the old
-- delegation uniqueness in place, so rebuild while retaining every durable
-- plan.  v054 did not persist an attempt id; its one plan per delegation was
-- necessarily issued for the first attempt, which is the conservative binding
-- for historic rows even if a legacy retry was later recorded.
DROP TRIGGER IF EXISTS trg_delegation_explorer_plan_binding;
DROP TRIGGER IF EXISTS trg_delegation_explorer_plan_immutable;

ALTER TABLE delegation_explorer_plans
    RENAME TO delegation_explorer_plans_v071;

CREATE TABLE delegation_explorer_plans_v072 (
    id TEXT PRIMARY KEY,
    delegation_id TEXT NOT NULL
        REFERENCES delegations(id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL
        REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    work_package_id TEXT NOT NULL
        REFERENCES work_packages(id) ON DELETE RESTRICT,
    scope_digest TEXT NOT NULL,
    worker_policy_version INTEGER NOT NULL CHECK (worker_policy_version > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    plan_json TEXT NOT NULL CHECK (length(plan_json) > 0),
    plan_digest TEXT NOT NULL CHECK (length(plan_digest) > 0),
    created_at INTEGER NOT NULL
);

-- Copy before recreating the admission trigger: historical plans can belong
-- to attempts that are already terminal, while future inserts must be queued.
-- The scalar subquery deliberately fails the migration if a durable plan has
-- no parent attempt, rather than silently discarding or weakening that row.
INSERT INTO delegation_explorer_plans_v072 (
    id, delegation_id, attempt_id, work_package_id, scope_digest,
    worker_policy_version, schema_version, plan_json, plan_digest, created_at
)
SELECT
    ep.id,
    ep.delegation_id,
    (
        SELECT a.id
        FROM delegation_attempts a
        WHERE a.delegation_id = ep.delegation_id
        ORDER BY a.attempt_number ASC, a.created_at ASC, a.id ASC
        LIMIT 1
    ),
    ep.work_package_id,
    ep.scope_digest,
    ep.worker_policy_version,
    ep.schema_version,
    ep.plan_json,
    ep.plan_digest,
    ep.created_at
FROM delegation_explorer_plans_v071 ep;

DROP TABLE delegation_explorer_plans_v071;
ALTER TABLE delegation_explorer_plans_v072
    RENAME TO delegation_explorer_plans;

-- One typed plan is immutable per attempt.  Retried attempts may intentionally
-- carry the same plan digest as their source attempt, so digest uniqueness is
-- scoped only by the fresh attempt identity.
CREATE UNIQUE INDEX idx_delegation_explorer_plans_attempt
    ON delegation_explorer_plans(attempt_id);
CREATE INDEX idx_delegation_explorer_plans_delegation_attempt
    ON delegation_explorer_plans(delegation_id, attempt_id);
CREATE INDEX idx_delegation_explorer_plans_package
    ON delegation_explorer_plans(work_package_id, created_at DESC);

-- SQLite cannot express the package/delegation/attempt composite relationship
-- with foreign keys alone.  Admit a new plan only for its own queued explorer
-- attempt under the immutable work-package policy envelope.
CREATE TRIGGER trg_delegation_explorer_plan_binding
BEFORE INSERT ON delegation_explorer_plans
FOR EACH ROW BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM delegations d
        JOIN delegation_attempts a
          ON a.id = NEW.attempt_id
         AND a.delegation_id = d.id
        JOIN work_packages p ON p.id = d.work_package_id
        WHERE d.id = NEW.delegation_id
          AND d.work_package_id = NEW.work_package_id
          AND d.status = 'queued'
          AND a.status = 'queued'
          AND p.task_shape = 'explore'
          AND p.worker_profile = 'explorer'
          AND p.scope_digest = NEW.scope_digest
          AND p.worker_policy_version = NEW.worker_policy_version
    ) THEN RAISE(ABORT, 'explorer plan binding mismatch') END;
END;

-- Plans are immutable after issuance. A retry is a new attempt with a new
-- record, never an update to the authority of an existing attempt.
CREATE TRIGGER trg_delegation_explorer_plan_immutable
BEFORE UPDATE ON delegation_explorer_plans
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'explorer plan is immutable');
END;
