-- Durable Main-Agent hand-off for a reviewed Change delivery.
--
-- This records the one-way transition from semantic review to an immutable
-- candidate, then to an explicit parent confirmation and materialization. It
-- intentionally does not store a path, model transcript, or worktree handle.
CREATE TABLE IF NOT EXISTS delegation_change_handoffs (
    delivery_id TEXT PRIMARY KEY
        REFERENCES delegation_deliveries(id) ON DELETE CASCADE,
    delivery_revision INTEGER NOT NULL CHECK(delivery_revision > 0),
    implementation_attempt_id TEXT NOT NULL
        REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    work_package_id TEXT NOT NULL
        REFERENCES work_packages(id) ON DELETE CASCADE,
    semantic_review_job_id TEXT NOT NULL
        REFERENCES delegation_review_jobs(id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK(status IN (
        'preparing', 'ready', 'confirmation_pending', 'materialized',
        'needs_decision', 'cancelled'
    )),
    artifact_id TEXT REFERENCES delegation_review_artifacts(id) ON DELETE RESTRICT,
    change_set_id TEXT REFERENCES work_package_change_sets(id) ON DELETE RESTRICT,
    base_revision TEXT,
    confirmation_id TEXT REFERENCES work_package_confirmations(id) ON DELETE RESTRICT,
    materialization_id TEXT REFERENCES delegation_materialization_receipts(id) ON DELETE RESTRICT,
    diagnostic_code TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK((status = 'ready') = (
        artifact_id IS NOT NULL AND change_set_id IS NOT NULL AND base_revision IS NOT NULL
    )),
    CHECK(status != 'materialized' OR materialization_id IS NOT NULL),
    UNIQUE(delivery_id, delivery_revision)
);

CREATE INDEX IF NOT EXISTS idx_change_handoffs_attempt
    ON delegation_change_handoffs(implementation_attempt_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_change_handoffs_status
    ON delegation_change_handoffs(status, updated_at);

CREATE TRIGGER IF NOT EXISTS trg_change_handoff_delivery_identity
BEFORE INSERT ON delegation_change_handoffs
FOR EACH ROW BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM delegation_deliveries d
        JOIN delegations g ON g.id = d.delegation_id
        WHERE d.id = NEW.delivery_id
          AND d.attempt_id = NEW.implementation_attempt_id
          AND g.work_package_id = NEW.work_package_id
    ) THEN RAISE(ABORT, 'change handoff must match delivery attempt and work package') END;
END;

CREATE TRIGGER IF NOT EXISTS trg_change_handoff_identity_immutable
BEFORE UPDATE ON delegation_change_handoffs
FOR EACH ROW WHEN NEW.delivery_id != OLD.delivery_id
  OR NEW.delivery_revision != OLD.delivery_revision
  OR NEW.implementation_attempt_id != OLD.implementation_attempt_id
  OR NEW.work_package_id != OLD.work_package_id
  OR NEW.semantic_review_job_id != OLD.semantic_review_job_id
  OR NEW.created_at != OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'change handoff identity is immutable');
END;
