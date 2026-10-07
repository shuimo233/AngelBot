-- Context capacity is resolved from the active provider/model, rather than a
-- global application preference. Remove values written by previous versions.
DELETE FROM settings WHERE key = 'context_window_tokens';
