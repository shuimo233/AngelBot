-- Migration: Add reviewable self-evolution proposals
-- Issue #56: Context-driven evolution must be user-reviewable before it writes long-term memory.

CREATE TABLE IF NOT EXISTS evolution_proposals (
    id TEXT PRIMARY KEY,
    proposal_type TEXT NOT NULL, -- 'memory' | 'profile_preferences'
    session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    category TEXT,
    content TEXT,
    importance INTEGER,
    preferences_json TEXT,
    summary TEXT,
    source TEXT NOT NULL DEFAULT 'evolution_review',
    status TEXT NOT NULL DEFAULT 'pending', -- pending | accepted | rejected
    created_at INTEGER NOT NULL,
    reviewed_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_evolution_proposals_status ON evolution_proposals(status);
CREATE INDEX IF NOT EXISTS idx_evolution_proposals_session ON evolution_proposals(session_id);
