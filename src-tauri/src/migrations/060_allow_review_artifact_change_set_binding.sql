-- A review artifact is immutable once captured, except for the one-way
-- Main-Agent binding to its derived ChangeSet. The original v057 trigger also
-- froze `change_set_id`, which made the intended capture -> bind -> review ->
-- confirmation flow impossible on every migrated database.
DROP TRIGGER IF EXISTS trg_delegation_review_artifact_identity;

CREATE TRIGGER IF NOT EXISTS trg_delegation_review_artifact_identity
BEFORE UPDATE OF schema_version, work_package_id,
    implementation_attempt_id, scope_digest, base_revision, base_commit,
    branch, audit_ref, commit_sha, diff_hash, changed_paths_json,
    allowed_relative_paths_json, created_at
    ON delegation_review_artifacts
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'review artifact identity is immutable');
END;
