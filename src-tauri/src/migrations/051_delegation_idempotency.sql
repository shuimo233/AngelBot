-- Durable idempotency for the Main Agent's delegate_work tool call.
-- Historical delegations remain valid with NULL; only newly issued work
-- supplies the parent-scoped tool-call key.
ALTER TABLE delegations ADD COLUMN idempotency_key TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_delegations_parent_idempotency
    ON delegations(parent_run_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
