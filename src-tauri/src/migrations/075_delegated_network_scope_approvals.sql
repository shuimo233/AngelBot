-- A delegated network policy becomes usable by foreground issuance only when
-- it has provenance from one exact, user-confirmed Main-Agent tool call.
-- The policy row remains the transport constraint; this immutable companion
-- record prevents a policy-shaped database write from being mistaken for user
-- consent and detects any later widening or tampering through policy_digest.
CREATE TABLE IF NOT EXISTS delegated_network_scope_approvals (
    id TEXT PRIMARY KEY,
    policy_id TEXT NOT NULL UNIQUE
        REFERENCES delegated_network_source_policies(id) ON DELETE RESTRICT,
    owner_profile_id INTEGER NOT NULL,
    workspace_key TEXT NOT NULL,
    session_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    parent_run_id TEXT NOT NULL,
    tool_call_id TEXT NOT NULL,
    scope_digest TEXT NOT NULL,
    policy_digest TEXT NOT NULL,
    approved_at INTEGER NOT NULL,
    revoked_at INTEGER,
    UNIQUE(session_id, message_id, tool_call_id),
    CHECK(length(id) > 0),
    CHECK(length(policy_id) > 0),
    CHECK(length(workspace_key) > 0),
    CHECK(length(session_id) > 0),
    CHECK(length(message_id) > 0),
    CHECK(length(parent_run_id) > 0),
    CHECK(length(tool_call_id) > 0),
    CHECK(length(scope_digest) > 0),
    CHECK(length(policy_digest) > 0)
);

CREATE INDEX IF NOT EXISTS idx_delegated_network_scope_approvals_lookup
    ON delegated_network_scope_approvals(policy_id, owner_profile_id, workspace_key)
    WHERE revoked_at IS NULL;
