-- Migration 005: Memory dedup with content hashing and conflict resolution
-- Adds content_hash for dedup, superseded_by for version chains, and memory_history audit log

ALTER TABLE memories ADD COLUMN content_hash TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_content_hash ON memories(content_hash);

ALTER TABLE memories ADD COLUMN superseded_by TEXT REFERENCES memories(id);

CREATE TABLE IF NOT EXISTS memory_history (
    id TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL,
    operation TEXT NOT NULL,        -- 'create'|'update'|'delete'|'supersede'|'merge'
    previous_content TEXT,
    new_content TEXT,
    details TEXT,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memory_history_memory ON memory_history(memory_id);
