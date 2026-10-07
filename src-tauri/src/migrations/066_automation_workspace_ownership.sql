-- Agent automations are foreground work for one stable Workspace.  Script
-- automations remain deterministic and may stay workspace-independent.
--
-- Keep the column nullable for legacy script definitions.  Existing Agent
-- definitions safely default to the Personal Workspace created by v62.
UPDATE automations
SET workspace_id = 'personal'
WHERE executor_kind = 'agent' AND workspace_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_automations_workspace_id
    ON automations(workspace_id);
