-- Migration: Add usage tracking tables
-- Issue #43: Provider cost tracking

-- Usage stats table for tracking token consumption
CREATE TABLE IF NOT EXISTS usage_stats (
    id TEXT PRIMARY KEY,
    session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0,
    latency_ms INTEGER,
    created_at INTEGER NOT NULL
);

-- Index for efficient usage queries
CREATE INDEX IF NOT EXISTS idx_usage_session ON usage_stats(session_id);
CREATE INDEX IF NOT EXISTS idx_usage_provider ON usage_stats(provider);
CREATE INDEX IF NOT EXISTS idx_usage_created ON usage_stats(created_at);

-- Cost summary view (materialized)
CREATE TABLE IF NOT EXISTS cost_summary_cache (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    total_prompt_tokens INTEGER NOT NULL DEFAULT 0,
    total_completion_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost_usd REAL NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);

-- Insert initial cache row
INSERT OR IGNORE INTO cost_summary_cache (id, total_prompt_tokens, total_completion_tokens, total_cost_usd, updated_at)
VALUES (1, 0, 0, 0.0, 0);
