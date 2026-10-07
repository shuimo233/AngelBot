-- Link durable Agent-automation queue inputs to their visible run state.
--
-- The supervisor input remains the delivery queue, while automation_runs owns
-- the user-facing lifecycle.  A stable foreground run id lets startup recovery
-- distinguish an input that never entered the Main Agent from one whose
-- bounded foreground turn must be continued explicitly.
ALTER TABLE automation_runs ADD COLUMN supervisor_input_id TEXT
    REFERENCES workspace_supervisor_inputs(id) ON DELETE SET NULL;
-- The Main-Agent reply id is reserved before its `task_runs` row exists. Keep
-- this as a durable soft reference so crash recovery can prove that a turn was
-- reserved without ever replaying a potentially side-effecting prompt.
ALTER TABLE automation_runs ADD COLUMN foreground_run_id TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_automation_runs_supervisor_input
    ON automation_runs(supervisor_input_id)
    WHERE supervisor_input_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_automation_runs_foreground_run
    ON automation_runs(foreground_run_id)
    WHERE foreground_run_id IS NOT NULL;
