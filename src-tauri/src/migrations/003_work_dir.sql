-- Migration 003: Add work_dir column to sessions
-- Enables session-level working directory override

ALTER TABLE sessions ADD COLUMN work_dir TEXT;
