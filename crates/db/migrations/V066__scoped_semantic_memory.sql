-- Scoped semantic memory is an append-only projection.  V061 introduced the
-- first scoped shape, but its authority vocabulary predates the federation
-- contract and it did not retain enough source metadata to enforce history
-- boundaries before FTS/body access.  Rebuild the table rather than mutating
-- historical rows in place; all existing ids and bodies are copied exactly.
PRAGMA foreign_keys = OFF;

DROP TRIGGER memory_item_ai;
DROP TRIGGER memory_item_ad;
DROP TABLE memory_item_fts;
DROP INDEX idx_memory_item_project;
DROP INDEX idx_memory_item_task;
DROP INDEX idx_memory_item_room;
DROP INDEX idx_memory_item_scope;
DROP INDEX idx_memory_item_owner;
DROP INDEX idx_memory_item_kind;
DROP INDEX idx_memory_item_created_at;
ALTER TABLE memory_item RENAME TO memory_item_legacy;

CREATE TABLE memory_item (
    row_id                 INTEGER PRIMARY KEY,
    id                     TEXT NOT NULL UNIQUE,
    project_id             TEXT REFERENCES project(id) ON DELETE CASCADE,
    task_id                TEXT REFERENCES task(id) ON DELETE SET NULL,
    execution_id           TEXT REFERENCES execution(id) ON DELETE SET NULL,
    room_id                TEXT REFERENCES room(id) ON DELETE SET NULL,
    scope_type             TEXT NOT NULL
                                CHECK (scope_type IN ('account', 'project', 'room', 'task')),
    scope_id               TEXT NOT NULL,
    visibility             TEXT NOT NULL DEFAULT 'project'
                                CHECK (visibility IN (
                                    'private', 'participants', 'project', 'account'
                                )),
    owner_identity_id      TEXT REFERENCES agent_identity(id) ON DELETE SET NULL,
    authority              TEXT NOT NULL DEFAULT 'observation'
                                CHECK (authority IN (
                                    'observation', 'hypothesis', 'proposal', 'decision',
                                    'verified_fact', 'procedure'
                                )),
    sensitivity            TEXT NOT NULL DEFAULT 'internal'
                                CHECK (sensitivity IN ('public', 'internal', 'restricted', 'secret')),
    retention_priority     INTEGER NOT NULL DEFAULT 0,
    provenance_json        TEXT NOT NULL DEFAULT '{}',
    publication_source_id  TEXT REFERENCES memory_item(id) ON DELETE SET NULL,
    supersedes_id          TEXT REFERENCES memory_item(id) ON DELETE SET NULL,
    valid_from             TEXT,
    valid_until            TEXT,
    source_event_id        TEXT REFERENCES domain_event(id) ON DELETE SET NULL,
    source_scope_type      TEXT,
    source_scope_id        TEXT,
    source_revision        TEXT,
    source_room_sequence   INTEGER,
    source_type            TEXT NOT NULL,
    kind                   TEXT NOT NULL,
    title                  TEXT NOT NULL,
    summary                TEXT,
    body                   TEXT NOT NULL,
    metadata_json          TEXT NOT NULL DEFAULT '{}',
    confidence             TEXT,
    quality_score          INTEGER,
    created_by_type        TEXT,
    created_by_id          TEXT,
    created_at             TEXT NOT NULL
);

INSERT INTO memory_item (
    row_id, id, project_id, task_id, execution_id, room_id,
    scope_type, scope_id, visibility, owner_identity_id, authority,
    sensitivity, retention_priority, provenance_json, publication_source_id,
    supersedes_id, valid_from, valid_until, source_event_id,
    source_scope_type, source_scope_id, source_revision, source_room_sequence,
    source_type, kind, title, summary, body, metadata_json, confidence,
    quality_score, created_by_type, created_by_id, created_at
)
SELECT
    legacy.row_id,
    legacy.id,
    legacy.project_id,
    legacy.task_id,
    legacy.execution_id,
    legacy.room_id,
    legacy.scope_type,
    legacy.scope_id,
    legacy.visibility,
    legacy.owner_identity_id,
    CASE legacy.authority
        WHEN 'decision' THEN 'decision'
        WHEN 'procedure' THEN 'procedure'
        ELSE 'observation'
    END,
    CASE
        WHEN legacy.source_type IN ('room', 'room_message', 'conversation_message')
         AND (
             lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%authorization: bearer%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%api_key%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%sk-%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%private key%'
         ) THEN 'restricted'
        WHEN json_valid(legacy.metadata_json)
         AND json_extract(legacy.metadata_json, '$.sensitivity') IN ('public', 'internal', 'restricted', 'secret')
        THEN json_extract(legacy.metadata_json, '$.sensitivity')
        ELSE 'internal'
    END,
    CASE
        WHEN legacy.authority IN ('decision', 'procedure') THEN 100
        ELSE 10
    END,
    CASE
        WHEN legacy.source_type IN ('room', 'room_message', 'conversation_message')
         AND (
             lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%authorization: bearer%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%api_key%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%sk-%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%private key%'
         )
        THEN json_object(
            'migration', 'scoped-memory',
            'action', 'redacted',
            'classification', 'legacy_known_secret_pattern',
            'legacy_provenance', '[protected value redacted during migration]'
        )
        ELSE legacy.provenance_json
    END,
    legacy.publication_source_id,
    legacy.supersedes_id,
    legacy.valid_from,
    legacy.valid_until,
    legacy.source_event_id,
    CASE WHEN legacy.room_id IS NOT NULL THEN 'room' ELSE legacy.scope_type END,
    CASE WHEN legacy.room_id IS NOT NULL THEN legacy.room_id ELSE legacy.scope_id END,
    CASE WHEN legacy.room_id IS NOT NULL THEN CAST(message.sequence AS TEXT) ELSE NULL END,
    message.sequence,
    legacy.source_type,
    legacy.kind,
    CASE
        WHEN legacy.source_type IN ('room', 'room_message', 'conversation_message')
         AND (
             lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%authorization: bearer%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%api_key%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%sk-%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%private key%'
         )
        THEN '[protected memory redacted during migration]'
        ELSE legacy.title
    END,
    CASE
        WHEN legacy.source_type IN ('room', 'room_message', 'conversation_message')
         AND (
             lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%authorization: bearer%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%api_key%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%sk-%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%private key%'
         )
        THEN NULL
        ELSE legacy.summary
    END,
    CASE
        WHEN legacy.source_type IN ('room', 'room_message', 'conversation_message')
         AND (
             lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%authorization: bearer%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%api_key%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%sk-%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%private key%'
         )
        THEN '[protected value redacted during migration]'
        ELSE legacy.body
    END,
    CASE
        WHEN legacy.source_type IN ('room', 'room_message', 'conversation_message')
         AND (
             lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%authorization: bearer%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%api_key%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%sk-%'
             OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%private key%'
         )
        THEN '{"migration":"scoped-memory","action":"redacted"}'
        ELSE legacy.metadata_json
    END,
    legacy.confidence,
    legacy.quality_score,
    legacy.created_by_type,
    legacy.created_by_id,
    legacy.created_at
FROM memory_item_legacy AS legacy
LEFT JOIN room_message AS message ON message.id = legacy.id
    OR (
        legacy.source_type IN ('room', 'room_message')
        AND json_valid(legacy.metadata_json)
        AND json_extract(legacy.metadata_json, '$.source_ref') = message.id
    );

INSERT INTO content_guard_audit (
    entity_type, entity_id, action, classifier, original_length, created_at
)
SELECT
    'memory_item', legacy.id, 'redacted', 'legacy_known_secret_pattern',
    length(legacy.body), legacy.created_at
FROM memory_item_legacy AS legacy
WHERE legacy.source_type IN ('room', 'room_message', 'conversation_message')
  AND (
      lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%authorization: bearer%'
      OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%api_key%'
      OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%sk-%'
      OR lower(COALESCE(legacy.title, '') || ' ' || COALESCE(legacy.summary, '') || ' ' || legacy.body || ' ' || legacy.metadata_json) LIKE '%private key%'
  );

CREATE VIRTUAL TABLE memory_item_fts USING fts5(
    title,
    summary,
    body,
    content='memory_item',
    content_rowid='row_id'
);

INSERT INTO memory_item_fts(rowid, title, summary, body)
SELECT row_id, title, summary, body FROM memory_item;

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
CREATE INDEX idx_memory_item_room ON memory_item(room_id);
CREATE INDEX idx_memory_item_scope
    ON memory_item(scope_type, scope_id, created_at DESC, id DESC);
CREATE INDEX idx_memory_item_owner ON memory_item(owner_identity_id, visibility);
CREATE INDEX idx_memory_item_authority
    ON memory_item(authority, retention_priority DESC, created_at DESC);
CREATE INDEX idx_memory_item_source_scope
    ON memory_item(source_scope_type, source_scope_id, source_room_sequence);
CREATE INDEX idx_memory_item_created_at ON memory_item(created_at);

-- A receipt gives Room-message indexing a database idempotency key without
-- imposing a uniqueness constraint on historical memory rows that may already
-- contain duplicate source references.  The consumer inserts the item and
-- receipt in one transaction; a racing worker rolls back and reuses the
-- receipt's item.
CREATE TABLE memory_source_receipt (
    source_type       TEXT NOT NULL,
    source_scope_type TEXT NOT NULL,
    source_scope_id   TEXT NOT NULL,
    source_ref        TEXT NOT NULL,
    memory_item_id    TEXT NOT NULL REFERENCES memory_item(id) ON DELETE CASCADE,
    created_at        TEXT NOT NULL,
    PRIMARY KEY (source_type, source_scope_type, source_scope_id, source_ref),
    UNIQUE (memory_item_id)
);

CREATE INDEX idx_memory_source_receipt_item
    ON memory_source_receipt(memory_item_id);

-- V061 may already have copied a Room/conversation memory row.  Seed the
-- receipt from that preserved row so replaying its historical domain event
-- does not create a second semantic record.
INSERT OR IGNORE INTO memory_source_receipt (
    source_type, source_scope_type, source_scope_id, source_ref,
    memory_item_id, created_at
)
SELECT
    'room',
    memory_item.scope_type,
    memory_item.scope_id,
    COALESCE(
        CASE WHEN json_valid(memory_item.metadata_json)
             THEN json_extract(memory_item.metadata_json, '$.source_ref') END,
        room_message.id
    ),
    memory_item.id,
    memory_item.created_at
FROM memory_item
LEFT JOIN room_message
  ON room_message.id = memory_item.id
  OR (
      json_valid(memory_item.metadata_json)
      AND json_extract(memory_item.metadata_json, '$.source_ref') = room_message.id
  )
WHERE memory_item.scope_type = 'room'
  AND memory_item.source_type IN ('room', 'room_message')
  AND (
      (json_valid(memory_item.metadata_json)
       AND json_extract(memory_item.metadata_json, '$.source_ref') IS NOT NULL)
      OR room_message.id IS NOT NULL
  );

CREATE TRIGGER memory_item_immutable_update
BEFORE UPDATE ON memory_item
BEGIN
    SELECT RAISE(ABORT, 'memory items are append-only');
END;

-- Lifecycle is an append-only assertion ledger.  Existing assertions retain
-- their ids and timestamps; new promotion/publication evidence is represented
-- by additional assertions rather than updates to memory bodies/ACLs.
ALTER TABLE memory_lifecycle_assertion RENAME TO memory_lifecycle_assertion_legacy;
CREATE TABLE memory_lifecycle_assertion (
    id                  TEXT PRIMARY KEY,
    memory_item_id      TEXT NOT NULL REFERENCES memory_item(id) ON DELETE CASCADE,
    assertion_type      TEXT NOT NULL
                            CHECK (assertion_type IN (
                                'published', 'promoted', 'superseded', 'retracted',
                                'disputed', 'expired', 'evidence'
                            )),
    related_memory_id   TEXT REFERENCES memory_item(id) ON DELETE SET NULL,
    reason              TEXT,
    evidence_json       TEXT NOT NULL DEFAULT '{}',
    asserted_by_type    TEXT NOT NULL,
    asserted_by_id      TEXT,
    source_event_id     TEXT REFERENCES domain_event(id) ON DELETE SET NULL,
    created_at          TEXT NOT NULL
);

INSERT INTO memory_lifecycle_assertion (
    id, memory_item_id, assertion_type, related_memory_id, reason,
    evidence_json, asserted_by_type, asserted_by_id, source_event_id, created_at
)
SELECT id, memory_item_id, assertion_type, related_memory_id, reason, '{}',
       asserted_by_type, asserted_by_id, source_event_id, created_at
FROM memory_lifecycle_assertion_legacy;
DROP TABLE memory_lifecycle_assertion_legacy;

CREATE INDEX idx_memory_lifecycle_item
    ON memory_lifecycle_assertion(memory_item_id, created_at ASC, id ASC);
CREATE INDEX idx_memory_lifecycle_relation
    ON memory_lifecycle_assertion(related_memory_id, assertion_type);

CREATE TRIGGER memory_lifecycle_assertion_immutable_update
BEFORE UPDATE ON memory_lifecycle_assertion
BEGIN
    SELECT RAISE(ABORT, 'memory lifecycle assertions are append-only');
END;

-- A binding is deliberately immutable: a new identity/scope admission gets a
-- new binding id.  This prevents a long-lived runtime MemorySource from being
-- retargeted by updating a scope row underneath it.
CREATE TABLE forge_memory_source_binding (
    id                  TEXT PRIMARY KEY,
    identity_id         TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    context_scope_id    TEXT NOT NULL REFERENCES agent_context_scope(id) ON DELETE CASCADE,
    scope_type          TEXT NOT NULL CHECK (scope_type IN ('account', 'project', 'room', 'task')),
    scope_id            TEXT NOT NULL,
    account_id          TEXT,
    project_id          TEXT REFERENCES project(id) ON DELETE CASCADE,
    room_id             TEXT REFERENCES room(id) ON DELETE CASCADE,
    task_id             TEXT REFERENCES task(id) ON DELETE CASCADE,
    policy_revision     TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    UNIQUE(identity_id, context_scope_id),
    UNIQUE(identity_id, scope_type, scope_id, policy_revision)
);

CREATE TRIGGER forge_memory_source_binding_immutable_update
BEFORE UPDATE ON forge_memory_source_binding
BEGIN
    SELECT RAISE(ABORT, 'memory source bindings are immutable');
END;

CREATE TRIGGER forge_memory_source_binding_immutable_delete
BEFORE DELETE ON forge_memory_source_binding
BEGIN
    SELECT RAISE(ABORT, 'memory source bindings are immutable');
END;

CREATE TABLE context_manifest (
    id                       TEXT PRIMARY KEY,
    identity_id              TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    agent_session_id         TEXT REFERENCES agent_session(id) ON DELETE SET NULL,
    context_scope_id         TEXT NOT NULL REFERENCES agent_context_scope(id) ON DELETE CASCADE,
    scope_type               TEXT NOT NULL CHECK (scope_type IN ('account', 'project', 'room', 'task')),
    scope_id                 TEXT NOT NULL,
    policy_revision          TEXT NOT NULL,
    domain_revision          TEXT NOT NULL,
    lcm_binding_revision     TEXT,
    runtime_manifest_id      TEXT,
    runtime_manifest_fingerprint TEXT,
    combined_fingerprint     TEXT NOT NULL,
    request_fingerprint      TEXT NOT NULL,
    created_at               TEXT NOT NULL
);

CREATE TABLE context_manifest_source (
    manifest_id          TEXT NOT NULL REFERENCES context_manifest(id) ON DELETE CASCADE,
    ordinal              INTEGER NOT NULL,
    source_id             TEXT NOT NULL,
    source_type           TEXT NOT NULL,
    source_revision       TEXT NOT NULL,
    selection_reason      TEXT NOT NULL,
    disposition            TEXT NOT NULL CHECK (disposition IN (
        'offered', 'included', 'summarized', 'omitted', 'deduplicated', 'rejected'
    )),
    retention_priority    INTEGER NOT NULL DEFAULT 0,
    fragment_fingerprint  TEXT NOT NULL,
    PRIMARY KEY (manifest_id, ordinal),
    UNIQUE (manifest_id, source_id, source_revision)
);

CREATE TRIGGER context_manifest_immutable_update
BEFORE UPDATE ON context_manifest
BEGIN
    SELECT RAISE(ABORT, 'context manifests are immutable');
END;

CREATE TRIGGER context_manifest_immutable_delete
BEFORE DELETE ON context_manifest
BEGIN
    SELECT RAISE(ABORT, 'context manifests are immutable');
END;

CREATE TRIGGER context_manifest_source_immutable_update
BEFORE UPDATE ON context_manifest_source
BEGIN
    SELECT RAISE(ABORT, 'context manifest sources are immutable');
END;

CREATE TRIGGER context_manifest_source_immutable_delete
BEFORE DELETE ON context_manifest_source
BEGIN
    SELECT RAISE(ABORT, 'context manifest sources are immutable');
END;

PRAGMA foreign_keys = ON;
