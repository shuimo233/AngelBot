-- Migration 004: FTS5 full-text search for messages and memories
-- Enables keyword search with BM25 scoring and unicode61 tokenizer for Chinese support

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    id UNINDEXED,
    session_id UNINDEXED,
    content,
    tokenize = 'unicode61 remove_diacritics 0'
);

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    id UNINDEXED,
    content,
    tokenize = 'unicode61 remove_diacritics 0'
);

-- Auto-sync triggers for messages
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

-- Auto-sync triggers for memories
CREATE TRIGGER IF NOT EXISTS memories_fts_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(id, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_ad AFTER DELETE ON memories BEGIN
    DELETE FROM memories_fts WHERE id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_au AFTER UPDATE ON memories BEGIN
    DELETE FROM memories_fts WHERE id = old.id;
    INSERT INTO memories_fts(id, content) VALUES (new.id, new.content);
END;
