-- Migration 036: Hard-delete branch on edit/resend
--
-- Design decision: when a user edits and resends a message, the entire subtree
-- rooted at that message is permanently removed (true DELETE). No soft-delete.
--
-- Rationale:
--   - SQLite FK constraints check existence, not application state. A soft-deleted
--     message still exists in the DB, making its parent_id a dangling semantic
--     pointer that confuses the conversation tree.
--   - Users who choose "edit and resend" are explicitly branching — they want
--     a clean slate from that point, not a reversible deletion.
--   - The conversation history remains traversable via the parent_id chain
--     leading to the session root; the deleted subtree is simply gone.
--
-- Changes:
--   1. Recreate `messages` with `parent_id REFERENCES messages(id) ON DELETE CASCADE`
--      so that hard-deleting a message automatically removes its subtree.
--   2. Drop the `is_deleted` column (unused after this migration).
--
-- Trigger notes:
--   - FTS triggers (`messages_fts_*`) are dropped along with the old `messages`
--     table; we recreate them after the swap.
--   - `trg_sessions_cleanup_dependents` references the messages table by name;
--     we DROP and re-CREATE it around the swap.
--
-- Column compatibility: `tool_results` is added by Rust code after migrations run,
-- so we do not reference it in INSERT (Rust will populate after migration).
-- FTS triggers (messages_fts_ad) handle cascade cleanup automatically.
-- NOTE: This migration runs inside a transaction managed by the migrate() framework.
-- Do NOT use BEGIN/COMMIT here.

-- ── Step 1: create new messages table with ON DELETE CASCADE ──────────────────────
CREATE TABLE messages_new (
    id TEXT NOT NULL PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    metadata TEXT,
    created_at INTEGER NOT NULL,
    parent_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
    tool_calls TEXT,
    tool_call_id TEXT,
    tool_name TEXT,
    tool_results TEXT
);

-- ── Step 2: drop triggers that reference the messages table ────────────────────
DROP TRIGGER IF EXISTS trg_sessions_cleanup_dependents;
DROP TRIGGER IF EXISTS messages_fts_ai;
DROP TRIGGER IF EXISTS messages_fts_ad;
DROP TRIGGER IF EXISTS messages_fts_au;

-- ── Step 3: copy data from old table ────────────────────────────────────────────
-- Only reference columns guaranteed to exist in the migration chain (001-035).
-- `tool_results` is added by Rust code after migration completes (NULL here is fine).
-- `is_deleted` is silently dropped (the column will not exist in messages_new).
INSERT INTO messages_new (id, session_id, role, content, metadata, created_at,
    parent_id, tool_calls, tool_call_id, tool_name)
SELECT
    CAST(id AS TEXT),
    CAST(session_id AS TEXT),
    CAST(role AS TEXT),
    CAST(content AS TEXT),
    CAST(metadata AS TEXT),
    CAST(created_at AS INTEGER),
    CAST(parent_id AS TEXT),
    CAST(tool_calls AS TEXT),
    CAST(tool_call_id AS TEXT),
    CAST(tool_name AS TEXT)
FROM messages;

-- ── Step 4: swap old → new table ────────────────────────────────────────────────
DROP TABLE messages;
ALTER TABLE messages_new RENAME TO messages;

-- Restore indexes
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
CREATE INDEX IF NOT EXISTS idx_messages_parent ON messages(parent_id);
CREATE INDEX IF NOT EXISTS idx_messages_created ON messages(created_at);

-- ── Step 5: rebuild FTS index for swapped table ─────────────────────────────────
-- The FTS table itself survives (it's a separate virtual table), but the
-- triggers that kept it in sync are gone, and existing rows must be reindexed.
INSERT INTO messages_fts(messages_fts) VALUES ('rebuild');

-- Recreate FTS triggers (mirror migration 004)
CREATE TRIGGER IF NOT EXISTS messages_fts_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(id, session_id, content) VALUES (new.id, new.session_id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_ad AFTER DELETE ON messages BEGIN
    DELETE FROM messages_fts WHERE id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_au AFTER UPDATE ON messages BEGIN
    DELETE FROM messages_fts WHERE id = old.id;
    INSERT INTO messages_fts(id, session_id, content) VALUES (new.id, new.session_id, new.content);
END;

-- ── Step 6: recreate the sessions cleanup trigger ──────────────────────────────
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

-- ── Step 7: clean up old indexes ────────────────────────────────────────────────
DROP INDEX IF EXISTS idx_messages_deleted;
DROP INDEX IF EXISTS idx_messages_is_deleted;