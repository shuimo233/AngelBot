-- Migration 026: Make session deletion complete and atomic at the schema boundary.
--
-- Several early tables reference sessions with the historical NO ACTION
-- default. Later features added more session-owned records, so deleting only
-- messages and context summaries can fail after already removing user data.
-- Keep the existing published table definitions intact and centralize the
-- ownership policy in a trigger that applies to every DELETE entry point.

CREATE TRIGGER IF NOT EXISTS trg_sessions_cleanup_dependents
BEFORE DELETE ON sessions
FOR EACH ROW
BEGIN
    -- Break cross-session/self-references before deleting owned rows.
    UPDATE agent_goals
       SET parent_goal_id = NULL
     WHERE parent_goal_id IN (
         SELECT id FROM agent_goals WHERE session_id = OLD.id
     );
    UPDATE session_branches
       SET parent_branch_id = NULL
     WHERE parent_branch_id IN (
         SELECT id FROM session_branches WHERE session_id = OLD.id
     );
    UPDATE sessions SET parent_id = NULL WHERE parent_id = OLD.id;

    -- Historical NO ACTION relationships.
    DELETE FROM agent_events WHERE session_id = OLD.id;
    DELETE FROM agent_goals WHERE session_id = OLD.id;
    DELETE FROM agent_steps WHERE session_id = OLD.id;
    DELETE FROM memory_index WHERE session_id = OLD.id;
    DELETE FROM smart_zone_log WHERE session_id = OLD.id;
    DELETE FROM context_summaries WHERE session_id = OLD.id;
    DELETE FROM messages WHERE session_id = OLD.id;

    -- Tables with CASCADE/SET NULL policies are intentionally left to their
    -- declared foreign-key actions (task_runs, session_branches, usage_stats,
    -- evolution_proposals).
END;
