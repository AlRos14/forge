CREATE TABLE task_media (
    id               TEXT PRIMARY KEY,
    task_id          TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    display_filename TEXT NOT NULL,
    content_type     TEXT NOT NULL,
    byte_size        INTEGER NOT NULL CHECK (byte_size >= 0),
    storage_key      TEXT NOT NULL UNIQUE,
    author_type      TEXT NOT NULL CHECK (author_type IN ('user', 'agent', 'system')),
    author_id        TEXT,
    author_name      TEXT NOT NULL,
    created_at       TEXT NOT NULL,
    deleted_at       TEXT
);

CREATE INDEX idx_task_media_task_created ON task_media(task_id, created_at);
CREATE INDEX idx_task_media_task_deleted ON task_media(task_id, deleted_at);
