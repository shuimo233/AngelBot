-- Migration 043: Explicit local-owner and workspace scope for attention.
--
-- NULL scope is intentional for legacy rows or sessions without a canonical
-- workspace. Those cards remain visible only within their originating session.

ALTER TABLE attention_states ADD COLUMN scope_owner_profile_id INTEGER;
ALTER TABLE attention_states ADD COLUMN scope_workspace_key TEXT;

CREATE INDEX IF NOT EXISTS idx_attention_states_scope_open
    ON attention_states(scope_owner_profile_id, scope_workspace_key, updated_at DESC)
    WHERE status = 'open'
      AND scope_owner_profile_id IS NOT NULL
      AND scope_workspace_key IS NOT NULL;
