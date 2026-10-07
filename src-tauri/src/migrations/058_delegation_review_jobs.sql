-- Durable semantic review jobs for every producer delivery.
--
-- A review job is an internal quality gate, not a user-visible delegation and
-- not a retry attempt.  The subject is a bounded structured snapshot of the
-- producer result; raw conversation/tool logs are intentionally excluded.
CREATE TABLE IF NOT EXISTS delegation_review_jobs (
    id TEXT PRIMARY KEY,
    implementation_attempt_id TEXT NOT NULL
        REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    delivery_id TEXT NOT NULL
        REFERENCES delegation_deliveries(id) ON DELETE CASCADE,
    delivery_revision INTEGER NOT NULL CHECK(delivery_revision > 0),
    subject_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN (
        'queued', 'running', 'passed', 'failed', 'needs_decision', 'cancelled'
    )),
    reviewer_attempt_id TEXT,
    outcome_json TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    started_at INTEGER,
    ended_at INTEGER,
    UNIQUE(delivery_id, delivery_revision),
    CHECK((status IN ('passed', 'failed', 'needs_decision')) = (outcome_json IS NOT NULL)),
    CHECK(status NOT IN ('queued', 'running') OR ended_at IS NULL)
);

CREATE INDEX IF NOT EXISTS idx_delegation_review_jobs_attempt
    ON delegation_review_jobs(implementation_attempt_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_delegation_review_jobs_status
    ON delegation_review_jobs(status, updated_at);

CREATE TRIGGER IF NOT EXISTS trg_review_job_delivery_matches_attempt
BEFORE INSERT ON delegation_review_jobs
FOR EACH ROW BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM delegation_deliveries d
        WHERE d.id = NEW.delivery_id
          AND d.attempt_id = NEW.implementation_attempt_id
    ) THEN RAISE(ABORT, 'review job delivery must match implementation attempt') END;
END;

CREATE TRIGGER IF NOT EXISTS trg_review_job_identity_immutable
BEFORE UPDATE ON delegation_review_jobs
FOR EACH ROW WHEN NEW.implementation_attempt_id != OLD.implementation_attempt_id
  OR NEW.delivery_id != OLD.delivery_id
  OR NEW.delivery_revision != OLD.delivery_revision
  OR NEW.subject_json != OLD.subject_json
  OR NEW.created_at != OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'review job identity is immutable');
END;
