-- Migration 009: Embedding cache for API call reduction
-- Stores content_hash → embedding mappings with TTL support.

CREATE TABLE IF NOT EXISTS embedding_cache (
    content_hash TEXT PRIMARY KEY,
    embedding BLOB NOT NULL,
    dimension INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    last_accessed_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_embedding_cache_accessed ON embedding_cache(last_accessed_at);
