-- A foreground reply may need a durable parent identity before its first
-- internal tool call. Such rows are never part of the visible transcript
-- until the foreground terminal barrier finalizes them.
ALTER TABLE messages ADD COLUMN is_provisional INTEGER NOT NULL DEFAULT 0
    CHECK(is_provisional IN (0, 1));
CREATE INDEX IF NOT EXISTS idx_messages_visible_session_created
    ON messages(session_id, is_provisional, created_at);
