-- Migration 034: Inline tool protocol into messages (data backfill)
-- Stores assistant tool_calls and tool results as first-class fields on the
-- messages table. The provider protocol can now be reconstructed with a single
-- query, instead of relying on a fragile `agent_steps.created_at =
-- messages.created_at` join. This makes replay/resend fully Pi-style: the
-- model sees exactly which tools it called last time and their outputs.
--
-- Schema: the `tool_calls`, `tool_call_id`, `tool_name` columns and the
-- `idx_messages_tool_call_id` index are added by `db.rs` directly using the
-- `MIGRATION_034_SCHEMA` constant, not from this file. Splitting them keeps
-- the schema portion idempotent across test fixtures that simulate
-- `settings.schema_version` drift — only the data backfill needs to be
-- replayed when the columns are already present.
--
-- Both steps below are idempotent: they only write rows whose `tool_calls`
-- (for assistant) or `tool_call_id` (for tool) is currently NULL, so re-
-- running after a partial migration is safe.

-- Step 1: synthesize tool messages for every agent_step, skipping synthetic
-- audit steps (context_compaction, agent_loop) that were never provider-issued.
INSERT INTO messages (id, session_id, role, content, tool_call_id, tool_name, created_at)
SELECT
    'bf-tool-' || s.id,
    s.session_id,
    'tool',
    CASE WHEN s.success = 1 THEN s.tool_output ELSE COALESCE(s.tool_output, '') END,
    s.id,
    s.tool_name,
    s.created_at
FROM agent_steps s
WHERE s.tool_name NOT IN ('context_compaction', 'agent_loop')
  AND NOT EXISTS (
      SELECT 1 FROM messages m
      WHERE m.tool_call_id = s.id
  );

-- Step 2: for every assistant message that still has no tool_calls stored,
-- aggregate its matching agent_steps into a JSON array. We use the same
-- created_at=match that already exists in code today; if no matching steps
-- exist (e.g. tool-less turn, or post-edit-resend where the new assistant
-- message has no agent_steps), tool_calls stays NULL.
UPDATE messages
SET tool_calls = (
    SELECT '[' || GROUP_CONCAT(
        json_object(
            'id', s.id,
            'name', s.tool_name,
            'arguments', json(COALESCE(s.tool_input, '{}'))
        ),
        ','
    ) || ']'
    FROM agent_steps s
    WHERE s.session_id = messages.session_id
      AND s.created_at = messages.created_at
      AND s.tool_name NOT IN ('context_compaction', 'agent_loop')
    ORDER BY s.seq ASC
)
WHERE role = 'assistant'
  AND tool_calls IS NULL
  AND EXISTS (
      SELECT 1 FROM agent_steps s2
      WHERE s2.session_id = messages.session_id
        AND s2.created_at = messages.created_at
        AND s2.tool_name NOT IN ('context_compaction', 'agent_loop')
  );