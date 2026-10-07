-- A delegation may predate WorkPackage, but every delegation issued through
-- the main-agent work-package boundary records that immutable parent.
ALTER TABLE delegations ADD COLUMN work_package_id TEXT
    REFERENCES work_packages(id) ON DELETE RESTRICT;
CREATE INDEX IF NOT EXISTS idx_delegations_work_package
    ON delegations(work_package_id, updated_at DESC)
    WHERE work_package_id IS NOT NULL;
