-- Execution permissions are explicit grants, not merely UI preferences.  A
-- NULL expiry represents an intentionally permanent user decision; a finite
-- grant must fail closed to `ask` once its lease expires.
CREATE TABLE IF NOT EXISTS agent_permission_grants (
    scope TEXT PRIMARY KEY CHECK (scope = 'global'),
    permission TEXT NOT NULL CHECK (permission IN ('ask', 'workspace_auto', 'full_access')),
    expires_at INTEGER,
    updated_at INTEGER NOT NULL
);

-- Preserve an existing choice when upgrading.  Older installations used the
-- settings key and had no expiry control, so it is migrated as permanent.
INSERT OR IGNORE INTO agent_permission_grants (scope, permission, expires_at, updated_at)
SELECT 'global',
       CASE value
           WHEN 'workspace_auto' THEN 'workspace_auto'
           WHEN 'full_access' THEN 'full_access'
           ELSE 'ask'
       END,
       NULL,
       CAST(strftime('%s', 'now') AS INTEGER)
FROM settings
WHERE key = 'agent_execution_permission';
