-- Migration 002: Add frequency decay and embedding fields to memories
-- These columns were previously added via ad-hoc ALTER TABLE with let _ =
-- Guard against re-running: skip if columns already exist
-- SQLite does not support IF NOT EXISTS for ALTER TABLE ADD COLUMN, so we use a workaround

ALTER TABLE memories ADD COLUMN frequency INTEGER DEFAULT 0;
ALTER TABLE memories ADD COLUMN last_mentioned INTEGER;
ALTER TABLE memories ADD COLUMN is_permanent INTEGER DEFAULT 0;
ALTER TABLE memories ADD COLUMN embedding BLOB;
ALTER TABLE memories ADD COLUMN decay_factor REAL DEFAULT 1.0;
ALTER TABLE memories ADD COLUMN forget_stage TEXT DEFAULT 'active';

-- Indexes for new columns
CREATE INDEX IF NOT EXISTS idx_memories_permanent ON memories(is_permanent);
CREATE INDEX IF NOT EXISTS idx_memories_forget_stage ON memories(forget_stage);
