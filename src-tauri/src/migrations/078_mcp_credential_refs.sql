-- MCP environment values live in the OS credential vault. SQLite retains only
-- an opaque, versioned reference and non-secret variable names for the UI.
ALTER TABLE mcp_servers ADD COLUMN env_ref TEXT;
ALTER TABLE mcp_servers ADD COLUMN env_keys TEXT NOT NULL DEFAULT '[]';
CREATE UNIQUE INDEX IF NOT EXISTS idx_mcp_servers_env_ref
    ON mcp_servers(env_ref) WHERE env_ref IS NOT NULL;

-- Old vault references are queued for retry if OS deletion fails after a
-- successful reference swap. This table never contains credential values.
CREATE TABLE IF NOT EXISTS mcp_credential_gc (
    env_ref TEXT PRIMARY KEY
);
