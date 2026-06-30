CREATE TABLE memory_item (
    row_id             INTEGER PRIMARY KEY,
    id                 TEXT NOT NULL UNIQUE,
    project_id         TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    task_id            TEXT REFERENCES task(id) ON DELETE SET NULL,
    execution_id       TEXT REFERENCES execution(id) ON DELETE SET NULL,
    conversation_id    TEXT REFERENCES conversation(id) ON DELETE SET NULL,
    source_type        TEXT NOT NULL,
    kind               TEXT NOT NULL,
    title              TEXT NOT NULL,
    summary            TEXT,
    body               TEXT NOT NULL,
    metadata_json      TEXT NOT NULL DEFAULT '{}',
    confidence         TEXT,
    quality_score      INTEGER,
    created_by_type    TEXT,
    created_by_id      TEXT,
    created_at         TEXT NOT NULL
);

CREATE VIRTUAL TABLE memory_item_fts USING fts5(
    title,
    summary,
    body,
    content='memory_item',
    content_rowid='row_id'
);

CREATE TRIGGER memory_item_ai
AFTER INSERT ON memory_item
BEGIN
    INSERT INTO memory_item_fts(rowid, title, summary, body)
    VALUES (new.row_id, new.title, new.summary, new.body);
END;

CREATE TRIGGER memory_item_ad
AFTER DELETE ON memory_item
BEGIN
    INSERT INTO memory_item_fts(memory_item_fts, rowid, title, summary, body)
    VALUES ('delete', old.row_id, old.title, old.summary, old.body);
END;

CREATE INDEX idx_memory_item_project ON memory_item(project_id);
CREATE INDEX idx_memory_item_task ON memory_item(task_id);
CREATE INDEX idx_memory_item_kind ON memory_item(kind);
CREATE INDEX idx_memory_item_created_at ON memory_item(created_at);
