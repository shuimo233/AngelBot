-- A user-comprehensible work package is the only confirmation boundary for
-- internally delegated candidate work.  Workers do not receive privileges
-- from these records; later adapters consume this contract.
CREATE TABLE IF NOT EXISTS work_packages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    owner_profile_id INTEGER NOT NULL,
    workspace_key TEXT NOT NULL,
    task_shape TEXT NOT NULL CHECK(task_shape IN ('explore', 'change')),
    scope_digest TEXT NOT NULL,
    capability_scope_ref TEXT NOT NULL,
    capability_expires_at INTEGER NOT NULL,
    candidate_version INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(id, session_id, owner_profile_id, workspace_key)
);
CREATE INDEX IF NOT EXISTS idx_work_packages_scope ON work_packages(session_id, owner_profile_id, workspace_key, updated_at DESC);

CREATE TABLE IF NOT EXISTS work_package_candidate_sets (
    id TEXT PRIMARY KEY,
    work_package_id TEXT NOT NULL REFERENCES work_packages(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    manifest_json TEXT NOT NULL,
    candidate_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(work_package_id, version)
);
CREATE INDEX IF NOT EXISTS idx_candidate_sets_package_version ON work_package_candidate_sets(work_package_id, version DESC);

CREATE TABLE IF NOT EXISTS work_package_change_sets (
    id TEXT PRIMARY KEY,
    work_package_id TEXT NOT NULL REFERENCES work_packages(id) ON DELETE CASCADE,
    candidate_set_id TEXT NOT NULL REFERENCES work_package_candidate_sets(id) ON DELETE RESTRICT,
    scope_digest TEXT NOT NULL,
    candidate_hash TEXT NOT NULL,
    base_revision TEXT NOT NULL,
    summary_refs_json TEXT NOT NULL,
    change_set_hash TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_change_sets_package ON work_package_change_sets(work_package_id, created_at DESC);

CREATE TABLE IF NOT EXISTS work_package_confirmations (
    id TEXT PRIMARY KEY,
    work_package_id TEXT NOT NULL REFERENCES work_packages(id) ON DELETE CASCADE,
    change_set_id TEXT NOT NULL REFERENCES work_package_change_sets(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN ('approved', 'declined', 'expired', 'invalidated')),
    scope_digest TEXT NOT NULL,
    candidate_hash TEXT NOT NULL,
    base_revision TEXT NOT NULL,
    change_set_hash TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_confirmations_change_set ON work_package_confirmations(change_set_id, status, expires_at);
