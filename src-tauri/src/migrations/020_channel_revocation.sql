ALTER TABLE channel_connections ADD COLUMN removed_at INTEGER;
CREATE INDEX IF NOT EXISTS idx_channel_connections_active ON channel_connections(removed_at, enabled);
