CREATE TABLE execution_usage (
    id TEXT PRIMARY KEY NOT NULL,
    execution_id TEXT NOT NULL REFERENCES execution(id) ON DELETE CASCADE,
    provider TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL,
    created_at TEXT NOT NULL,
    UNIQUE(execution_id, provider, model)
);

CREATE INDEX idx_execution_usage_execution_id ON execution_usage(execution_id);
