-- Channel policy is explicit, local, and disabled by default.
ALTER TABLE channel_connections ADD COLUMN proactive_reason TEXT NOT NULL DEFAULT '';
ALTER TABLE channel_connections ADD COLUMN proactive_daily_limit INTEGER NOT NULL DEFAULT 0;
