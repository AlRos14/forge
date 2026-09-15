-- `cursor_poll` is a distinct observation provenance from provider-native
-- events. Preserve all existing snapshots while making the source truthful.
DROP TRIGGER account_usage_snapshot_immutable;
DROP INDEX idx_account_usage_snapshot_latest;
DROP INDEX idx_account_usage_snapshot_execution;
ALTER TABLE account_usage_snapshot RENAME TO account_usage_snapshot_old;

CREATE TABLE account_usage_snapshot (
    id                  TEXT PRIMARY KEY,
    account_key         TEXT NOT NULL,
    executor_type       TEXT NOT NULL,
    daemon_id           TEXT,
    source              TEXT NOT NULL CHECK (source IN ('provider_event', 'cursor_poll', 'cursor_usage', 'manual_refresh')),
    usage_json          TEXT NOT NULL CHECK (json_valid(usage_json)),
    captured_at         TEXT NOT NULL,
    stale_after         TEXT NOT NULL,
    execution_id        TEXT REFERENCES execution(id) ON DELETE SET NULL
);

INSERT INTO account_usage_snapshot (
    id, account_key, executor_type, daemon_id, source, usage_json,
    captured_at, stale_after, execution_id
)
SELECT id, account_key, executor_type, daemon_id, source, usage_json,
       captured_at, stale_after, execution_id
FROM account_usage_snapshot_old;

DROP TABLE account_usage_snapshot_old;

CREATE INDEX idx_account_usage_snapshot_latest
    ON account_usage_snapshot(account_key, captured_at DESC);

CREATE INDEX idx_account_usage_snapshot_execution
    ON account_usage_snapshot(execution_id, captured_at DESC)
    WHERE execution_id IS NOT NULL;

CREATE TRIGGER account_usage_snapshot_immutable
BEFORE UPDATE ON account_usage_snapshot BEGIN
    SELECT RAISE(ABORT, 'Account usage snapshots are immutable');
END;
