-- Immutable implementation artifact facts and one-shot independent review.
-- The temporary worktree itself is owned by its provider; this table stores
-- only the reviewed commit/diff identity and bounded evidence references.
CREATE TABLE IF NOT EXISTS delegation_review_artifacts (
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
    verifier_attempt_id TEXT REFERENCES delegation_attempts(id) ON DELETE RESTRICT,
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

CREATE INDEX IF NOT EXISTS idx_delegation_review_artifacts_package
    ON delegation_review_artifacts(work_package_id, created_at);
CREATE INDEX IF NOT EXISTS idx_delegation_review_artifacts_review
    ON delegation_review_artifacts(verdict, reviewed_at);

CREATE TRIGGER IF NOT EXISTS trg_delegation_review_artifact_scope
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

CREATE TRIGGER IF NOT EXISTS trg_delegation_review_artifact_identity
BEFORE UPDATE OF schema_version, work_package_id, change_set_id,
    implementation_attempt_id, scope_digest, base_revision, base_commit,
    branch, audit_ref, commit_sha, diff_hash, changed_paths_json,
    allowed_relative_paths_json, created_at
    ON delegation_review_artifacts
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'review artifact identity is immutable');
END;

CREATE TRIGGER IF NOT EXISTS trg_delegation_review_artifact_review_once
BEFORE UPDATE OF verifier_attempt_id, verdict, reviewed_commit_sha,
    reviewed_diff_hash, evidence_refs_json, reviewed_at
    ON delegation_review_artifacts
FOR EACH ROW WHEN OLD.verdict IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'review artifact review is immutable');
END;
