-- The additive retry-source column is installed by `migrate()` after a
-- schema probe. SQLite lacks `ADD COLUMN IF NOT EXISTS`, and recovery can
-- replay this version when the column survived but its journal record did not.
CREATE UNIQUE INDEX IF NOT EXISTS idx_delegation_attempts_retry_source
    ON delegation_attempts(delegation_id, retry_source_delivery_id)
    WHERE retry_source_delivery_id IS NOT NULL;

CREATE TRIGGER IF NOT EXISTS trg_delegation_retry_source_matches_attempt
BEFORE INSERT ON delegation_attempts
FOR EACH ROW WHEN NEW.retry_source_delivery_id IS NOT NULL
BEGIN
    SELECT CASE WHEN NEW.attempt_number < 2
        THEN RAISE(ABORT, 'retry source requires a non-initial attempt') END;
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM delegation_deliveries source
        WHERE source.id = NEW.retry_source_delivery_id
          AND source.delegation_id = NEW.delegation_id
    ) THEN RAISE(ABORT, 'retry source delivery must belong to delegation') END;
END;
