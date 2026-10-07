CREATE TABLE desktop_trusted_apps (
    id TEXT PRIMARY KEY NOT NULL,
    display_name TEXT NOT NULL,
    executable_path TEXT NOT NULL UNIQUE,
    capabilities TEXT NOT NULL DEFAULT '[]',
    draft_selector TEXT,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX idx_desktop_trusted_apps_enabled
    ON desktop_trusted_apps(enabled);
