CREATE TABLE account_usage_snapshot (
    id                  TEXT PRIMARY KEY,
    account_key         TEXT NOT NULL,
    executor_type       TEXT NOT NULL,
    daemon_id           TEXT,
    source              TEXT NOT NULL CHECK (source IN ('provider_event', 'cursor_usage', 'manual_refresh')),
    usage_json          TEXT NOT NULL CHECK (json_valid(usage_json)),
    captured_at         TEXT NOT NULL,
    stale_after         TEXT NOT NULL
);

CREATE INDEX idx_account_usage_snapshot_latest
    ON account_usage_snapshot(account_key, captured_at DESC);

CREATE TRIGGER account_usage_snapshot_immutable
BEFORE UPDATE ON account_usage_snapshot BEGIN
    SELECT RAISE(ABORT, 'Account usage snapshots are immutable');
END;
