-- Persist a WorkPackage's selected profile and immutable policy contract for
-- audit/recovery.  These are policy identities only; they do not grant any
-- worker capability until a future runtime translates them through Gateway.
ALTER TABLE work_packages ADD COLUMN worker_profile TEXT NOT NULL DEFAULT 'explorer'
    CHECK(worker_profile IN ('explorer', 'implementer', 'verifier'));
ALTER TABLE work_packages ADD COLUMN worker_policy_version INTEGER NOT NULL DEFAULT 1
    CHECK(worker_policy_version > 0);
