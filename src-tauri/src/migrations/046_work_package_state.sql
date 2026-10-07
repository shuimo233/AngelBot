-- Minimal lifecycle guard for the user-visible delegation unit.
ALTER TABLE work_packages ADD COLUMN status TEXT NOT NULL DEFAULT 'active'
    CHECK(status IN ('draft', 'active', 'frozen', 'accepted', 'declined', 'cancelled', 'expired'));
