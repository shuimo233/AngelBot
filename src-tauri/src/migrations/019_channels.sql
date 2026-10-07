CREATE TABLE IF NOT EXISTS channel_connections (
    id TEXT PRIMARY KEY,
    channel_type TEXT NOT NULL,
    display_name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0,
    permission_summary TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS channel_audit_log (
    id TEXT PRIMARY KEY,
    connection_id TEXT NOT NULL REFERENCES channel_connections(id) ON DELETE CASCADE,
    action TEXT NOT NULL,
    details TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
