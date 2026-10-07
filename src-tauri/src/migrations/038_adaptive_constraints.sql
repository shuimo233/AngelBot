-- Learned interaction preferences are deliberately separate from the user's
-- visible persona and explicit rules. They can be disabled or expired without
-- rewriting profile data, and retain evidence for later audit/review tooling.
CREATE TABLE IF NOT EXISTS adaptive_constraints (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL CHECK (scope IN ('global', 'session')),
    session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
    constraint_key TEXT NOT NULL,
    constraint_value TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.0 CHECK (confidence >= 0.0 AND confidence <= 1.0),
    evidence_count INTEGER NOT NULL DEFAULT 1 CHECK (evidence_count >= 1),
    source TEXT NOT NULL DEFAULT 'adaptive_learning',
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled', 'expired')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    expires_at INTEGER,
    CHECK ((scope = 'global' AND session_id IS NULL) OR (scope = 'session' AND session_id IS NOT NULL))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_adaptive_constraints_scope_key
    ON adaptive_constraints(scope, COALESCE(session_id, ''), constraint_key);

CREATE INDEX IF NOT EXISTS idx_adaptive_constraints_active
    ON adaptive_constraints(status, scope, session_id, expires_at, updated_at DESC);
