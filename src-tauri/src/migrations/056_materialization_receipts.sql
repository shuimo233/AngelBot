-- Durable, idempotent result of the only operation allowed to write the
-- canonical workspace.  The receipt stores immutable reviewed facts so a
-- restart never has to infer what a prior cherry-pick may have done.
CREATE TABLE IF NOT EXISTS delegation_materialization_receipts (
    id TEXT PRIMARY KEY,
    confirmation_id TEXT NOT NULL UNIQUE
        REFERENCES work_package_confirmations(id) ON DELETE RESTRICT,
    change_set_id TEXT NOT NULL
        REFERENCES work_package_change_sets(id) ON DELETE RESTRICT,
    work_package_id TEXT NOT NULL
        REFERENCES work_packages(id) ON DELETE RESTRICT,
    scope_digest TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    base_commit TEXT NOT NULL,
    reviewed_commit_sha TEXT NOT NULL,
    reviewed_diff_hash TEXT NOT NULL,
    changed_paths_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('applying', 'applied', 'refused', 'unknown')),
    error_class TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    applied_at INTEGER,
    CHECK(length(scope_digest) > 0),
    CHECK(length(artifact_id) > 0),
    CHECK(length(base_commit) > 0),
    CHECK(length(reviewed_commit_sha) > 0),
    CHECK(length(reviewed_diff_hash) > 0)
);

CREATE INDEX IF NOT EXISTS idx_materialization_receipts_package
    ON delegation_materialization_receipts(work_package_id, status);

CREATE TRIGGER IF NOT EXISTS trg_materialization_receipt_immutable
BEFORE UPDATE ON delegation_materialization_receipts
FOR EACH ROW
WHEN NEW.confirmation_id != OLD.confirmation_id
  OR NEW.change_set_id != OLD.change_set_id
  OR NEW.work_package_id != OLD.work_package_id
  OR NEW.scope_digest != OLD.scope_digest
  OR NEW.artifact_id != OLD.artifact_id
  OR NEW.base_commit != OLD.base_commit
  OR NEW.reviewed_commit_sha != OLD.reviewed_commit_sha
  OR NEW.reviewed_diff_hash != OLD.reviewed_diff_hash
  OR NEW.changed_paths_json != OLD.changed_paths_json
BEGIN
    SELECT RAISE(ABORT, 'materialization receipt identity is immutable');
END;

