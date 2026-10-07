-- An explicit, versioned source policy is required before a delegated lease
-- can use the mediated network gateway.  It is scoped to one profile and one
-- canonical workspace; it is not a global browser preference.
CREATE TABLE IF NOT EXISTS delegated_network_source_policies (
    id TEXT PRIMARY KEY,
    owner_profile_id INTEGER NOT NULL,
    workspace_key TEXT NOT NULL,
    capability_scope_ref TEXT NOT NULL,
    policy_version INTEGER NOT NULL CHECK(policy_version > 0),
    actions_json TEXT NOT NULL,
    allowed_hosts_json TEXT NOT NULL,
    allowed_mime_types_json TEXT NOT NULL,
    max_response_bytes INTEGER NOT NULL CHECK(max_response_bytes > 0),
    max_redirects INTEGER NOT NULL CHECK(max_redirects >= 0 AND max_redirects <= 8),
    enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(owner_profile_id, workspace_key, capability_scope_ref, policy_version)
);
CREATE INDEX IF NOT EXISTS idx_delegated_network_source_policy_scope
    ON delegated_network_source_policies(owner_profile_id, workspace_key, capability_scope_ref, enabled, policy_version DESC);
