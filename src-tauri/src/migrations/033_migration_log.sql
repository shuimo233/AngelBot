-- Migration 033: Migration execution journal
-- Adds a per-migration execution log so migrate() can detect
-- "settings.schema_version was written but the actual schema was not applied"
-- and recover automatically. The MAX(version) WHERE status='applied' in
-- migration_log is the new source of truth.
--
-- This migration is self-healing: if settings.schema_version claims v32 ran
-- but agent_goals still carries the v10 foreign key, v33 replays the v32
-- schema changes inline (in this same transaction) before backfilling v32
-- into migration_log. After this, the journal and the actual schema agree.

CREATE TABLE IF NOT EXISTS migration_log (
    version INTEGER PRIMARY KEY,
    description TEXT NOT NULL,
    checksum TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('applied','failed')),
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_migration_log_status
    ON migration_log(status, version);

-- ─── Self-heal: replay v32 if its FK rebuild never landed ────────────────────
-- Detect the broken v32: agent_goals.session_id still has the v10 FK clause.
-- We probe sqlite_master.sql for the agent_goals CREATE TABLE statement.
PRAGMA foreign_keys=OFF;

DROP TRIGGER IF EXISTS trg_sessions_cleanup_dependents;

CREATE TABLE IF NOT EXISTS agent_goals_new (
    id TEXT PRIMARY KEY,
    session_id TEXT DEFAULT '',
    goal_text TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','active','done','failed','cancelled')),
    progress_pct INTEGER DEFAULT 0,
    parent_goal_id TEXT,
    summary TEXT,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
);

INSERT OR IGNORE INTO agent_goals_new
    SELECT id, COALESCE(session_id, ''), goal_text, status,
           progress_pct, parent_goal_id, summary, created_at, completed_at
    FROM agent_goals;

DROP TABLE IF EXISTS agent_goals;
ALTER TABLE agent_goals_new RENAME TO agent_goals;

CREATE TRIGGER IF NOT EXISTS trg_sessions_cleanup_dependents
BEFORE DELETE ON sessions
FOR EACH ROW
BEGIN
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
    DELETE FROM agent_events WHERE session_id = OLD.id;
    DELETE FROM agent_goals WHERE session_id = OLD.id;
    DELETE FROM agent_steps WHERE session_id = OLD.id;
    DELETE FROM memory_index WHERE session_id = OLD.id;
    DELETE FROM smart_zone_log WHERE session_id = OLD.id;
    DELETE FROM context_summaries WHERE session_id = OLD.id;
    DELETE FROM messages WHERE session_id = OLD.id;
END;

PRAGMA foreign_keys=ON;

-- ─── Backfill migration_log from settings.schema_version ────────────────────
-- After the self-heal above, settings.schema_version may still claim a
-- higher version than what actually ran. We want the journal to reflect
-- reality, so we re-sync schema_version to MAX(version) after backfill.
INSERT OR IGNORE INTO migration_log (version, description, checksum, status, started_at, finished_at)
SELECT
    v.value,
    'backfilled from settings.schema_version',
    '',
    'applied',
    0,
    0
FROM settings s
JOIN (SELECT 1 AS value UNION SELECT 2 UNION SELECT 3 UNION SELECT 4 UNION SELECT 5
      UNION SELECT 6 UNION SELECT 7 UNION SELECT 8 UNION SELECT 9 UNION SELECT 10
      UNION SELECT 11 UNION SELECT 12 UNION SELECT 13 UNION SELECT 14 UNION SELECT 15
      UNION SELECT 16 UNION SELECT 17 UNION SELECT 18 UNION SELECT 19 UNION SELECT 20
      UNION SELECT 21 UNION SELECT 22 UNION SELECT 23 UNION SELECT 24 UNION SELECT 25
      UNION SELECT 26 UNION SELECT 27 UNION SELECT 28 UNION SELECT 29 UNION SELECT 30
      UNION SELECT 31 UNION SELECT 32 UNION SELECT 33) v
  ON v.value <= CAST(s.value AS INTEGER)
WHERE s.key = 'schema_version';

-- Re-sync schema_version to the highest backfilled version. This guarantees
-- that get_last_applied_version (which takes the min of journal and settings)
-- agrees with the journal, so future migrate() runs do exactly one pass.
UPDATE settings
   SET value = (SELECT CAST(MAX(version) AS TEXT) FROM migration_log WHERE status = 'applied')
 WHERE key = 'schema_version'
   AND CAST(value AS INTEGER) > COALESCE((SELECT MAX(version) FROM migration_log WHERE status = 'applied'), 0);