-- Keep provider call IDs separate from globally unique durable step IDs.
-- The column is added by db::migrate after probing SQLite's table metadata.
UPDATE agent_steps SET call_id = id WHERE call_id IS NULL;
CREATE INDEX IF NOT EXISTS idx_agent_steps_session_call_id
    ON agent_steps(session_id, call_id);
