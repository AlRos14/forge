PRAGMA foreign_keys = OFF;

ALTER TABLE conversation RENAME TO conversation_legacy;
ALTER TABLE conversation_message RENAME TO conversation_message_legacy;

CREATE TABLE room (
    id                              TEXT PRIMARY KEY,
    scope_type                      TEXT NOT NULL CHECK (scope_type IN ('account', 'project')),
    scope_id                        TEXT NOT NULL,
    owner_user_id                   TEXT,
    owning_project_id               TEXT REFERENCES project(id) ON DELETE CASCADE,
    title                           TEXT NOT NULL,
    status                          TEXT NOT NULL DEFAULT 'active'
                                        CHECK (status IN ('active', 'archived')),
    responder_policy                TEXT NOT NULL DEFAULT 'explicit_identity'
                                        CHECK (responder_policy IN (
                                            'explicit_identity', 'project_primary'
                                        )),
    default_responder_identity_id   TEXT REFERENCES agent_identity(id) ON DELETE SET NULL,
    history_policy                  TEXT NOT NULL DEFAULT 'participants'
                                        CHECK (history_policy IN (
                                            'owner_only', 'participants', 'project_members'
                                        )),
    message_count                   INTEGER NOT NULL DEFAULT 0,
    last_message_at                 TEXT,
    version                         INTEGER NOT NULL DEFAULT 1,
    created_at                      TEXT NOT NULL,
    updated_at                      TEXT NOT NULL,
    CHECK (
        (scope_type = 'account' AND owning_project_id IS NULL AND owner_user_id = scope_id)
        OR
        (scope_type = 'project' AND owning_project_id = scope_id)
    )
);

CREATE INDEX idx_room_scope_updated
    ON room(scope_type, scope_id, updated_at DESC, id DESC);
CREATE INDEX idx_room_project
    ON room(owning_project_id, updated_at DESC);

CREATE TABLE room_instruction_revision (
    id                  TEXT PRIMARY KEY,
    room_id             TEXT NOT NULL REFERENCES room(id) ON DELETE CASCADE,
    revision            INTEGER NOT NULL,
    body                TEXT NOT NULL,
    content_guard_json  TEXT NOT NULL DEFAULT '{}',
    sensitivity         TEXT NOT NULL DEFAULT 'internal'
                            CHECK (sensitivity IN ('public', 'internal', 'restricted')),
    created_by_type     TEXT NOT NULL,
    created_by_id       TEXT,
    created_at          TEXT NOT NULL,
    UNIQUE (room_id, revision)
);

CREATE TRIGGER room_instruction_revision_immutable_update
BEFORE UPDATE ON room_instruction_revision
BEGIN
    SELECT RAISE(ABORT, 'room instruction revisions are immutable');
END;

CREATE TRIGGER room_instruction_revision_immutable_delete
BEFORE DELETE ON room_instruction_revision
BEGIN
    SELECT RAISE(ABORT, 'room instruction revisions are immutable');
END;

CREATE TABLE room_participant (
    room_id             TEXT NOT NULL REFERENCES room(id) ON DELETE CASCADE,
    participant_type    TEXT NOT NULL CHECK (participant_type IN ('user', 'agent')),
    participant_id      TEXT NOT NULL,
    role                TEXT NOT NULL DEFAULT 'participant'
                            CHECK (role IN ('owner', 'participant', 'responder')),
    can_read_history    INTEGER NOT NULL DEFAULT 1,
    state               TEXT NOT NULL DEFAULT 'active'
                            CHECK (state IN ('active', 'removed')),
    joined_at           TEXT NOT NULL,
    removed_at          TEXT,
    PRIMARY KEY (room_id, participant_type, participant_id)
);

CREATE INDEX idx_room_participant_identity
    ON room_participant(participant_type, participant_id, state, room_id);

CREATE TABLE room_message (
    id                          TEXT PRIMARY KEY,
    room_id                     TEXT NOT NULL REFERENCES room(id) ON DELETE CASCADE,
    author_type                 TEXT NOT NULL CHECK (author_type IN ('user', 'agent', 'system')),
    author_id                   TEXT,
    addressed_identity_id       TEXT REFERENCES agent_identity(id) ON DELETE SET NULL,
    reply_to_message_id         TEXT REFERENCES room_message(id) ON DELETE SET NULL,
    content                     TEXT NOT NULL,
    content_guard_json          TEXT NOT NULL DEFAULT '{}',
    sensitivity                 TEXT NOT NULL DEFAULT 'internal'
                                    CHECK (sensitivity IN ('public', 'internal', 'restricted')),
    status                      TEXT NOT NULL CHECK (status IN ('complete', 'failed', 'cancelled')),
    outcome                     TEXT,
    model                       TEXT,
    profile_id                  TEXT REFERENCES agent_profile(id) ON DELETE SET NULL,
    session_id                  TEXT,
    token_usage_json            TEXT,
    duration_ms                 INTEGER,
    error                       TEXT,
    correlation_id              TEXT NOT NULL,
    source_event_id             TEXT,
    sequence                    INTEGER NOT NULL,
    created_at                  TEXT NOT NULL,
    UNIQUE(room_id, sequence)
);

CREATE INDEX idx_room_message_room_sequence
    ON room_message(room_id, sequence ASC);
CREATE INDEX idx_room_message_author
    ON room_message(author_type, author_id, created_at DESC);

CREATE TRIGGER room_message_immutable_update
BEFORE UPDATE ON room_message
BEGIN
    SELECT RAISE(ABORT, 'room messages are immutable');
END;

CREATE TRIGGER room_message_immutable_delete
BEFORE DELETE ON room_message
BEGIN
    SELECT RAISE(ABORT, 'room messages are immutable');
END;

CREATE TABLE agent_turn_job (
    id                      TEXT PRIMARY KEY,
    room_id                 TEXT NOT NULL REFERENCES room(id) ON DELETE CASCADE,
    input_message_id        TEXT NOT NULL REFERENCES room_message(id) ON DELETE CASCADE,
    responder_identity_id   TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type              TEXT NOT NULL CHECK (scope_type IN ('room')),
    scope_id                TEXT NOT NULL,
    status                  TEXT NOT NULL DEFAULT 'queued'
                                CHECK (status IN (
                                    'queued', 'leased', 'running', 'completed', 'failed',
                                    'cancelled', 'suppressed'
                                )),
    dedupe_key              TEXT NOT NULL UNIQUE,
    lease_owner             TEXT,
    leased_until            TEXT,
    attempt_count           INTEGER NOT NULL DEFAULT 0,
    response_message_id     TEXT REFERENCES room_message(id) ON DELETE SET NULL,
    error                   TEXT,
    correlation_id          TEXT NOT NULL,
    causation_id            TEXT,
    causation_depth         INTEGER NOT NULL DEFAULT 0 CHECK (causation_depth BETWEEN 0 AND 16),
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    CHECK (scope_id = room_id)
);

CREATE INDEX idx_agent_turn_job_dispatch
    ON agent_turn_job(responder_identity_id, scope_type, scope_id, status, created_at, id);
CREATE UNIQUE INDEX idx_agent_turn_job_active_scope
    ON agent_turn_job(responder_identity_id, scope_type, scope_id)
    WHERE status IN ('leased', 'running');

CREATE TABLE bounded_room_round (
    id                      TEXT PRIMARY KEY,
    room_id                 TEXT NOT NULL REFERENCES room(id) ON DELETE CASCADE,
    prompt_message_id       TEXT NOT NULL REFERENCES room_message(id) ON DELETE CASCADE,
    deadline_at             TEXT NOT NULL,
    response_contract_json  TEXT NOT NULL DEFAULT '{}',
    status                  TEXT NOT NULL DEFAULT 'open'
                                CHECK (status IN ('open', 'synthesizing', 'complete', 'expired')),
    synthesis_job_id        TEXT REFERENCES agent_turn_job(id) ON DELETE SET NULL,
    version                 INTEGER NOT NULL DEFAULT 1,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);

CREATE TABLE bounded_room_round_participant (
    round_id                TEXT NOT NULL REFERENCES bounded_room_round(id) ON DELETE CASCADE,
    identity_id             TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    turn_job_id             TEXT REFERENCES agent_turn_job(id) ON DELETE SET NULL,
    response_message_id     TEXT REFERENCES room_message(id) ON DELETE SET NULL,
    status                  TEXT NOT NULL DEFAULT 'pending'
                                CHECK (status IN ('pending', 'responded', 'failed', 'expired')),
    PRIMARY KEY (round_id, identity_id)
);

CREATE TABLE protected_legacy_session_ref (
    room_id                 TEXT PRIMARY KEY REFERENCES room(id) ON DELETE CASCADE,
    opaque_session_ref      TEXT NOT NULL,
    access_class            TEXT NOT NULL DEFAULT 'protected',
    migrated_at             TEXT NOT NULL
);

CREATE TABLE content_guard_audit (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type         TEXT NOT NULL,
    entity_id           TEXT NOT NULL,
    action              TEXT NOT NULL,
    classifier          TEXT NOT NULL,
    original_length     INTEGER NOT NULL,
    created_at          TEXT NOT NULL
);

INSERT INTO room (
    id, scope_type, scope_id, owner_user_id, owning_project_id, title, status,
    responder_policy, default_responder_identity_id, history_policy,
    message_count, last_message_at, version, created_at, updated_at
)
SELECT
    conversation_legacy.id,
    'project',
    conversation_legacy.project_id,
    project.owner_id,
    conversation_legacy.project_id,
    conversation_legacy.title,
    conversation_legacy.status,
    'explicit_identity',
    conversation_legacy.agent_id,
    'project_members',
    conversation_legacy.message_count,
    conversation_legacy.last_message_at,
    conversation_legacy.version,
    conversation_legacy.created_at,
    conversation_legacy.updated_at
FROM conversation_legacy
JOIN project ON project.id = conversation_legacy.project_id;

INSERT INTO room_instruction_revision (
    id, room_id, revision, body, content_guard_json, sensitivity,
    created_by_type, created_by_id, created_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    conversation_legacy.id,
    1,
    CASE
        WHEN lower(conversation_legacy.system_prompt) LIKE '%authorization: bearer%'
          OR lower(conversation_legacy.system_prompt) LIKE '%api_key%'
          OR lower(conversation_legacy.system_prompt) LIKE '%sk-%'
        THEN '[protected value redacted during migration]'
        ELSE conversation_legacy.system_prompt
    END,
    CASE
        WHEN lower(conversation_legacy.system_prompt) LIKE '%authorization: bearer%'
          OR lower(conversation_legacy.system_prompt) LIKE '%api_key%'
          OR lower(conversation_legacy.system_prompt) LIKE '%sk-%'
        THEN '{"migration":"legacy","action":"redacted","classification":"secret"}'
        ELSE '{"migration":"legacy","action":"admitted","classification":"unreviewed"}'
    END,
    CASE
        WHEN lower(conversation_legacy.system_prompt) LIKE '%authorization: bearer%'
          OR lower(conversation_legacy.system_prompt) LIKE '%api_key%'
          OR lower(conversation_legacy.system_prompt) LIKE '%sk-%'
        THEN 'restricted'
        ELSE 'internal'
    END,
    'migration',
    NULL,
    conversation_legacy.created_at
FROM conversation_legacy
WHERE conversation_legacy.system_prompt IS NOT NULL;

INSERT INTO content_guard_audit (
    entity_type, entity_id, action, classifier, original_length, created_at
)
SELECT
    'room_instruction_revision',
    conversation_legacy.id,
    'redacted',
    'legacy_known_secret_pattern',
    length(conversation_legacy.system_prompt),
    conversation_legacy.updated_at
FROM conversation_legacy
WHERE conversation_legacy.system_prompt IS NOT NULL
  AND (
      lower(conversation_legacy.system_prompt) LIKE '%authorization: bearer%'
      OR lower(conversation_legacy.system_prompt) LIKE '%api_key%'
      OR lower(conversation_legacy.system_prompt) LIKE '%sk-%'
  );

INSERT INTO room_participant (
    room_id, participant_type, participant_id, role,
    can_read_history, state, joined_at, removed_at
)
SELECT
    id, 'agent', agent_id, 'responder', 1, 'active', created_at, NULL
FROM conversation_legacy
WHERE agent_id IS NOT NULL;

INSERT INTO room_message (
    id, room_id, author_type, author_id, addressed_identity_id,
    reply_to_message_id, content, content_guard_json, sensitivity, status,
    outcome, model, profile_id, session_id, token_usage_json, duration_ms,
    error, correlation_id, source_event_id, sequence, created_at
)
SELECT
    message.id,
    message.conversation_id,
    CASE message.role
        WHEN 'assistant' THEN 'agent'
        WHEN 'user' THEN 'user'
        ELSE 'system'
    END,
    CASE message.role
        WHEN 'assistant' THEN conversation_legacy.agent_id
        WHEN 'user' THEN project.owner_id
        ELSE NULL
    END,
    CASE WHEN message.role = 'user' THEN conversation_legacy.agent_id ELSE NULL END,
    NULL,
    CASE
        WHEN lower(message.content) LIKE '%authorization: bearer%'
          OR lower(message.content) LIKE '%api_key%'
          OR lower(message.content) LIKE '%sk-%'
        THEN '[protected value redacted during migration]'
        ELSE message.content
    END,
    CASE
        WHEN lower(message.content) LIKE '%authorization: bearer%'
          OR lower(message.content) LIKE '%api_key%'
          OR lower(message.content) LIKE '%sk-%'
        THEN '{"migration":"legacy","action":"redacted","classification":"secret"}'
        ELSE '{"migration":"legacy","action":"admitted","classification":"unreviewed"}'
    END,
    CASE
        WHEN lower(message.content) LIKE '%authorization: bearer%'
          OR lower(message.content) LIKE '%api_key%'
          OR lower(message.content) LIKE '%sk-%'
        THEN 'restricted'
        ELSE 'internal'
    END,
    CASE WHEN message.status = 'streaming' THEN 'failed' ELSE message.status END,
    CASE WHEN message.status = 'streaming' THEN 'interrupted_migration' ELSE NULL END,
    message.model,
    NULL,
    NULL,
    message.token_usage_json,
    message.duration_ms,
    CASE
        WHEN message.status = 'streaming'
        THEN COALESCE(message.error, 'interrupted during scoped Room migration')
        ELSE message.error
    END,
    message.id,
    NULL,
    message.sequence,
    message.created_at
FROM conversation_message_legacy AS message
JOIN conversation_legacy ON conversation_legacy.id = message.conversation_id
JOIN project ON project.id = conversation_legacy.project_id
ORDER BY message.conversation_id, message.sequence;

INSERT INTO content_guard_audit (
    entity_type, entity_id, action, classifier, original_length, created_at
)
SELECT
    'room_message', message.id, 'redacted', 'legacy_known_secret_pattern',
    length(message.content), message.updated_at
FROM conversation_message_legacy AS message
WHERE lower(message.content) LIKE '%authorization: bearer%'
   OR lower(message.content) LIKE '%api_key%'
   OR lower(message.content) LIKE '%sk-%';

INSERT INTO protected_legacy_session_ref (
    room_id, opaque_session_ref, access_class, migrated_at
)
SELECT id, agent_session_id, 'protected', updated_at
FROM conversation_legacy
WHERE agent_session_id IS NOT NULL;

-- Rebuild semantic memory so message references point at Rooms and so all
-- candidates carry an explicit canonical scope before FTS admission.
DROP TRIGGER memory_item_ai;
DROP TRIGGER memory_item_ad;
DROP TABLE memory_item_fts;
DROP INDEX idx_memory_item_project;
DROP INDEX idx_memory_item_task;
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
                                    'observation', 'decision', 'policy', 'procedure', 'commitment'
                                )),
    provenance_json        TEXT NOT NULL DEFAULT '{}',
    publication_source_id  TEXT REFERENCES memory_item(id) ON DELETE SET NULL,
    supersedes_id          TEXT REFERENCES memory_item(id) ON DELETE SET NULL,
    valid_from             TEXT,
    valid_until            TEXT,
    source_event_id        TEXT REFERENCES domain_event(id) ON DELETE SET NULL,
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
    provenance_json, publication_source_id, supersedes_id, valid_from,
    valid_until, source_event_id, source_type, kind, title, summary, body,
    metadata_json, confidence, quality_score, created_by_type, created_by_id,
    created_at
)
SELECT
    row_id, id, project_id, task_id, execution_id, conversation_id,
    CASE
        WHEN conversation_id IS NOT NULL THEN 'room'
        WHEN task_id IS NOT NULL THEN 'task'
        ELSE 'project'
    END,
    COALESCE(conversation_id, task_id, project_id),
    CASE WHEN conversation_id IS NOT NULL THEN 'participants' ELSE 'project' END,
    CASE WHEN created_by_type = 'agent' THEN created_by_id ELSE NULL END,
    'observation',
    json_object('migration', 'legacy-memory-item', 'legacy_source_type', source_type),
    NULL, NULL, created_at, NULL, NULL,
    CASE WHEN source_type = 'conversation' THEN 'room_message' ELSE source_type END,
    kind, title, summary, body, metadata_json, confidence, quality_score,
    created_by_type, created_by_id, created_at
FROM memory_item_legacy;

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
CREATE INDEX idx_memory_item_scope ON memory_item(scope_type, scope_id, created_at DESC, id DESC);
CREATE INDEX idx_memory_item_owner ON memory_item(owner_identity_id, visibility);
CREATE INDEX idx_memory_item_kind ON memory_item(kind);
CREATE INDEX idx_memory_item_created_at ON memory_item(created_at);

CREATE TABLE memory_lifecycle_assertion (
    id                  TEXT PRIMARY KEY,
    memory_item_id      TEXT NOT NULL REFERENCES memory_item(id) ON DELETE CASCADE,
    assertion_type      TEXT NOT NULL
                            CHECK (assertion_type IN (
                                'published', 'superseded', 'retracted', 'disputed', 'expired'
                            )),
    related_memory_id   TEXT REFERENCES memory_item(id) ON DELETE SET NULL,
    reason              TEXT,
    asserted_by_type    TEXT NOT NULL,
    asserted_by_id      TEXT,
    source_event_id     TEXT REFERENCES domain_event(id) ON DELETE SET NULL,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_memory_lifecycle_item
    ON memory_lifecycle_assertion(memory_item_id, created_at ASC, id ASC);

-- Backfill bounded, content-free Room event history for replay consumers.
INSERT INTO domain_event (
    id, event_type, entity_type, entity_id, actor_type, actor_id,
    scope_type, scope_id, correlation_id, causation_id, causation_depth,
    dedupe_key, payload_json, created_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    'room.message.admitted',
    'room_message',
    room_message.id,
    room_message.author_type,
    room_message.author_id,
    'room',
    room_message.room_id,
    room_message.correlation_id,
    NULL,
    0,
    'migration:room-message:' || room_message.id,
    json_object(
        'room_id', room_message.room_id,
        'message_id', room_message.id,
        'author_type', room_message.author_type,
        'sequence', room_message.sequence,
        'status', room_message.status,
        'sensitivity', room_message.sensitivity
    ),
    room_message.created_at
FROM room_message
ORDER BY room_message.created_at ASC, room_message.id ASC;

DROP TABLE memory_item_legacy;
DROP TABLE conversation_message_legacy;
DROP TABLE conversation_legacy;

PRAGMA foreign_keys = ON;
