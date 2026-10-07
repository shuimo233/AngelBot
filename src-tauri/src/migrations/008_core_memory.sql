-- Migration 008: Core Memory tier
-- Adds core_memory_blocks for persistent AI identity and user context.

CREATE TABLE IF NOT EXISTS core_memory_blocks (
    id TEXT PRIMARY KEY,
    block_type TEXT NOT NULL CHECK(block_type IN ('human', 'persona', 'context', 'rules')),
    label TEXT NOT NULL,
    content TEXT NOT NULL,
    importance INTEGER DEFAULT 5,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_core_memory_type ON core_memory_blocks(block_type);
