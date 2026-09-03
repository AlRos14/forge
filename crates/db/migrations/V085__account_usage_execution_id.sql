ALTER TABLE account_usage_snapshot
    ADD COLUMN execution_id TEXT REFERENCES execution(id) ON DELETE SET NULL;

CREATE INDEX idx_account_usage_snapshot_execution
    ON account_usage_snapshot(execution_id, captured_at DESC)
    WHERE execution_id IS NOT NULL;
