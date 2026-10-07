-- Migration 030: bind each foreground agent session to its first provider/model.
-- NULL preserves compatibility for sessions created before the first agent run.
ALTER TABLE sessions ADD COLUMN agent_provider TEXT;
ALTER TABLE sessions ADD COLUMN agent_model TEXT;
