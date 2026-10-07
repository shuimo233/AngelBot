-- Session-scoped supplemental directories. These directories are read-only;
-- writes remain confined to sessions.work_dir.
ALTER TABLE sessions ADD COLUMN additional_read_dirs TEXT NOT NULL DEFAULT '[]';
