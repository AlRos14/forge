-- Singular Main/Project Agent Chats and their explicit bindings.
--
-- V059 through V070 are intentionally left untouched.  This migration keeps
-- the pre-release Room tables as a source of historical data while copying
-- their durable transcript into the owner-owned, singular chat ledger.  Room
-- rows remain available to the verification/rollback tooling until the
-- service/API migration removes that compatibility surface.

CREATE TABLE account_main_agent_binding (
    id                         TEXT PRIMARY KEY,
    account_id                 TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    identity_id                TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE RESTRICT,
    profile_id                 TEXT NOT NULL REFERENCES agent_profile(id) ON DELETE RESTRICT,
    state                      TEXT NOT NULL DEFAULT 'active'
                                   CHECK (state IN ('active', 'replaced', 'paused', 'revoked')),
    autonomy_policy_json       TEXT NOT NULL DEFAULT '{}',
    tool_policy_revision       TEXT NOT NULL DEFAULT 'default',
    version                    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    replaced_by_binding_id     TEXT,
    replacement_reason         TEXT,
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL,
    FOREIGN KEY (replaced_by_binding_id)
        REFERENCES account_main_agent_binding(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX idx_account_main_binding_active
    ON account_main_agent_binding(account_id)
    WHERE state = 'active';
CREATE INDEX idx_account_main_binding_history
    ON account_main_agent_binding(account_id, created_at ASC, id ASC);
CREATE INDEX idx_account_main_binding_identity
    ON account_main_agent_binding(identity_id, state, created_at DESC);

CREATE TRIGGER account_main_binding_identity_profile_guard_insert
BEFORE INSERT ON account_main_agent_binding
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_profile
            WHERE agent_profile.id = NEW.profile_id
              AND agent_profile.identity_id = NEW.identity_id
        ) THEN RAISE(ABORT, 'Main binding profile must belong to identity')
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_identity
            WHERE agent_identity.id = NEW.identity_id
              AND agent_identity.owner_id = NEW.account_id
        ) THEN RAISE(ABORT, 'Main binding identity must belong to account')
    END;
END;

CREATE TRIGGER account_main_binding_identity_profile_guard_update
BEFORE UPDATE OF account_id, identity_id, profile_id ON account_main_agent_binding
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_profile
            WHERE agent_profile.id = NEW.profile_id
              AND agent_profile.identity_id = NEW.identity_id
        ) THEN RAISE(ABORT, 'Main binding profile must belong to identity')
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_identity
            WHERE agent_identity.id = NEW.identity_id
              AND agent_identity.owner_id = NEW.account_id
        ) THEN RAISE(ABORT, 'Main binding identity must belong to account')
    END;
END;

CREATE TABLE project_agent_binding (
    id                         TEXT PRIMARY KEY,
    project_id                 TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    identity_id                TEXT REFERENCES agent_identity(id) ON DELETE RESTRICT,
    profile_id                 TEXT REFERENCES agent_profile(id) ON DELETE RESTRICT,
    state                      TEXT NOT NULL DEFAULT 'active'
                                   CHECK (state IN (
                                       'active', 'agent_setup_required', 'replaced',
                                       'paused', 'revoked'
                                   )),
    autonomy_policy_json       TEXT NOT NULL DEFAULT '{}',
    permission_ceiling_json    TEXT NOT NULL DEFAULT '{}',
    subscriptions_json         TEXT NOT NULL DEFAULT '[]',
    wake_budget                INTEGER NOT NULL DEFAULT 0 CHECK (wake_budget >= 0),
    version                    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    replaced_by_binding_id     TEXT,
    replacement_reason         TEXT,
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL,
    FOREIGN KEY (replaced_by_binding_id)
        REFERENCES project_agent_binding(id) ON DELETE SET NULL,
    CHECK (
        (state = 'agent_setup_required' AND identity_id IS NULL AND profile_id IS NULL)
        OR
        (state = 'replaced' AND (
            (identity_id IS NULL AND profile_id IS NULL)
            OR (identity_id IS NOT NULL AND profile_id IS NOT NULL)
        ))
        OR
        (state IN ('active', 'paused', 'revoked')
            AND identity_id IS NOT NULL AND profile_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX idx_project_agent_binding_active
    ON project_agent_binding(project_id)
    WHERE state IN ('active', 'agent_setup_required');
CREATE INDEX idx_project_agent_binding_history
    ON project_agent_binding(project_id, created_at ASC, id ASC);
CREATE INDEX idx_project_agent_binding_identity
    ON project_agent_binding(identity_id, state, created_at DESC);

CREATE TRIGGER project_binding_identity_profile_guard_insert
BEFORE INSERT ON project_agent_binding
WHEN NEW.state IN ('active', 'paused', 'revoked')
  OR (NEW.state = 'replaced' AND NEW.identity_id IS NOT NULL)
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_profile
            WHERE agent_profile.id = NEW.profile_id
              AND agent_profile.identity_id = NEW.identity_id
        ) THEN RAISE(ABORT, 'Project binding profile must belong to identity')
    END;
END;

CREATE TRIGGER project_binding_identity_profile_guard_update
BEFORE UPDATE OF identity_id, profile_id, state ON project_agent_binding
WHEN NEW.state IN ('active', 'paused', 'revoked')
  OR (NEW.state = 'replaced' AND NEW.identity_id IS NOT NULL)
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_profile
            WHERE agent_profile.id = NEW.profile_id
              AND agent_profile.identity_id = NEW.identity_id
        ) THEN RAISE(ABORT, 'Project binding profile must belong to identity')
    END;
END;

CREATE TABLE agent_chat (
    id                         TEXT PRIMARY KEY,
    kind                       TEXT NOT NULL CHECK (kind IN ('account_main', 'project')),
    account_id                 TEXT REFERENCES user(id) ON DELETE CASCADE,
    project_id                 TEXT REFERENCES project(id) ON DELETE CASCADE,
    status                     TEXT NOT NULL DEFAULT 'ready'
                                   CHECK (status IN ('ready', 'agent_setup_required', 'archived')),
    instruction_revision       INTEGER NOT NULL DEFAULT 0 CHECK (instruction_revision >= 0),
    message_count              INTEGER NOT NULL DEFAULT 0 CHECK (message_count >= 0),
    last_message_at            TEXT,
    version                    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL,
    CHECK (
        (kind = 'account_main' AND account_id IS NOT NULL AND project_id IS NULL)
        OR
        (kind = 'project' AND project_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX idx_agent_chat_account_main
    ON agent_chat(account_id)
    WHERE kind = 'account_main';
CREATE UNIQUE INDEX idx_agent_chat_project
    ON agent_chat(project_id)
    WHERE kind = 'project';
CREATE INDEX idx_agent_chat_account_status
    ON agent_chat(account_id, status, updated_at DESC, id DESC);
CREATE INDEX idx_agent_chat_project_status
    ON agent_chat(project_id, status, updated_at DESC, id DESC);

-- One source mapping lets later Room/LCM/memory/context cleanup prove that no
-- historical row was orphaned while keeping the old source tables intact.
CREATE TABLE agent_chat_source_ref (
    chat_id                    TEXT NOT NULL REFERENCES agent_chat(id) ON DELETE CASCADE,
    source_type                TEXT NOT NULL
                                   CHECK (source_type IN (
                                       'room', 'conversation', 'memory_item',
                                       'lcm_timeline', 'context_manifest'
                                   )),
    source_id                  TEXT NOT NULL,
    source_scope_type          TEXT,
    source_scope_id            TEXT,
    source_revision            TEXT,
    created_at                 TEXT NOT NULL,
    PRIMARY KEY (chat_id, source_type, source_id)
);

CREATE INDEX idx_agent_chat_source_ref_source
    ON agent_chat_source_ref(source_type, source_id);

CREATE TABLE agent_chat_instruction_revision (
    id                         TEXT PRIMARY KEY,
    chat_id                    TEXT NOT NULL REFERENCES agent_chat(id) ON DELETE CASCADE,
    source_type                TEXT NOT NULL DEFAULT 'native'
                                   CHECK (source_type IN ('native', 'room', 'conversation', 'migration')),
    source_id                  TEXT,
    revision                   INTEGER NOT NULL CHECK (revision >= 1),
    body                       TEXT NOT NULL,
    content_guard_json         TEXT NOT NULL DEFAULT '{}',
    sensitivity                TEXT NOT NULL DEFAULT 'internal'
                                   CHECK (sensitivity IN ('public', 'internal', 'restricted')),
    created_by_type            TEXT NOT NULL,
    created_by_id              TEXT,
    created_at                 TEXT NOT NULL,
    UNIQUE (chat_id, revision, source_type, source_id)
);

CREATE INDEX idx_agent_chat_instruction_chat
    ON agent_chat_instruction_revision(chat_id, revision DESC, source_type ASC, source_id ASC);

CREATE TRIGGER agent_chat_instruction_immutable_update
BEFORE UPDATE ON agent_chat_instruction_revision
BEGIN
    SELECT RAISE(ABORT, 'Agent Chat instructions are immutable');
END;

CREATE TRIGGER agent_chat_instruction_immutable_delete
BEFORE DELETE ON agent_chat_instruction_revision
BEGIN
    SELECT RAISE(ABORT, 'Agent Chat instructions are immutable');
END;

CREATE TABLE agent_handoff (
    id                         TEXT PRIMARY KEY,
    source_chat_id             TEXT NOT NULL REFERENCES agent_chat(id) ON DELETE CASCADE,
    target_chat_id             TEXT NOT NULL REFERENCES agent_chat(id) ON DELETE CASCADE,
    source_message_id          TEXT,
    source_turn_job_id         TEXT,
    target_message_id          TEXT,
    target_turn_job_id         TEXT,
    author_identity_id         TEXT REFERENCES agent_identity(id) ON DELETE SET NULL,
    content                    TEXT NOT NULL,
    content_guard_json         TEXT NOT NULL DEFAULT '{}',
    source_revisions_json      TEXT NOT NULL DEFAULT '[]',
    status                     TEXT NOT NULL DEFAULT 'pending'
                                   CHECK (status IN ('pending', 'delivered', 'failed', 'cancelled')),
    error_code                 TEXT,
    correlation_id             TEXT NOT NULL,
    causation_id               TEXT,
    dedupe_key                 TEXT NOT NULL UNIQUE,
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL,
    CHECK (source_chat_id != target_chat_id),
    CHECK (error_code IS NULL OR length(error_code) <= 128)
);

CREATE INDEX idx_agent_handoff_target
    ON agent_handoff(target_chat_id, created_at ASC, id ASC);
CREATE INDEX idx_agent_handoff_source
    ON agent_handoff(source_chat_id, created_at ASC, id ASC);

CREATE TRIGGER agent_handoff_immutable_update
BEFORE UPDATE ON agent_handoff
BEGIN
    SELECT RAISE(ABORT, 'Agent handoffs are immutable');
END;

CREATE TRIGGER agent_handoff_immutable_delete
BEFORE DELETE ON agent_handoff
BEGIN
    SELECT RAISE(ABORT, 'Agent handoffs are immutable');
END;

-- Delivery/terminal state is an append-only receipt rather than an UPDATE of
-- the immutable handoff publication.  A composite admission can insert a
-- `delivered` receipt with preallocated target IDs; retries use the unique
-- handoff key and never mutate the source publication.
CREATE TABLE agent_handoff_delivery (
    handoff_id                 TEXT NOT NULL REFERENCES agent_handoff(id) ON DELETE CASCADE,
    delivery_sequence          INTEGER NOT NULL DEFAULT 1 CHECK (delivery_sequence >= 1),
    status                     TEXT NOT NULL
                                   CHECK (status IN ('delivered', 'failed', 'cancelled')),
    target_message_id          TEXT,
    target_turn_job_id         TEXT,
    error_code                 TEXT,
    error_message              TEXT,
    created_at                 TEXT NOT NULL,
    PRIMARY KEY (handoff_id, delivery_sequence),
    CHECK (error_code IS NULL OR length(error_code) <= 128),
    CHECK (error_message IS NULL OR length(error_message) <= 2048)
);

CREATE INDEX idx_agent_handoff_delivery_latest
    ON agent_handoff_delivery(handoff_id, delivery_sequence DESC);

CREATE TRIGGER agent_handoff_delivery_immutable_update
BEFORE UPDATE ON agent_handoff_delivery
BEGIN
    SELECT RAISE(ABORT, 'Handoff delivery receipts are immutable');
END;

CREATE TRIGGER agent_handoff_delivery_immutable_delete
BEFORE DELETE ON agent_handoff_delivery
BEGIN
    SELECT RAISE(ABORT, 'Handoff delivery receipts are immutable');
END;

CREATE TABLE agent_chat_message (
    id                         TEXT PRIMARY KEY,
    chat_id                    TEXT NOT NULL REFERENCES agent_chat(id) ON DELETE CASCADE,
    sequence                   INTEGER NOT NULL CHECK (sequence >= 0),
    author_type                TEXT NOT NULL CHECK (author_type IN ('user', 'agent', 'system', 'handoff')),
    author_id                  TEXT,
    content                    TEXT NOT NULL,
    content_guard_json         TEXT NOT NULL DEFAULT '{}',
    sensitivity                TEXT NOT NULL DEFAULT 'internal'
                                   CHECK (sensitivity IN ('public', 'internal', 'restricted')),
    status                     TEXT NOT NULL CHECK (status IN ('complete', 'failed', 'cancelled')),
    outcome                    TEXT,
    model                      TEXT,
    profile_id                 TEXT REFERENCES agent_profile(id) ON DELETE SET NULL,
    session_id                TEXT,
    context_manifest_id        TEXT,
    token_usage_json           TEXT,
    duration_ms                INTEGER,
    error                      TEXT,
    correlation_id             TEXT NOT NULL,
    causation_id               TEXT,
    handoff_id                 TEXT REFERENCES agent_handoff(id) ON DELETE SET NULL,
    source_type                TEXT NOT NULL DEFAULT 'native'
                                   CHECK (source_type IN ('native', 'room', 'conversation', 'handoff')),
    source_id                  TEXT,
    source_message_id          TEXT,
    source_room_id             TEXT,
    source_conversation_id     TEXT,
    source_sequence            INTEGER,
    source_metadata_json       TEXT NOT NULL DEFAULT '{}',
    created_at                 TEXT NOT NULL,
    UNIQUE (chat_id, sequence),
    CHECK (error IS NULL OR length(error) <= 2048),
    CHECK (source_sequence IS NULL OR source_sequence >= 0)
);

CREATE INDEX idx_agent_chat_message_chat_sequence
    ON agent_chat_message(chat_id, sequence ASC);
CREATE INDEX idx_agent_chat_message_source
    ON agent_chat_message(source_type, source_id, source_sequence);
CREATE INDEX idx_agent_chat_message_handoff
    ON agent_chat_message(handoff_id);

CREATE TRIGGER agent_chat_message_immutable_update
BEFORE UPDATE ON agent_chat_message
BEGIN
    SELECT RAISE(ABORT, 'Agent Chat messages are immutable');
END;

CREATE TRIGGER agent_chat_message_immutable_delete
BEFORE DELETE ON agent_chat_message
BEGIN
    SELECT RAISE(ABORT, 'Agent Chat messages are immutable');
END;

CREATE TABLE agent_chat_turn_job (
    id                         TEXT PRIMARY KEY,
    chat_id                    TEXT NOT NULL REFERENCES agent_chat(id) ON DELETE CASCADE,
    triggering_message_id      TEXT NOT NULL REFERENCES agent_chat_message(id) ON DELETE CASCADE,
    responder_identity_id      TEXT REFERENCES agent_identity(id) ON DELETE SET NULL,
    profile_id                 TEXT REFERENCES agent_profile(id) ON DELETE SET NULL,
    canonical_scope_type       TEXT NOT NULL CHECK (canonical_scope_type = 'agent_chat'),
    canonical_scope_id         TEXT NOT NULL,
    status                     TEXT NOT NULL DEFAULT 'queued'
                                   CHECK (status IN (
                                       'queued', 'leased', 'retry_wait',
                                       'succeeded', 'failed', 'cancelled'
                                   )),
    dedupe_key                 TEXT NOT NULL UNIQUE,
    lease_owner                TEXT,
    leased_until               TEXT,
    attempt_count              INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    max_attempts               INTEGER NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 16),
    next_attempt_at            TEXT,
    response_message_id        TEXT REFERENCES agent_chat_message(id) ON DELETE SET NULL,
    error_code                 TEXT,
    error_message              TEXT,
    correlation_id             TEXT NOT NULL,
    causation_id               TEXT,
    causation_depth            INTEGER NOT NULL DEFAULT 0 CHECK (causation_depth BETWEEN 0 AND 16),
    version                    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL,
    UNIQUE (chat_id, triggering_message_id),
    CHECK (canonical_scope_id = chat_id),
    CHECK (error_code IS NULL OR length(error_code) <= 128),
    CHECK (error_message IS NULL OR length(error_message) <= 2048)
);

CREATE INDEX idx_agent_chat_turn_dispatch
    ON agent_chat_turn_job(status, next_attempt_at, created_at, id);
CREATE INDEX idx_agent_chat_turn_chat
    ON agent_chat_turn_job(chat_id, created_at ASC, id ASC);
CREATE UNIQUE INDEX idx_agent_chat_turn_active_lease
    ON agent_chat_turn_job(chat_id)
    WHERE status = 'leased';

-- The old Room worker may have left `running`/`leased` rows behind.  New
-- repositories only accept this finite state vocabulary, and the migration
-- below explicitly maps old jobs rather than carrying a silent lease over.

-- One global chat exists for every authenticated account.  It starts in setup
-- state until exactly one safe account-owned identity is selected below.
INSERT INTO agent_chat (
    id, kind, account_id, project_id, status, created_at, updated_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    'account_main',
    id,
    NULL,
    'agent_setup_required',
    COALESCE(created_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    COALESCE(updated_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
FROM user;

-- Every existing Project gets one owner-owned timeline.  A Project with no
-- safely inferable binding remains visible but explicitly setup-required.
INSERT INTO agent_chat (
    id, kind, account_id, project_id, status, created_at, updated_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    'project',
    CASE WHEN EXISTS (
        SELECT 1 FROM user AS project_owner WHERE project_owner.id = project.owner_id
    ) THEN project.owner_id ELSE NULL END,
    project.id,
    'agent_setup_required',
    project.created_at,
    project.updated_at
FROM project;

-- A Main binding is inferred only when one and only one active account-owned
-- identity has a selected profile.  No default/primary role is consulted.
INSERT INTO account_main_agent_binding (
    id, account_id, identity_id, profile_id, state, version, created_at, updated_at
)
WITH candidates AS (
    SELECT
        identity.owner_id AS account_id,
        identity.id AS identity_id,
        identity.selected_profile_id AS profile_id,
        identity.created_at,
        identity.id AS ordering_id
    FROM agent_identity AS identity
    JOIN user ON user.id = identity.owner_id
    WHERE identity.owner_id IS NOT NULL
      AND identity.selected_profile_id IS NOT NULL
      AND identity.archived_at IS NULL
      AND identity.paused = 0
      AND identity.status != 'offline'
), chosen AS (
    SELECT account_id, MIN(identity_id) AS identity_id, MIN(profile_id) AS profile_id,
           MIN(created_at) AS created_at
    FROM candidates
    GROUP BY account_id
    HAVING COUNT(*) = 1
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    account_id,
    identity_id,
    profile_id,
    'active',
    1,
    created_at,
    created_at
FROM chosen;

-- Project candidates come only from an active Steward or an explicit Room
-- responder.  A primary Worker is deliberately excluded.  UNION removes
-- duplicate evidence for the same identity; two distinct candidates leave
-- the Project in setup-required state.
INSERT INTO project_agent_binding (
    id, project_id, identity_id, profile_id, state, version, created_at, updated_at
)
WITH eligible AS (
    SELECT identity.id AS identity_id, identity.selected_profile_id AS profile_id
    FROM agent_identity AS identity
    WHERE identity.selected_profile_id IS NOT NULL
      AND identity.archived_at IS NULL
      AND identity.paused = 0
      AND identity.status != 'offline'
), steward_candidates AS (
    SELECT DISTINCT membership.project_id, membership.identity_id, eligible.profile_id
    FROM project_agent_membership AS membership
    JOIN eligible ON eligible.identity_id = membership.identity_id
    WHERE membership.state = 'active'
      AND membership.role = 'steward'
      AND NOT EXISTS (
          SELECT 1
          FROM project_agent_membership AS worker
          WHERE worker.project_id = membership.project_id
            AND worker.identity_id = membership.identity_id
            AND worker.state = 'active'
            AND worker.role = 'worker'
            AND worker.is_primary = 1
      )
), explicit_candidates AS (
    SELECT DISTINCT room.scope_id AS project_id,
                    room.default_responder_identity_id AS identity_id,
                    eligible.profile_id
    FROM room
    JOIN eligible ON eligible.identity_id = room.default_responder_identity_id
    WHERE room.scope_type = 'project'
      AND room.responder_policy = 'explicit_identity'
      AND room.default_responder_identity_id IS NOT NULL
      AND NOT EXISTS (
          SELECT 1
          FROM project_agent_membership AS worker
          WHERE worker.project_id = room.scope_id
            AND worker.identity_id = room.default_responder_identity_id
            AND worker.state = 'active'
            AND worker.role = 'worker'
            AND worker.is_primary = 1
      )
), candidates AS (
    SELECT project_id, identity_id, profile_id FROM steward_candidates
    UNION
    SELECT project_id, identity_id, profile_id FROM explicit_candidates
), chosen AS (
    SELECT project_id,
           MIN(identity_id) AS identity_id,
           MIN(profile_id) AS profile_id,
           COUNT(*) AS candidate_count
    FROM candidates
    GROUP BY project_id
), projects AS (
    SELECT project.id, project.created_at, project.updated_at,
           chosen.identity_id, chosen.profile_id,
           CASE WHEN chosen.candidate_count = 1 THEN 'active'
                ELSE 'agent_setup_required' END AS binding_state
    FROM project
    LEFT JOIN chosen ON chosen.project_id = project.id
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    id,
    CASE WHEN binding_state = 'active' THEN identity_id ELSE NULL END,
    CASE WHEN binding_state = 'active' THEN profile_id ELSE NULL END,
    binding_state,
    1,
    created_at,
    updated_at
FROM projects;

UPDATE agent_chat
SET status = 'ready', version = version + 1
WHERE kind = 'account_main'
  AND EXISTS (
      SELECT 1 FROM account_main_agent_binding AS binding
      WHERE binding.account_id = agent_chat.account_id
        AND binding.state = 'active'
  );

UPDATE agent_chat
SET status = 'ready', version = version + 1
WHERE kind = 'project'
  AND EXISTS (
      SELECT 1 FROM project_agent_binding AS binding
      WHERE binding.project_id = agent_chat.project_id
        AND binding.state = 'active'
  );

-- Preserve source thread boundaries for Room-derived LCM, semantic memory,
-- and context-manifest rows.  The rows themselves remain in their old scope
-- tables until the service migration can atomically switch readers.
INSERT OR IGNORE INTO agent_chat_source_ref (
    chat_id, source_type, source_id, source_scope_type, source_scope_id,
    source_revision, created_at
)
SELECT chat.id, 'room', room.id, room.scope_type, room.scope_id,
       CAST(room.version AS TEXT), room.created_at
FROM room
JOIN agent_chat AS chat
  ON chat.kind = CASE WHEN room.scope_type = 'account' THEN 'account_main' ELSE 'project' END
 AND (
      (room.scope_type = 'account' AND chat.account_id = room.scope_id)
      OR (room.scope_type = 'project' AND chat.project_id = room.scope_id)
 );

INSERT OR IGNORE INTO agent_chat_source_ref (
    chat_id, source_type, source_id, source_scope_type, source_scope_id,
    source_revision, created_at
)
SELECT source.chat_id, 'memory_item', memory.id, memory.scope_type,
       memory.scope_id, memory.source_revision, memory.created_at
FROM memory_item AS memory
JOIN agent_chat_source_ref AS source
  ON source.source_type = 'room'
 AND source.source_id = memory.room_id
WHERE memory.room_id IS NOT NULL;

INSERT OR IGNORE INTO agent_chat_source_ref (
    chat_id, source_type, source_id, source_scope_type, source_scope_id,
    source_revision, created_at
)
SELECT source.chat_id, 'lcm_timeline', timeline.id, timeline.scope_type,
       timeline.scope_id, CAST(timeline.revision AS TEXT), timeline.created_at
FROM agent_lcm_timeline AS timeline
JOIN agent_chat_source_ref AS source
  ON source.source_type = 'room'
 AND source.source_id = timeline.scope_id
WHERE timeline.scope_type = 'room';

INSERT OR IGNORE INTO agent_chat_source_ref (
    chat_id, source_type, source_id, source_scope_type, source_scope_id,
    source_revision, created_at
)
SELECT source.chat_id, 'context_manifest', manifest.id, manifest.scope_type,
       manifest.scope_id, manifest.domain_revision, manifest.created_at
FROM context_manifest AS manifest
JOIN agent_chat_source_ref AS source
  ON source.source_type = 'room'
 AND source.source_id = manifest.scope_id
WHERE manifest.scope_type = 'room';

INSERT OR IGNORE INTO agent_chat_instruction_revision (
    id, chat_id, source_type, source_id, revision, body, content_guard_json,
    sensitivity, created_by_type, created_by_id, created_at
)
SELECT instruction.id, source.chat_id, 'room', instruction.room_id, instruction.revision,
       instruction.body, instruction.content_guard_json, instruction.sensitivity,
       instruction.created_by_type, instruction.created_by_id, instruction.created_at
FROM room_instruction_revision AS instruction
JOIN agent_chat_source_ref AS source
  ON source.source_type = 'room'
 AND source.source_id = instruction.room_id;

-- Merge all Room timelines into the owning singular chat.  Window ordering is
-- deliberately stable across retries: original timestamp, source Room ID,
-- source sequence, then source message ID.  The old sequence is retained
-- separately so the original thread boundary remains inspectable.
INSERT INTO agent_chat_message (
    id, chat_id, sequence, author_type, author_id, content,
    content_guard_json, sensitivity, status, outcome, model, profile_id,
    session_id, context_manifest_id, token_usage_json, duration_ms, error,
    correlation_id, causation_id, source_type, source_id, source_message_id,
    source_room_id, source_conversation_id, source_sequence,
    source_metadata_json, created_at
)
SELECT
    message.id,
    chat.id,
    ROW_NUMBER() OVER (
        PARTITION BY chat.id
        ORDER BY message.created_at ASC, message.room_id ASC,
                 message.sequence ASC, message.id ASC
    ) - 1,
    message.author_type,
    message.author_id,
    CASE
        WHEN lower(message.content) LIKE '%authorization: bearer%'
          OR lower(message.content) LIKE '%api_key%'
          OR lower(message.content) LIKE '%sk-%'
          OR lower(message.content) LIKE '%private key%'
        THEN '[protected value redacted during migration]'
        ELSE message.content
    END,
    CASE
        WHEN lower(message.content) LIKE '%authorization: bearer%'
          OR lower(message.content) LIKE '%api_key%'
          OR lower(message.content) LIKE '%sk-%'
          OR lower(message.content) LIKE '%private key%'
        THEN json_object('migration', 'singular-agent-chat', 'action', 'redacted',
                         'classification', 'legacy_known_secret_pattern',
                         'source_room_id', message.room_id,
                         'source_message_id', message.id)
        ELSE message.content_guard_json
    END,
    CASE
        WHEN lower(message.content) LIKE '%authorization: bearer%'
          OR lower(message.content) LIKE '%api_key%'
          OR lower(message.content) LIKE '%sk-%'
          OR lower(message.content) LIKE '%private key%'
        THEN 'restricted'
        ELSE message.sensitivity
    END,
    message.status,
    message.outcome,
    message.model,
    message.profile_id,
    message.session_id,
    message.context_manifest_id,
    message.token_usage_json,
    message.duration_ms,
    substr(message.error, 1, 2048),
    message.correlation_id,
    NULL,
    'room',
    message.room_id,
    message.id,
    message.room_id,
    NULL,
    message.sequence,
    json_object('source', 'room', 'room_id', message.room_id,
                'room_sequence', message.sequence,
                'reply_to_message_id', message.reply_to_message_id),
    message.created_at
FROM room_message AS message
JOIN room ON room.id = message.room_id
JOIN agent_chat AS chat
  ON chat.kind = CASE WHEN room.scope_type = 'account' THEN 'account_main' ELSE 'project' END
 AND (
      (room.scope_type = 'account' AND chat.account_id = room.scope_id)
      OR (room.scope_type = 'project' AND chat.project_id = room.scope_id)
 );

INSERT INTO content_guard_audit (
    entity_type, entity_id, action, classifier, original_length, created_at
)
SELECT 'agent_chat_message', message.id, 'redacted',
       'legacy_known_secret_pattern', length(message.content), message.created_at
FROM room_message AS message
WHERE lower(message.content) LIKE '%authorization: bearer%'
   OR lower(message.content) LIKE '%api_key%'
   OR lower(message.content) LIKE '%sk-%'
   OR lower(message.content) LIKE '%private key%';

UPDATE agent_chat
SET message_count = (
        SELECT COUNT(*) FROM agent_chat_message AS message
        WHERE message.chat_id = agent_chat.id
    ),
    last_message_at = (
        SELECT MAX(message.created_at) FROM agent_chat_message AS message
        WHERE message.chat_id = agent_chat.id
    )
WHERE EXISTS (
    SELECT 1 FROM agent_chat_message AS message
    WHERE message.chat_id = agent_chat.id
);

-- Copy the old turn ledger after messages so triggering/response references
-- remain valid.  Expired or authority-ambiguous leases become retryable or
-- terminal, never a silent forever-lease.  Historical completion/failure is
-- preserved as a finite terminal state.
INSERT INTO agent_chat_turn_job (
    id, chat_id, triggering_message_id, responder_identity_id, profile_id,
    canonical_scope_type, canonical_scope_id, status, dedupe_key, lease_owner,
    leased_until, attempt_count, max_attempts, next_attempt_at,
    response_message_id, error_code, error_message, correlation_id,
    causation_id, causation_depth, created_at, updated_at
)
SELECT
    job.id,
    chat.id,
    job.input_message_id,
    job.responder_identity_id,
    identity.selected_profile_id,
    'agent_chat',
    chat.id,
    CASE
        WHEN job.status = 'completed' THEN 'succeeded'
        WHEN job.status = 'cancelled' THEN 'cancelled'
        WHEN job.status IN ('failed', 'suppressed') THEN 'failed'
        WHEN COALESCE(main_binding.identity_id, project_binding.identity_id) IS NULL THEN 'failed'
        WHEN job.status IN ('leased', 'running')
             AND (job.leased_until IS NULL OR job.leased_until <= job.updated_at)
             AND MAX(job.attempt_count, 0) >= 3
        THEN 'failed'
        WHEN job.status IN ('leased', 'running')
             AND job.leased_until IS NOT NULL
             AND job.leased_until > job.updated_at
        THEN 'leased'
        WHEN job.status IN ('leased', 'running') THEN 'retry_wait'
        ELSE 'queued'
    END,
    job.dedupe_key,
    CASE
        WHEN job.status IN ('leased', 'running')
         AND job.leased_until IS NOT NULL
         AND job.leased_until > job.updated_at
         AND COALESCE(main_binding.identity_id, project_binding.identity_id) IS NOT NULL
        THEN job.lease_owner ELSE NULL END,
    CASE
        WHEN job.status IN ('leased', 'running')
         AND job.leased_until IS NOT NULL
         AND job.leased_until > job.updated_at
         AND COALESCE(main_binding.identity_id, project_binding.identity_id) IS NOT NULL
        THEN job.leased_until ELSE NULL END,
    MAX(job.attempt_count, 0),
    3,
    CASE
        WHEN job.status IN ('leased', 'running')
         AND (job.leased_until IS NULL OR job.leased_until <= job.updated_at)
         AND MAX(job.attempt_count, 0) >= 3
        THEN NULL
        WHEN job.status IN ('leased', 'running')
        THEN COALESCE(job.leased_until, job.updated_at)
        ELSE NULL
    END,
    job.response_message_id,
    CASE
        WHEN COALESCE(main_binding.identity_id, project_binding.identity_id) IS NULL THEN 'binding_unresolved'
        WHEN job.status IN ('leased', 'running')
         AND (job.leased_until IS NULL OR job.leased_until <= job.updated_at)
         AND MAX(job.attempt_count, 0) >= 3
        THEN 'retry_exhausted'
        WHEN job.status IN ('leased', 'running')
         AND (job.leased_until IS NULL OR job.leased_until <= job.updated_at)
        THEN 'lease_expired_during_migration'
        ELSE NULL
    END,
    CASE
        WHEN COALESCE(main_binding.identity_id, project_binding.identity_id) IS NULL THEN 'Project/Main binding was ambiguous or setup-required during migration'
        WHEN job.status IN ('leased', 'running')
         AND (job.leased_until IS NULL OR job.leased_until <= job.updated_at)
         AND MAX(job.attempt_count, 0) >= 3
        THEN 'Legacy turn retry budget was exhausted during singular-chat migration'
        WHEN job.status IN ('leased', 'running')
         AND (job.leased_until IS NULL OR job.leased_until <= job.updated_at)
        THEN 'Legacy turn lease expired before singular-chat migration'
        ELSE substr(job.error, 1, 2048)
    END,
    job.correlation_id,
    job.causation_id,
    job.causation_depth,
    job.created_at,
    job.updated_at
FROM agent_turn_job AS job
JOIN room ON room.id = job.room_id
JOIN agent_chat AS chat
  ON chat.kind = CASE WHEN room.scope_type = 'account' THEN 'account_main' ELSE 'project' END
 AND (
      (room.scope_type = 'account' AND chat.account_id = room.scope_id)
      OR (room.scope_type = 'project' AND chat.project_id = room.scope_id)
 )
LEFT JOIN agent_identity AS identity ON identity.id = job.responder_identity_id
LEFT JOIN account_main_agent_binding AS main_binding
  ON chat.kind = 'account_main'
 AND main_binding.account_id = chat.account_id
 AND main_binding.state = 'active'
LEFT JOIN project_agent_binding AS project_binding
  ON chat.kind = 'project'
 AND project_binding.project_id = chat.project_id
 AND project_binding.state = 'active'
LEFT JOIN agent_identity AS binding
  ON binding.id = COALESCE(main_binding.identity_id, project_binding.identity_id)
WHERE EXISTS (
    SELECT 1 FROM agent_chat_message AS triggering
    WHERE triggering.id = job.input_message_id
      AND triggering.chat_id = chat.id
)
  AND NOT EXISTS (
      SELECT 1 FROM agent_chat_turn_job AS existing WHERE existing.id = job.id
  );

CREATE INDEX idx_agent_chat_source_ref_chat_type
    ON agent_chat_source_ref(chat_id, source_type, source_scope_id);

-- Keep the one-chat/setup-required invariant true for new users and Projects
-- created through any DB caller, including import/tests that do not go through
-- the Rust repositories.  The AFTER INSERT trigger runs in the caller's
-- transaction, so a failed chat/binding insert rolls back the owner row too.
CREATE TRIGGER user_agent_chat_after_insert
AFTER INSERT ON user
BEGIN
    INSERT INTO agent_chat (
        id, kind, account_id, project_id, status,
        instruction_revision, message_count, version, created_at, updated_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'account_main', NEW.id, NULL, 'agent_setup_required', 0, 0, 1,
        NEW.created_at, NEW.updated_at
    WHERE NOT EXISTS (
        SELECT 1 FROM agent_chat
        WHERE kind = 'account_main' AND account_id = NEW.id
    );
END;

CREATE TRIGGER project_agent_chat_after_insert
AFTER INSERT ON project
BEGIN
    INSERT INTO agent_chat (
        id, kind, account_id, project_id, status,
        instruction_revision, message_count, version, created_at, updated_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'project', NULL, NEW.id, 'agent_setup_required', 0, 0, 1,
        NEW.created_at, NEW.updated_at
    WHERE NOT EXISTS (
        SELECT 1 FROM agent_chat
        WHERE kind = 'project' AND project_id = NEW.id
    );

    INSERT INTO project_agent_binding (
        id, project_id, identity_id, profile_id, state,
        autonomy_policy_json, permission_ceiling_json, subscriptions_json,
        wake_budget, version, created_at, updated_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        NEW.id, NULL, NULL, 'agent_setup_required', '{}', '{}', '[]', 0, 1,
        NEW.created_at, NEW.updated_at
    WHERE NOT EXISTS (
        SELECT 1 FROM project_agent_binding
        WHERE project_id = NEW.id
          AND state IN ('active', 'agent_setup_required')
    );
END;
