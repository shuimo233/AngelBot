-- db.rs installs the nullable anchor column with a guarded additive ALTER so
-- journal recovery can replay these indexes/triggers without rewriting data.
CREATE INDEX IF NOT EXISTS idx_context_summaries_anchor
ON context_summaries(session_id, anchor_message_id, compressed_at DESC);

CREATE TRIGGER IF NOT EXISTS context_summary_anchor_insert
BEFORE INSERT ON context_summaries
WHEN NEW.anchor_message_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM messages WHERE id=NEW.anchor_message_id AND session_id=NEW.session_id
)
BEGIN
    SELECT RAISE(ABORT, 'context checkpoint anchor is outside its conversation');
END;

-- Replacing/deleting any covered input invalidates derived context, even if
-- its anchor ID survives an edit. Raw messages and legacy summaries remain.
CREATE TRIGGER IF NOT EXISTS context_checkpoint_source_update
AFTER UPDATE OF content,parent_id,role,metadata,session_id ON messages
WHEN OLD.content IS NOT NEW.content OR OLD.parent_id IS NOT NEW.parent_id
  OR OLD.role IS NOT NEW.role OR OLD.metadata IS NOT NEW.metadata
  OR OLD.session_id IS NOT NEW.session_id
BEGIN
    DELETE FROM context_summaries WHERE anchor_message_id IN (
        WITH RECURSIVE affected(id) AS (
            SELECT NEW.id
            UNION
            SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id
            WHERE m.session_id=OLD.session_id
        ) SELECT id FROM affected
    );
END;

CREATE TRIGGER IF NOT EXISTS context_checkpoint_source_delete
BEFORE DELETE ON messages
BEGIN
    DELETE FROM context_summaries WHERE anchor_message_id IN (
        WITH RECURSIVE affected(id) AS (
            SELECT OLD.id
            UNION
            SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id
            WHERE m.session_id=OLD.session_id
        ) SELECT id FROM affected
    );
END;

CREATE TRIGGER IF NOT EXISTS context_summary_anchor_update
BEFORE UPDATE OF session_id, anchor_message_id ON context_summaries
WHEN NEW.anchor_message_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM messages WHERE id=NEW.anchor_message_id AND session_id=NEW.session_id
)
BEGIN
    SELECT RAISE(ABORT, 'context checkpoint anchor is outside its conversation');
END;
