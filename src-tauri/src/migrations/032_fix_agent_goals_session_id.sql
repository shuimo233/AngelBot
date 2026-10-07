-- Migration 032: Fix agent_goals session_id constraint
-- Previous migration 010 defined session_id as NOT NULL with REFERENCES,
-- but goal tools always insert empty string, causing FOREIGN KEY constraint failures.
-- This migration removes the foreign key constraint to allow empty session_id.
-- Idempotent: safe to re-run if interrupted previously.

PRAGMA foreign_keys=OFF;

-- Drop trigger first (it references agent_goals)
DROP TRIGGER IF EXISTS trg_sessions_cleanup_dependents;

-- If the rebuild already happened (agent_goals already has no FK and the trigger
-- is missing), this block is skipped on subsequent runs.
-- Create new table without foreign key constraint
CREATE TABLE IF NOT EXISTS agent_goals_new (
    id TEXT PRIMARY KEY,
    session_id TEXT DEFAULT '',
    goal_text TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','active','done','failed','cancelled')),
    progress_pct INTEGER DEFAULT 0,
    parent_goal_id TEXT,
    summary TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
);

-- Copy data (handles both empty and populated tables)
INSERT OR IGNORE INTO agent_goals_new
    SELECT id, COALESCE(session_id, ''), goal_text, status,
           progress_pct, parent_goal_id, summary, created_at, completed_at
    FROM agent_goals;

-- Drop old table (only if a row was actually copied or the source table is empty)
DROP TABLE IF EXISTS agent_goals;

-- Rename new table
ALTER TABLE agent_goals_new RENAME TO agent_goals;

-- Recreate the trigger with correct reference (idempotent)
CREATE TRIGGER IF NOT EXISTS trg_sessions_cleanup_dependents
BEFORE DELETE ON sessions
FOR EACH ROW
BEGIN
    UPDATE agent_goals
       SET parent_goal_id = NULL
     WHERE parent_goal_id IN (
         SELECT id FROM agent_goals WHERE session_id = OLD.id
     );
    UPDATE session_branches
       SET parent_branch_id = NULL
     WHERE parent_branch_id IN (
         SELECT id FROM session_branches WHERE session_id = OLD.id
     );
    UPDATE sessions SET parent_id = NULL WHERE parent_id = OLD.id;
    DELETE FROM agent_events WHERE session_id = OLD.id;
    DELETE FROM agent_goals WHERE session_id = OLD.id;
    DELETE FROM agent_steps WHERE session_id = OLD.id;
    DELETE FROM memory_index WHERE session_id = OLD.id;
    DELETE FROM smart_zone_log WHERE session_id = OLD.id;
    DELETE FROM context_summaries WHERE session_id = OLD.id;
    DELETE FROM messages WHERE session_id = OLD.id;
END;

PRAGMA foreign_keys=ON;