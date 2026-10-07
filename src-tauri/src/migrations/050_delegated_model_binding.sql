-- A delegated attempt must resume with the model identity selected by the
-- Main Agent, never whichever foreground provider happens to be current.
-- These are opaque, non-secret references. API keys remain in the OS keychain.
CREATE TABLE IF NOT EXISTS delegated_model_bindings (
    id TEXT PRIMARY KEY,
    work_package_id TEXT NOT NULL UNIQUE REFERENCES work_packages(id) ON DELETE RESTRICT,
    owner_profile_id INTEGER NOT NULL,
    workspace_key TEXT NOT NULL,
    provider_profile_ref TEXT NOT NULL,
    model_ref TEXT NOT NULL,
    credential_handle_ref TEXT NOT NULL,
    policy_version INTEGER NOT NULL CHECK(policy_version > 0),
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_delegated_model_bindings_scope
    ON delegated_model_bindings(owner_profile_id, workspace_key, created_at DESC);

-- The WorkPackage scope is the binding's authority boundary. SQLite foreign
-- keys cannot express this composite relationship directly, so reject a
-- cross-scope insert at the durable boundary.
CREATE TRIGGER IF NOT EXISTS trg_delegated_model_binding_scope
BEFORE INSERT ON delegated_model_bindings
FOR EACH ROW BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM work_packages
        WHERE id = NEW.work_package_id
          AND owner_profile_id = NEW.owner_profile_id
          AND workspace_key = NEW.workspace_key
    ) THEN RAISE(ABORT, 'delegated model binding scope mismatch') END;
END;

CREATE TRIGGER IF NOT EXISTS trg_delegated_model_binding_immutable
BEFORE UPDATE ON delegated_model_bindings
FOR EACH ROW BEGIN
    SELECT RAISE(ABORT, 'delegated model binding is immutable');
END;
