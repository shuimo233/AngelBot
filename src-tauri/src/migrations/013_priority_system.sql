-- Migration 013: Priority system from context-gatekeeper design
-- Adds 5-level priority, constraint validation, and smart-zone tracking.

-- Add priority column to memories
ALTER TABLE memories ADD COLUMN priority TEXT DEFAULT 'fact' CHECK(priority IN ('anchored','constraint','decision','preference','fact'));

-- Track constraint-check results
CREATE TABLE IF NOT EXISTS constraint_checks (
    id TEXT PRIMARY KEY,
    action_description TEXT NOT NULL,
    violated_constraint_ids TEXT, -- JSON array of memory IDs that were violated
    passed INTEGER NOT NULL DEFAULT 1,
    checked_at INTEGER NOT NULL
);

-- Smart zone token tracking
CREATE TABLE IF NOT EXISTS smart_zone_log (
    id TEXT PRIMARY KEY,
    session_id TEXT REFERENCES sessions(id),
    token_count INTEGER NOT NULL,
    smart_zone_pct REAL NOT NULL, -- 0-100, target 60-80%
    recorded_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memories_priority ON memories(priority);
