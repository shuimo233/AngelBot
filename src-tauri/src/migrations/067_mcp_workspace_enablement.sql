-- MCP servers are installed capabilities, not implicit project permissions.
-- A foreground session exposes a server only when its owning Workspace has a
-- current explicit enablement. Deleting either owner removes the grant.
CREATE TABLE IF NOT EXISTS mcp_workspace_enablements (
    workspace_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    server_id TEXT NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    enabled_at INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, server_id)
);

CREATE INDEX IF NOT EXISTS idx_mcp_workspace_enablements_server
    ON mcp_workspace_enablements(server_id, workspace_id);
