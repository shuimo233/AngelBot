-- Derived discovery only: never credentials, execution receipts or authority.
-- Existing installations begin without a catalog and connect once explicitly.
CREATE TABLE IF NOT EXISTS mcp_tool_catalog (
    server_id TEXT PRIMARY KEY REFERENCES mcp_servers(id) ON DELETE CASCADE,
    config_fingerprint TEXT NOT NULL CHECK(length(config_fingerprint) = 64),
    tools_json TEXT NOT NULL CHECK(json_valid(tools_json) AND json_type(tools_json) = 'array'
                                  AND length(tools_json) <= 4194304),
    updated_at INTEGER NOT NULL
);

CREATE TRIGGER IF NOT EXISTS invalidate_mcp_tool_catalog_on_config_update
AFTER UPDATE OF command, args, enabled ON mcp_servers
WHEN NEW.command IS NOT OLD.command OR NEW.args IS NOT OLD.args OR NEW.enabled = 0
BEGIN
    DELETE FROM mcp_tool_catalog WHERE server_id = NEW.id;
END;
