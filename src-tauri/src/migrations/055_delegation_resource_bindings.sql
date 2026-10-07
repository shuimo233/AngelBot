-- Durable identity ledger for temporary resources owned by one delegated attempt.
--
-- Provider paths and branch names are deliberately absent.  `resource_ref` and
-- `manifest_locator` are opaque provider identities and may only be resolved by
-- the provider that created them.  The row records authority and cleanup
-- progress; it is not a second provider lifecycle state machine.
CREATE TABLE IF NOT EXISTS delegation_resource_bindings (
    id TEXT PRIMARY KEY,
    delegation_id TEXT NOT NULL REFERENCES delegations(id) ON DELETE CASCADE,
    work_package_id TEXT NOT NULL REFERENCES work_packages(id) ON DELETE RESTRICT,
    attempt_id TEXT NOT NULL REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    lease_id TEXT NOT NULL REFERENCES delegation_capability_leases(id) ON DELETE RESTRICT,
    resource_kind TEXT NOT NULL CHECK(resource_kind IN ('sandbox', 'worktree')),
    provider_kind TEXT NOT NULL CHECK(length(provider_kind) > 0),
    resource_ref TEXT NOT NULL CHECK(length(resource_ref) > 0),
    manifest_locator TEXT NOT NULL CHECK(length(manifest_locator) > 0),
    manifest_version INTEGER NOT NULL CHECK(manifest_version > 0),
    manifest_nonce TEXT NOT NULL CHECK(length(manifest_nonce) > 0),
    manifest_digest TEXT NOT NULL CHECK(length(manifest_digest) > 0),
    scope_digest TEXT NOT NULL CHECK(length(scope_digest) > 0),
    lease_epoch INTEGER NOT NULL CHECK(lease_epoch > 0),
    -- Captured only after the scheduler obtains a workspace admission.  An
    -- issuance row can therefore be persisted before admission is acquired.
    admission_id TEXT REFERENCES workspace_admissions(id) ON DELETE RESTRICT,
    admission_state_version INTEGER CHECK(admission_state_version IS NULL OR admission_state_version >= 0),
    lifecycle_state TEXT NOT NULL DEFAULT 'preparing'
        CHECK(lifecycle_state IN ('preparing', 'ready', 'sealed', 'revoked', 'quarantined', 'unknown')),
    cleanup_status TEXT NOT NULL DEFAULT 'none'
        CHECK(cleanup_status IN ('none', 'pending', 'retry', 'cleaned', 'blocked', 'retained')),
    cleanup_step TEXT,
    cleanup_attempts INTEGER NOT NULL DEFAULT 0 CHECK(cleanup_attempts >= 0),
    next_cleanup_at INTEGER,
    quarantine_until INTEGER,
    last_error_class TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    sealed_at INTEGER,
    revoked_at INTEGER,
    cleaned_at INTEGER,
    UNIQUE(attempt_id, resource_kind),
    UNIQUE(provider_kind, resource_ref),
    UNIQUE(manifest_nonce),
    CHECK((admission_id IS NULL) = (admission_state_version IS NULL))
);

CREATE INDEX IF NOT EXISTS idx_delegation_resource_bindings_attempt
    ON delegation_resource_bindings(attempt_id, resource_kind);
CREATE INDEX IF NOT EXISTS idx_delegation_resource_bindings_cleanup
    ON delegation_resource_bindings(cleanup_status, quarantine_until, updated_at);
CREATE INDEX IF NOT EXISTS idx_delegation_resource_bindings_retention
    ON delegation_resource_bindings(quarantine_until)
    WHERE quarantine_until IS NOT NULL;

-- SQLite cannot express the package/delegation/attempt/lease composite
-- relationship through foreign keys alone.  Reject a cross-scope binding at
-- the durable boundary before any provider is allowed to act on it.
CREATE TRIGGER IF NOT EXISTS trg_delegation_resource_binding_scope
BEFORE INSERT ON delegation_resource_bindings
FOR EACH ROW BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM delegation_attempts a
        JOIN delegations d ON d.id = a.delegation_id
        JOIN work_packages p ON p.id = d.work_package_id
        JOIN delegation_capability_leases l ON l.attempt_id = a.id
        WHERE a.id = NEW.attempt_id
          AND a.delegation_id = NEW.delegation_id
          AND d.work_package_id = NEW.work_package_id
          AND l.id = NEW.lease_id
          AND p.scope_digest = NEW.scope_digest
    ) THEN RAISE(ABORT, 'delegation resource binding scope mismatch') END;
    SELECT CASE WHEN NEW.admission_id IS NOT NULL AND NOT EXISTS (
        SELECT 1
        FROM workspace_admissions wa
        WHERE wa.id = NEW.admission_id
          AND wa.attempt_id = NEW.attempt_id
          AND wa.lease_id = NEW.lease_id
          AND wa.status = 'active'
          AND wa.state_version = NEW.admission_state_version
    ) THEN RAISE(ABORT, 'delegation resource admission binding mismatch') END;
END;

-- Identity facts are write-once.  Cleanup progress may advance, but a retry
-- can never retarget an opaque provider resource or move it to another scope.
CREATE TRIGGER IF NOT EXISTS trg_delegation_resource_binding_immutable
BEFORE UPDATE OF delegation_id, work_package_id, attempt_id, lease_id,
    resource_kind, provider_kind, resource_ref, manifest_locator,
    manifest_version, manifest_nonce, manifest_digest, scope_digest, lease_epoch
    ON delegation_resource_bindings
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'delegation resource binding identity is immutable');
END;

-- Admission capture is itself a CAS token.  It can be set once and may not be
-- changed after the scheduler has handed the resource to a worker.
CREATE TRIGGER IF NOT EXISTS trg_delegation_resource_binding_admission_immutable
BEFORE UPDATE OF admission_id, admission_state_version ON delegation_resource_bindings
FOR EACH ROW WHEN OLD.admission_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'delegation resource admission binding is immutable');
END;
