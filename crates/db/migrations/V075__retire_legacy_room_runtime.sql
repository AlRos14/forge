-- The singular Agent Chat ledger (V071+) is the only live interaction
-- surface.  Keep the pre-release Room and membership rows for audit,
-- migration replay, and provenance inspection, but quarantine their table
-- names so no runtime repository can accidentally write them again.
--
-- SQLite updates foreign-key declarations and trigger bodies when a table is
-- renamed.  Rename children before their legacy parents so every historical
-- relationship remains valid after the quarantine.

PRAGMA foreign_keys = OFF;

ALTER TABLE project_agent_membership RENAME TO legacy_project_agent_membership;
ALTER TABLE bounded_room_round_participant RENAME TO legacy_bounded_room_round_participant;
ALTER TABLE bounded_room_round RENAME TO legacy_bounded_room_round;
ALTER TABLE agent_turn_job RENAME TO legacy_agent_turn_job;
ALTER TABLE room_message RENAME TO legacy_room_message;
ALTER TABLE room_participant RENAME TO legacy_room_participant;
ALTER TABLE room_instruction_revision RENAME TO legacy_room_instruction_revision;
ALTER TABLE room RENAME TO legacy_room;

-- Canonical Agent Chat memory uses its chat id as scope and the singular
-- `chat` visibility. Preserve the original Room id/sequence only as source
-- provenance; it is no longer an authorizable runtime scope.
DROP TRIGGER memory_item_ai;
DROP TRIGGER memory_item_ad;
DROP TRIGGER memory_item_immutable_update;
DROP TABLE memory_item_fts;
DROP INDEX idx_memory_item_project;
DROP INDEX idx_memory_item_task;
DROP INDEX idx_memory_item_room;
DROP INDEX idx_memory_item_scope;
DROP INDEX idx_memory_item_owner;
DROP INDEX idx_memory_item_authority;
DROP INDEX idx_memory_item_source_scope;
DROP INDEX idx_memory_item_created_at;

CREATE TABLE memory_item_new (
    row_id                 INTEGER PRIMARY KEY,
    id                     TEXT NOT NULL UNIQUE,
    project_id             TEXT REFERENCES project(id) ON DELETE CASCADE,
    task_id                TEXT REFERENCES task(id) ON DELETE SET NULL,
    execution_id           TEXT REFERENCES execution(id) ON DELETE SET NULL,
    room_id                TEXT REFERENCES legacy_room(id) ON DELETE SET NULL,
    scope_type             TEXT NOT NULL
                                CHECK (scope_type IN (
                                    'account', 'project', 'task', 'agent_chat'
                                )),
    scope_id               TEXT NOT NULL,
    visibility             TEXT NOT NULL DEFAULT 'project'
                                CHECK (visibility IN (
                                    'private', 'chat', 'project', 'account'
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
    publication_source_id  TEXT REFERENCES memory_item_new(id) ON DELETE SET NULL,
    supersedes_id          TEXT REFERENCES memory_item_new(id) ON DELETE SET NULL,
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

INSERT INTO memory_item_new (
    row_id, id, project_id, task_id, execution_id, room_id,
    scope_type, scope_id, visibility, owner_identity_id, authority,
    sensitivity, retention_priority, provenance_json, publication_source_id,
    supersedes_id, valid_from, valid_until, source_event_id,
    source_scope_type, source_scope_id, source_revision, source_room_sequence,
    source_type, kind, title, summary, body, metadata_json, confidence,
    quality_score, created_by_type, created_by_id, created_at
)
SELECT
    memory.row_id,
    memory.id,
    memory.project_id,
    memory.task_id,
    memory.execution_id,
    memory.room_id,
    CASE WHEN memory.scope_type = 'room' THEN 'agent_chat' ELSE memory.scope_type END,
    CASE
        WHEN memory.scope_type = 'room' THEN COALESCE(
            (
                SELECT source.chat_id
                FROM agent_chat_source_ref AS source
                WHERE source.source_type = 'room'
                  AND source.source_id = COALESCE(memory.room_id, memory.scope_id)
                ORDER BY source.created_at ASC, source.chat_id ASC
                LIMIT 1
            ),
            memory.scope_id
        )
        ELSE memory.scope_id
    END,
    CASE
        WHEN memory.visibility = 'participants' AND memory.scope_type = 'room' THEN 'chat'
        WHEN memory.visibility = 'participants' THEN 'project'
        ELSE memory.visibility
    END,
    memory.owner_identity_id,
    memory.authority,
    memory.sensitivity,
    memory.retention_priority,
    memory.provenance_json,
    memory.publication_source_id,
    memory.supersedes_id,
    memory.valid_from,
    memory.valid_until,
    memory.source_event_id,
    COALESCE(memory.source_scope_type,
             CASE WHEN memory.scope_type = 'room' THEN 'room' END),
    COALESCE(memory.source_scope_id,
             CASE WHEN memory.scope_type = 'room' THEN COALESCE(memory.room_id, memory.scope_id) END),
    memory.source_revision,
    memory.source_room_sequence,
    memory.source_type,
    memory.kind,
    memory.title,
    memory.summary,
    memory.body,
    memory.metadata_json,
    memory.confidence,
    memory.quality_score,
    memory.created_by_type,
    memory.created_by_id,
    memory.created_at
FROM memory_item AS memory;

DROP TABLE memory_item;
ALTER TABLE memory_item_new RENAME TO memory_item;

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

CREATE TRIGGER memory_item_immutable_update
BEFORE UPDATE ON memory_item
BEGIN
    SELECT RAISE(ABORT, 'memory items are append-only');
END;

-- Several durable ledgers intentionally retain pre-V071 rows whose recorded
-- scope was a Room. Those rows are immutable migration provenance, not a live
-- authority surface. Reject any attempt to create or retarget runtime records
-- to that retired scope while allowing the historical rows to remain linked.
CREATE TRIGGER domain_event_reject_legacy_room_insert
BEFORE INSERT ON domain_event
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER domain_event_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id ON domain_event
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_wake_lease_reject_legacy_room_insert
BEFORE INSERT ON agent_wake_lease
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_wake_lease_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id ON agent_wake_lease
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_wake_budget_window_reject_legacy_room_insert
BEFORE INSERT ON agent_wake_budget_window
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_wake_budget_window_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id ON agent_wake_budget_window
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_context_scope_reject_legacy_room_insert
BEFORE INSERT ON agent_context_scope
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_context_scope_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id, room_id ON agent_context_scope
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_lcm_timeline_reject_legacy_room_insert
BEFORE INSERT ON agent_lcm_timeline
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_lcm_timeline_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id ON agent_lcm_timeline
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER forge_memory_source_binding_reject_legacy_room_insert
BEFORE INSERT ON forge_memory_source_binding
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER forge_memory_source_binding_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id, room_id ON forge_memory_source_binding
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER context_manifest_reject_legacy_room_insert
BEFORE INSERT ON context_manifest
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER context_manifest_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id ON context_manifest
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_commitment_reject_legacy_room_insert
BEFORE INSERT ON agent_commitment
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_commitment_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id ON agent_commitment
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_inbox_item_reject_legacy_room_insert
BEFORE INSERT ON agent_inbox_item
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_inbox_item_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id ON agent_inbox_item
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_question_reject_legacy_room_insert
BEFORE INSERT ON agent_question
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_question_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id ON agent_question
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_action_reject_legacy_room_insert
BEFORE INSERT ON agent_action
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

CREATE TRIGGER agent_action_reject_legacy_room_update
BEFORE UPDATE OF scope_type, scope_id ON agent_action
WHEN NEW.scope_type = 'room'
BEGIN
    SELECT RAISE(ABORT, 'Room scopes are retired; use an Agent Chat scope');
END;

PRAGMA foreign_keys = ON;
