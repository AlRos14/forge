ALTER TABLE daemon ADD COLUMN owner_id TEXT REFERENCES "user"(id) ON DELETE SET NULL;
ALTER TABLE daemon ADD COLUMN visibility TEXT NOT NULL DEFAULT 'global' CHECK (visibility IN ('global', 'account'));

CREATE INDEX idx_daemon_owner_id ON daemon(owner_id);

CREATE TABLE IF NOT EXISTS system_setting (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
