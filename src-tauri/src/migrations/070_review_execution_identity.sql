-- Semantic reviewers are deliberately not delegated producer attempts. Their
-- durable identity is owned by delegation_review_jobs, while the reviewed
-- artifact stores that distinct execution identity as immutable evidence.
-- v057 incorrectly required verifier_attempt_id to reference
-- delegation_attempts, making every real semantic-review handoff fail.
--
-- Rebuild the artifact and its sole dependent table together so existing
-- review and handoff rows are preserved without disabling foreign keys.

DROP TRIGGER IF EXISTS trg_change_handoff_delivery_identity;
DROP TRIGGER IF EXISTS trg_change_handoff_identity_immutable;
DROP INDEX IF EXISTS idx_change_handoffs_attempt;
DROP INDEX IF EXISTS idx_change_handoffs_status;
ALTER TABLE delegation_change_handoffs RENAME TO delegation_change_handoffs_v069;

DROP TRIGGER IF EXISTS trg_delegation_review_artifact_scope;
DROP TRIGGER IF EXISTS trg_delegation_review_artifact_identity;
DROP TRIGGER IF EXISTS trg_delegation_review_artifact_review_once;
DROP INDEX IF EXISTS idx_delegation_review_artifacts_package;
DROP INDEX IF EXISTS idx_delegation_review_artifacts_review;

CREATE TABLE delegation_review_artifacts_v070 (
    id TEXT PRIMARY KEY,
    schema_version INTEGER NOT NULL CHECK(schema_version > 0),
    work_package_id TEXT NOT NULL REFERENCES work_packages(id) ON DELETE RESTRICT,
    change_set_id TEXT REFERENCES work_package_change_sets(id) ON DELETE RESTRICT,
    implementation_attempt_id TEXT NOT NULL REFERENCES delegation_attempts(id) ON DELETE RESTRICT,
    scope_digest TEXT NOT NULL CHECK(length(scope_digest) > 0),
    base_revision TEXT NOT NULL CHECK(length(base_revision) > 0),
    base_commit TEXT NOT NULL CHECK(length(base_commit) BETWEEN 40 AND 128),
    branch TEXT NOT NULL CHECK(length(branch) > 0),
    audit_ref TEXT NOT NULL CHECK(length(audit_ref) > 0),
    commit_sha TEXT NOT NULL CHECK(length(commit_sha) BETWEEN 40 AND 128),
    diff_hash TEXT NOT NULL CHECK(length(diff_hash) > 0),
    changed_paths_json TEXT NOT NULL,
    allowed_relative_paths_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    verifier_attempt_id TEXT,
    verdict TEXT CHECK(verdict IS NULL OR verdict IN ('passed', 'failed', 'needs_decision')),
    reviewed_commit_sha TEXT,
    reviewed_diff_hash TEXT,
    evidence_refs_json TEXT,
    reviewed_at INTEGER,
    updated_at INTEGER NOT NULL,
    CHECK((verdict IS NULL) = (verifier_attempt_id IS NULL
        AND reviewed_commit_sha IS NULL
        AND reviewed_diff_hash IS NULL
        AND evidence_refs_json IS NULL
        AND reviewed_at IS NULL)),
    UNIQUE(work_package_id, id),
    UNIQUE(change_set_id)
);

INSERT INTO delegation_review_artifacts_v070 (
    id,schema_version,work_package_id,change_set_id,implementation_attempt_id,
    scope_digest,base_revision,base_commit,branch,audit_ref,commit_sha,diff_hash,
    changed_paths_json,allowed_relative_paths_json,created_at,
    verifier_attempt_id,verdict,reviewed_commit_sha,reviewed_diff_hash,
    evidence_refs_json,reviewed_at,updated_at
)
SELECT
    id,schema_version,work_package_id,change_set_id,implementation_attempt_id,
    scope_digest,base_revision,base_commit,branch,audit_ref,commit_sha,diff_hash,
    changed_paths_json,allowed_relative_paths_json,created_at,
    verifier_attempt_id,verdict,reviewed_commit_sha,reviewed_diff_hash,
    evidence_refs_json,reviewed_at,updated_at
FROM delegation_review_artifacts;

CREATE TABLE delegation_change_handoffs_v070 (
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
    artifact_id TEXT REFERENCES delegation_review_artifacts_v070(id) ON DELETE RESTRICT,
    change_set_id TEXT REFERENCES work_package_change_sets(id) ON DELETE RESTRICT,
    base_revision TEXT,
    confirmation_id TEXT REFERENCES work_package_confirmations(id) ON DELETE RESTRICT,
    materialization_id TEXT REFERENCES delegation_materialization_receipts(id) ON DELETE RESTRICT,
    diagnostic_code TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK(status NOT IN ('ready', 'confirmation_pending', 'materialized') OR (
        artifact_id IS NOT NULL AND change_set_id IS NOT NULL AND base_revision IS NOT NULL
    )),
    CHECK(status != 'materialized' OR materialization_id IS NOT NULL),
    UNIQUE(delivery_id, delivery_revision)
);

INSERT INTO delegation_change_handoffs_v070 (
    delivery_id,delivery_revision,implementation_attempt_id,work_package_id,
    semantic_review_job_id,status,artifact_id,change_set_id,base_revision,
    confirmation_id,materialization_id,diagnostic_code,created_at,updated_at
)
SELECT
    delivery_id,delivery_revision,implementation_attempt_id,work_package_id,
    semantic_review_job_id,status,artifact_id,change_set_id,base_revision,
    confirmation_id,materialization_id,diagnostic_code,created_at,updated_at
FROM delegation_change_handoffs_v069;

DROP TABLE delegation_change_handoffs_v069;
DROP TABLE delegation_review_artifacts;
ALTER TABLE delegation_review_artifacts_v070 RENAME TO delegation_review_artifacts;
ALTER TABLE delegation_change_handoffs_v070 RENAME TO delegation_change_handoffs;

CREATE INDEX idx_delegation_review_artifacts_package
    ON delegation_review_artifacts(work_package_id, created_at);
CREATE INDEX idx_delegation_review_artifacts_review
    ON delegation_review_artifacts(verdict, reviewed_at);

CREATE TRIGGER trg_delegation_review_artifact_scope
BEFORE INSERT ON delegation_review_artifacts
FOR EACH ROW BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM work_packages p
        JOIN delegation_attempts a ON a.id = NEW.implementation_attempt_id
        JOIN delegations d ON d.id = a.delegation_id
        WHERE p.id = NEW.work_package_id
          AND d.work_package_id = p.id
          AND p.scope_digest = NEW.scope_digest
    ) THEN RAISE(ABORT, 'review artifact scope mismatch') END;
END;

CREATE TRIGGER trg_delegation_review_artifact_identity
BEFORE UPDATE OF schema_version, work_package_id,
    implementation_attempt_id, scope_digest, base_revision, base_commit,
    branch, audit_ref, commit_sha, diff_hash, changed_paths_json,
    allowed_relative_paths_json, created_at
    ON delegation_review_artifacts
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'review artifact identity is immutable');
END;

CREATE TRIGGER trg_delegation_review_artifact_review_once
BEFORE UPDATE OF verifier_attempt_id, verdict, reviewed_commit_sha,
    reviewed_diff_hash, evidence_refs_json, reviewed_at
    ON delegation_review_artifacts
FOR EACH ROW WHEN OLD.verdict IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'review artifact review is immutable');
END;

CREATE INDEX idx_change_handoffs_attempt
    ON delegation_change_handoffs(implementation_attempt_id, updated_at DESC);
CREATE INDEX idx_change_handoffs_status
    ON delegation_change_handoffs(status, updated_at);

CREATE TRIGGER trg_change_handoff_delivery_identity
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

CREATE TRIGGER trg_change_handoff_identity_immutable
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
