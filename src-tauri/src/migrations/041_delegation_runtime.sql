-- Migration 041: Durable delegated-task runtime foundation.
--
-- A delegation is the user-visible unit of work.  Attempts are disposable
-- executions of that delegation; this distinction keeps retries and recovery
-- from manufacturing new user-facing tasks.  Nothing in this schema dispatches
-- an agent or grants it access: it only persists the records a future runtime
-- must write before doing so.

CREATE TABLE IF NOT EXISTS delegations (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    parent_run_id TEXT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    objective TEXT NOT NULL,
    brief_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN (
        'queued', 'running', 'awaiting_confirmation', 'awaiting_summary',
        'completed', 'failed', 'cancelled', 'needs_decision'
    )),
    state_version INTEGER NOT NULL DEFAULT 0 CHECK (state_version >= 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_delegations_session_updated
    ON delegations(session_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_delegations_parent_run
    ON delegations(parent_run_id);

-- A task run is the authoritative parent tuple; accepting an arbitrary run
-- from another conversation would leak context across the main-agent boundary.
CREATE TRIGGER IF NOT EXISTS trg_delegations_parent_links_match
BEFORE INSERT ON delegations
FOR EACH ROW
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM task_runs r
        WHERE r.id = NEW.parent_run_id
          AND r.session_id = NEW.session_id
          AND r.message_id = NEW.message_id
    ) THEN RAISE(ABORT, 'delegation parent links must match task run') END;
END;

CREATE TABLE IF NOT EXISTS delegation_attempts (
    id TEXT PRIMARY KEY,
    delegation_id TEXT NOT NULL REFERENCES delegations(id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL CHECK (attempt_number > 0),
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'sealed', 'failed', 'cancelled')),
    sandbox_ref TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    ended_at INTEGER,
    UNIQUE(delegation_id, attempt_number)
);

CREATE INDEX IF NOT EXISTS idx_delegation_attempts_delegation
    ON delegation_attempts(delegation_id, attempt_number DESC);
-- Multiple historical attempts are valid, but a delegation never has two
-- concurrently dispatchable attempts competing for the same task.
CREATE UNIQUE INDEX IF NOT EXISTS idx_delegation_attempts_one_active
    ON delegation_attempts(delegation_id)
    WHERE status IN ('queued', 'running');

CREATE TABLE IF NOT EXISTS delegation_capability_leases (
    id TEXT PRIMARY KEY,
    attempt_id TEXT NOT NULL UNIQUE REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    read_roots_json TEXT NOT NULL,
    write_roots_json TEXT NOT NULL,
    tool_allowlist_json TEXT NOT NULL,
    network_hosts_json TEXT NOT NULL,
    budget_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'expired')),
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    CHECK (expires_at > issued_at),
    CHECK ((status = 'revoked') = (revoked_at IS NOT NULL))
);

CREATE TABLE IF NOT EXISTS delegation_deliveries (
    id TEXT PRIMARY KEY,
    delegation_id TEXT NOT NULL REFERENCES delegations(id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    delivery_version INTEGER NOT NULL CHECK (delivery_version > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    payload_json TEXT NOT NULL,
    acceptance_status TEXT NOT NULL DEFAULT 'submitted'
        CHECK (acceptance_status IN ('submitted', 'accepted', 'rejected')),
    accepted_by TEXT CHECK (accepted_by IS NULL OR accepted_by = 'parent_agent'),
    accepted_at INTEGER,
    created_at INTEGER NOT NULL,
    UNIQUE(attempt_id, delivery_version),
    CHECK ((acceptance_status = 'accepted') = (accepted_by = 'parent_agent' AND accepted_at IS NOT NULL)),
    CHECK (acceptance_status != 'rejected' OR (accepted_by IS NULL AND accepted_at IS NULL))
);

CREATE INDEX IF NOT EXISTS idx_delegation_deliveries_delegation_created
    ON delegation_deliveries(delegation_id, created_at DESC);

CREATE TRIGGER IF NOT EXISTS trg_delegation_delivery_matches_attempt
BEFORE INSERT ON delegation_deliveries
FOR EACH ROW
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM delegation_attempts a
        WHERE a.id = NEW.attempt_id AND a.delegation_id = NEW.delegation_id
    ) THEN RAISE(ABORT, 'delivery attempt must belong to delegation') END;
END;

-- Parent-only acceptance: child delivery submission cannot make a delegation
-- complete.  The repository performs acceptance and the DB verifies that an
-- accepted delivery belongs to the delegation it updates.
CREATE TRIGGER IF NOT EXISTS trg_delegations_completed_requires_parent_acceptance
BEFORE UPDATE OF status ON delegations
FOR EACH ROW WHEN NEW.status = 'completed'
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM delegation_deliveries d
        WHERE d.delegation_id = NEW.id
          AND d.acceptance_status = 'accepted'
          AND d.accepted_by = 'parent_agent'
    ) THEN RAISE(ABORT, 'completed delegation requires parent-accepted delivery') END;
END;

CREATE TABLE IF NOT EXISTS delegation_attempt_events (
    id TEXT PRIMARY KEY,
    attempt_id TEXT NOT NULL REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(attempt_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_delegation_attempt_events_attempt_sequence
    ON delegation_attempt_events(attempt_id, sequence);

-- The outbox is inserted in the same transaction as a newly queued attempt.
-- A dispatcher may later claim it, but may never dispatch work not represented
-- here first.
CREATE TABLE IF NOT EXISTS delegation_outbox (
    id TEXT PRIMARY KEY,
    attempt_id TEXT NOT NULL REFERENCES delegation_attempts(id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    dispatched_at INTEGER,
    UNIQUE(attempt_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_delegation_outbox_pending
    ON delegation_outbox(dispatched_at, created_at);
