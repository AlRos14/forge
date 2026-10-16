-- PR10 removed Forge-owned model/runtime execution. Preserve the exact old
-- session rows and protected encrypted children as history, but remove the
-- active agent_session storage name and its runtime-only scope guards/index.
-- SQLite updates self-FKs and child FKs atomically when the table is renamed.
ALTER TABLE agent_session RENAME TO agent_session_history;

DROP TRIGGER agent_context_scope_identity_profile_guard;
DROP TRIGGER agent_context_scope_identity_profile_guard_update;

DROP INDEX idx_agent_session_active_scope;
DROP INDEX idx_agent_session_identity_status;
CREATE INDEX idx_agent_session_history_identity_created
    ON agent_session_history(identity_id, created_at DESC, id DESC);
CREATE INDEX idx_agent_session_history_identity_activity
    ON agent_session_history(identity_id, last_activity_at);
