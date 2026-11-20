CREATE TABLE IF NOT EXISTS composer_drafts (
    session_id TEXT NOT NULL PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    text TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0)
);
