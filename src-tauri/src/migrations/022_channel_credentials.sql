-- Secrets never enter SQLite. This flag only reflects a successful OS-keychain write.
ALTER TABLE channel_connections ADD COLUMN credential_configured INTEGER NOT NULL DEFAULT 0;
