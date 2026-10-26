-- Restored paths are information, not authority on the receiving machine.
-- Existing locally created projects retain their current authorization.
ALTER TABLE projects ADD COLUMN directory_confirmed INTEGER NOT NULL DEFAULT 1
    CHECK (directory_confirmed IN (0, 1));
