-- Migration 035: Session tree with parent_id for non-destructive edit/resend
--
-- Adds three columns that together enable a session tree (DAG):
--   messages.parent_id   – points to the predecessor message within the same session.
--                         A root message has NULL parent_id.
--   messages.is_deleted  – soft-delete flag; deleted messages are hidden from the
--                         provider but remain in the DB so the tree stays traversable.
--   sessions.leaf_message_id – cached pointer to the current leaf (tip) of the
--                         active branch in this session. Updated by application code.
--
-- Backfill strategy (idempotent):
--   Step 1 – add columns + indexes (no-op if already present).
--   Step 2 – backfill parent_id: for each session, walk messages in
--             chronological order and link each to its predecessor using a
--             window function. Rows already having a parent_id are skipped.
--   Step 3 – backfill sessions.leaf_message_id from the max(message.id) per
--             session, so existing sessions are not left with a NULL leaf.

-- ── Step 1: schema ────────────────────────────────────────────────────────────

ALTER TABLE messages ADD COLUMN parent_id TEXT REFERENCES messages(id);
ALTER TABLE messages ADD COLUMN is_deleted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN leaf_message_id TEXT REFERENCES messages(id);

CREATE INDEX IF NOT EXISTS idx_messages_parent
    ON messages(parent_id);
CREATE INDEX IF NOT EXISTS idx_sessions_leaf
    ON sessions(leaf_message_id);

-- ── Step 2: backfill messages.parent_id (idempotent) ──────────────────────────
-- NULL parent_id means "not yet backfilled"; we skip those rows.
-- The LAG window function gives each message its predecessor's id in
-- (session_id, created_at, id) order, which is the correct chronological path.

WITH ordered AS (
    SELECT
        id,
        session_id,
        LAG(id) OVER (
            PARTITION BY session_id
            ORDER BY created_at ASC, id ASC
        ) AS prev_id
    FROM messages
)
UPDATE messages
SET parent_id = (
    SELECT prev_id FROM ordered o WHERE o.id = messages.id
)
WHERE parent_id IS NULL
  AND EXISTS (
      SELECT 1 FROM ordered o
      WHERE o.id = messages.id AND o.prev_id IS NOT NULL
  );

-- ── Step 3: backfill sessions.leaf_message_id (idempotent) ───────────────────

UPDATE sessions
SET leaf_message_id = (
    SELECT m.id
    FROM messages m
    WHERE m.session_id = sessions.id
    ORDER BY m.created_at DESC, m.id DESC
    LIMIT 1
)
WHERE leaf_message_id IS NULL
  AND EXISTS (
      SELECT 1 FROM messages m2 WHERE m2.session_id = sessions.id
  );
