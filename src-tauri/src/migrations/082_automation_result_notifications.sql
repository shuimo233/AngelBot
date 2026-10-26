-- A receipt only, never a retry queue. NULL means no attempt recorded.
-- A crash after claiming leaves unconfirmed; startup must not send it again.
ALTER TABLE automation_runs ADD COLUMN notification_status TEXT
    CHECK (notification_status IS NULL OR notification_status IN
        ('unconfirmed', 'requested', 'muted', 'unavailable'));
