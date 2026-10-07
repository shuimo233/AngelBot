-- Migration 042: Durable, compact foreground attention state.
--
-- The card is the only representation eligible for automatic model-context
-- injection. Diagnostic details are intentionally excluded; `evidence_refs`
-- are opaque ACL-checked pointers owned by their evidence store.

CREATE TABLE IF NOT EXISTS attention_states (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    goal_ref TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'resolved', 'superseded', 'expired')),
    category TEXT NOT NULL,
    impact TEXT NOT NULL CHECK (impact IN ('informational', 'recoverable', 'blocking')),
    next_action TEXT NOT NULL CHECK (next_action IN ('retry', 'resume', 'ask_user', 'inspect')),
    progress_json TEXT NOT NULL,
    recovery_ref TEXT,
    evidence_refs_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_attention_states_session_open
    ON attention_states(session_id, updated_at DESC)
    WHERE status = 'open';

-- Raw diagnostic material is deliberately isolated from attention_cards. No
-- context/query projection may join this table; future evidence access must
-- authorize the session and explicit evidence reference before reading it.
CREATE TABLE IF NOT EXISTS attention_diagnostic_evidence (
    id TEXT PRIMARY KEY,
    attention_id TEXT NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    access_scope TEXT NOT NULL CHECK (access_scope = 'diagnostic_evidence'),
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_attention_diagnostic_evidence_owner
    ON attention_diagnostic_evidence(attention_id, session_id);
