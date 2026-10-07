-- A scheduler heartbeat can overlap with another heartbeat while a task is
-- executing. Persist a short-lived ownership token so an overdue task is only
-- launched once, while a crash can still be recovered after the bound expires.
ALTER TABLE automations ADD COLUMN schedule_claim_token TEXT;
ALTER TABLE automations ADD COLUMN schedule_claimed_at INTEGER;

CREATE INDEX IF NOT EXISTS idx_automations_schedule_claim
ON automations(enabled, trigger_kind, next_run_at, schedule_claimed_at);
