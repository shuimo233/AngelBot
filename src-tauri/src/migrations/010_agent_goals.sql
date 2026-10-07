-- Migration 010: Agent autonomous goal tracking
-- Enables Plan-Execute-Reflect cycle with persistent goal tracking.

CREATE TABLE IF NOT EXISTS agent_goals (
    id TEXT PRIMARY KEY,
    session_id TEXT DEFAULT '' REFERENCES sessions(id) ON DELETE SET DEFAULT,
    goal_text TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','active','done','failed','cancelled')),
    progress_pct INTEGER DEFAULT 0,
    parent_goal_id TEXT REFERENCES agent_goals(id),
    summary TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_agent_goals_session ON agent_goals(session_id);
CREATE INDEX IF NOT EXISTS idx_agent_goals_status ON agent_goals(status);

-- tool execution patterns for self-improvement (Phase 5)
CREATE TABLE IF NOT EXISTS tool_patterns (
    id TEXT PRIMARY KEY,
    tool_name TEXT NOT NULL,
    task_description TEXT,
    arguments_pattern TEXT,
    success INTEGER NOT NULL DEFAULT 1,
    latency_ms INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tool_patterns_name ON tool_patterns(tool_name);
