-- Checkpoints are derived data. Invalidate only checkpoints whose selected
-- ancestry actually covers the changed owner; new/current/off-branch work
-- must not erase an older checkpoint on every turn. No originals are deleted.
CREATE TRIGGER IF NOT EXISTS checkpoint_message_time_update AFTER UPDATE OF created_at,tool_calls,is_provisional ON messages
WHEN OLD.created_at IS NOT NEW.created_at OR OLD.tool_calls IS NOT NEW.tool_calls OR OLD.is_provisional IS NOT NEW.is_provisional
BEGIN
 DELETE FROM context_summaries WHERE session_id=OLD.session_id AND anchor_message_id IN (
  WITH RECURSIVE affected(id) AS (SELECT OLD.id UNION SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id WHERE m.session_id=OLD.session_id)
  SELECT id FROM affected);
END;
CREATE TRIGGER IF NOT EXISTS checkpoint_task_insert AFTER INSERT ON task_runs
BEGIN
 DELETE FROM context_summaries WHERE session_id=NEW.session_id AND anchor_message_id IN (
  WITH RECURSIVE affected(id) AS (SELECT NEW.message_id UNION SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id WHERE m.session_id=NEW.session_id)
  SELECT id FROM affected);
END;
CREATE TRIGGER IF NOT EXISTS checkpoint_task_update AFTER UPDATE ON task_runs
WHEN OLD.id IS NOT NEW.id OR OLD.status IS NOT NEW.status OR OLD.confirmation_state IS NOT NEW.confirmation_state
 OR OLD.resumable IS NOT NEW.resumable OR OLD.goal IS NOT NEW.goal OR OLD.plan IS NOT NEW.plan
 OR OLD.step_count IS NOT NEW.step_count OR OLD.completed_step_count IS NOT NEW.completed_step_count
 OR OLD.message_id IS NOT NEW.message_id OR OLD.session_id IS NOT NEW.session_id
BEGIN
 DELETE FROM context_summaries WHERE session_id IN (OLD.session_id,NEW.session_id) AND anchor_message_id IN (
  WITH RECURSIVE affected(id) AS (SELECT OLD.message_id UNION SELECT NEW.message_id UNION SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id WHERE m.session_id IN (OLD.session_id,NEW.session_id))
  SELECT id FROM affected);
END;
CREATE TRIGGER IF NOT EXISTS checkpoint_task_delete AFTER DELETE ON task_runs
BEGIN
 DELETE FROM context_summaries WHERE session_id=OLD.session_id AND anchor_message_id IN (
  WITH RECURSIVE affected(id) AS (SELECT OLD.message_id UNION SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id WHERE m.session_id=OLD.session_id)
  SELECT id FROM affected);
END;
CREATE TRIGGER IF NOT EXISTS checkpoint_facts_insert AFTER INSERT ON task_run_facts
BEGIN
 DELETE FROM context_summaries WHERE session_id=NEW.session_id AND anchor_message_id IN (
  WITH RECURSIVE affected(id) AS (SELECT NEW.message_id UNION SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id WHERE m.session_id=NEW.session_id)
  SELECT id FROM affected);
END;
CREATE TRIGGER IF NOT EXISTS checkpoint_facts_update AFTER UPDATE ON task_run_facts
WHEN OLD.task_run_id IS NOT NEW.task_run_id OR OLD.facts_json IS NOT NEW.facts_json OR OLD.updated_at IS NOT NEW.updated_at
 OR OLD.message_id IS NOT NEW.message_id OR OLD.session_id IS NOT NEW.session_id
BEGIN
 DELETE FROM context_summaries WHERE session_id IN (OLD.session_id,NEW.session_id) AND anchor_message_id IN (
  WITH RECURSIVE affected(id) AS (SELECT OLD.message_id UNION SELECT NEW.message_id UNION SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id WHERE m.session_id IN (OLD.session_id,NEW.session_id))
  SELECT id FROM affected);
END;
CREATE TRIGGER IF NOT EXISTS checkpoint_facts_delete AFTER DELETE ON task_run_facts
BEGIN
 DELETE FROM context_summaries WHERE session_id=OLD.session_id AND anchor_message_id IN (
  WITH RECURSIVE affected(id) AS (SELECT OLD.message_id UNION SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id WHERE m.session_id=OLD.session_id)
  SELECT id FROM affected);
END;
CREATE TRIGGER IF NOT EXISTS checkpoint_receipt_insert AFTER INSERT ON agent_run_events
WHEN NEW.event_type IN ('ToolExecutionEnd','ToolExecutionStart','ExternalToolDispatchReserved','AgentEnd','Error')
BEGIN
 DELETE FROM context_summaries WHERE session_id=NEW.session_id AND anchor_message_id IN (
  WITH RECURSIVE affected(id) AS (SELECT NEW.run_id UNION SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id WHERE m.session_id=NEW.session_id)
  SELECT id FROM affected);
END;
CREATE TRIGGER IF NOT EXISTS checkpoint_receipt_update AFTER UPDATE ON agent_run_events
WHEN (OLD.event_type IN ('ToolExecutionEnd','ToolExecutionStart','ExternalToolDispatchReserved','AgentEnd','Error') OR NEW.event_type IN ('ToolExecutionEnd','ToolExecutionStart','ExternalToolDispatchReserved','AgentEnd','Error')) AND (OLD.id IS NOT NEW.id OR OLD.payload IS NOT NEW.payload OR OLD.run_id IS NOT NEW.run_id
 OR OLD.session_id IS NOT NEW.session_id OR OLD.sequence IS NOT NEW.sequence OR OLD.created_at IS NOT NEW.created_at OR OLD.event_type IS NOT NEW.event_type)
BEGIN
 DELETE FROM context_summaries WHERE session_id IN (OLD.session_id,NEW.session_id) AND anchor_message_id IN (
  WITH RECURSIVE affected(id) AS (SELECT OLD.run_id UNION SELECT NEW.run_id UNION SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id WHERE m.session_id IN (OLD.session_id,NEW.session_id))
  SELECT id FROM affected);
END;
CREATE TRIGGER IF NOT EXISTS checkpoint_receipt_delete AFTER DELETE ON agent_run_events
WHEN OLD.event_type IN ('ToolExecutionEnd','ToolExecutionStart','ExternalToolDispatchReserved','AgentEnd','Error')
BEGIN
 DELETE FROM context_summaries WHERE session_id=OLD.session_id AND anchor_message_id IN (
  WITH RECURSIVE affected(id) AS (SELECT OLD.run_id UNION SELECT m.id FROM messages m JOIN affected a ON m.parent_id=a.id WHERE m.session_id=OLD.session_id)
  SELECT id FROM affected);
END;
