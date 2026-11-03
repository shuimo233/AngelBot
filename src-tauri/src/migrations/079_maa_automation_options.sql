-- One explicitly authorized MAA preset, owned by an existing automation.
CREATE TABLE IF NOT EXISTS maa_automation_options (
    automation_id TEXT PRIMARY KEY REFERENCES automations(id) ON DELETE CASCADE,
    -- Deletion must invalidate consent even if the same app id is re-created.
    trusted_app_id TEXT NOT NULL REFERENCES desktop_trusted_apps(id) ON DELETE CASCADE,
    app_revision INTEGER NOT NULL,
    launch_emulator INTEGER NOT NULL CHECK (launch_emulator IN (0, 1))
);
