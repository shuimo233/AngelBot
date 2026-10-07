-- Preserve one observation record per learned preference. This prevents a
-- resend/edit loop in one conversation from masquerading as independent
-- evidence while keeping genuine later observations available for learning.
CREATE TABLE IF NOT EXISTS adaptive_constraint_evidence (
    id TEXT PRIMARY KEY,
    constraint_key TEXT NOT NULL,
    -- This is provenance, not an ownership relationship. Keep the evidence
    -- after a chat is deleted so deleting a conversation cannot reset a
    -- learned global preference back to one observation.
    session_id TEXT,
    source TEXT NOT NULL,
    observed_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_adaptive_constraint_evidence_recent
    ON adaptive_constraint_evidence(constraint_key, session_id, observed_at DESC);
