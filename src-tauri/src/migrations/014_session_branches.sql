-- Migration: Add session branching support
-- Issue #41: Session branches with tree structure

-- Add parent_id to sessions for tree structure
ALTER TABLE sessions ADD COLUMN parent_id TEXT REFERENCES sessions(id);

-- Add branch metadata
ALTER TABLE sessions ADD COLUMN branch_name TEXT;
ALTER TABLE sessions ADD COLUMN branch_created_at INTEGER;

-- Create branches table for tracking all branches
CREATE TABLE IF NOT EXISTS session_branches (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    parent_branch_id TEXT REFERENCES session_branches(id),
    name TEXT NOT NULL,
    description TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    is_active INTEGER DEFAULT 0,
    message_count INTEGER DEFAULT 0
);

-- Create index for efficient branch queries
CREATE INDEX IF NOT EXISTS idx_branches_session ON session_branches(session_id);
CREATE INDEX IF NOT EXISTS idx_branches_parent ON session_branches(parent_branch_id);
CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_id);
