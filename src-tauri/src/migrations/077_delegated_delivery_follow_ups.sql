-- A reviewed read-only Explorer result still belongs to the Main Agent.
--
-- This binding turns one passed producer delivery revision into exactly one
-- generic Workspace follow-up input.  It deliberately stores identifiers and
-- lifecycle state only: structured evidence is reassembled by the foreground
-- context boundary, so raw child payloads and execution logs never enter the
-- durable prompt queue.
CREATE TABLE IF NOT EXISTS delegated_delivery_follow_ups (
    delivery_id TEXT NOT NULL
        REFERENCES delegation_deliveries(id) ON DELETE CASCADE,
    delivery_revision INTEGER NOT NULL CHECK(delivery_revision > 0),
    supervisor_input_id TEXT NOT NULL UNIQUE
        REFERENCES workspace_supervisor_inputs(id) ON DELETE CASCADE,
    foreground_run_id TEXT,
    status TEXT NOT NULL CHECK(status IN (
        'queued', 'running', 'summarized', 'cancelled'
    )),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(delivery_id, delivery_revision),
    CHECK(status != 'queued' OR foreground_run_id IS NULL),
    CHECK(status NOT IN ('running', 'summarized') OR foreground_run_id IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS idx_delegated_delivery_follow_ups_status
    ON delegated_delivery_follow_ups(status, updated_at);

-- A delivery revision is immutable.  The trigger keeps the mapping tied to
-- the same parent-owned source even if future lifecycle code updates status.
CREATE TRIGGER IF NOT EXISTS trg_delegated_delivery_follow_up_identity
BEFORE INSERT ON delegated_delivery_follow_ups
FOR EACH ROW BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM delegation_deliveries delivery
        WHERE delivery.id = NEW.delivery_id
          AND delivery.delivery_version = NEW.delivery_revision
    ) THEN RAISE(ABORT, 'follow-up must match delivery revision') END;
END;

CREATE TRIGGER IF NOT EXISTS trg_delegated_delivery_follow_up_immutable
BEFORE UPDATE ON delegated_delivery_follow_ups
FOR EACH ROW WHEN NEW.delivery_id != OLD.delivery_id
  OR NEW.delivery_revision != OLD.delivery_revision
  OR NEW.supervisor_input_id != OLD.supervisor_input_id
  OR NEW.created_at != OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'follow-up identity is immutable');
END;

-- Removing the source delivery must not leave an ownerless generic input for
-- a later foreground pump to consume.  The same statement is harmless when
-- the input deletion itself cascades back to this binding.
CREATE TRIGGER IF NOT EXISTS trg_delegated_delivery_follow_up_cleans_input
AFTER DELETE ON delegated_delivery_follow_ups
FOR EACH ROW BEGIN
    DELETE FROM workspace_supervisor_inputs
    WHERE id = OLD.supervisor_input_id;
END;
