-- Migration 028: lightweight continuation facts for resumable agent slices.
-- This is deliberately not an execution-state machine. It stores only the
-- user-visible facts needed to construct a fresh, bounded follow-up slice.

ALTER TABLE task_runs ADD COLUMN continuation_context TEXT NOT NULL DEFAULT '';
