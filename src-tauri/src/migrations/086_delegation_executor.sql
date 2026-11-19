ALTER TABLE delegations ADD COLUMN executor TEXT NOT NULL DEFAULT 'internal'
    CHECK (executor IN ('internal', 'codex_cli'));
