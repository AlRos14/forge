-- Durable Product Genesis discovery state.  Genesis is scoped to the
-- existing account Main Agent Chat; it never creates another chat/thread.
-- V071 owns the singular chat/binding tables.  This migration only adds the
-- typed discovery lifecycle and deliberately keeps source message references
-- as immutable JSON so a prompt revision can survive chat/session rotation.

CREATE TABLE product_genesis_session (
    id                                  TEXT PRIMARY KEY,
    account_id                          TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    main_chat_id                        TEXT NOT NULL REFERENCES agent_chat(id) ON DELETE CASCADE,
    prompt_revision                     TEXT NOT NULL,
    prompt_body                         TEXT NOT NULL,
    maturity                            TEXT NOT NULL CHECK (maturity IN (
                                            'prototype', 'mvp', 'production', 'critical'
                                        )),
    initial_idea                        TEXT,
    lifecycle                           TEXT NOT NULL DEFAULT 'discovering'
                                            CHECK (lifecycle IN (
                                                'discovering', 'ready_for_project',
                                                'handed_off', 'cancelled'
                                            )),
    source_message_ids_json             TEXT NOT NULL DEFAULT '[]',
    preferred_project_agent_identity_id TEXT REFERENCES agent_identity(id) ON DELETE SET NULL,
    project_id                          TEXT REFERENCES project(id) ON DELETE SET NULL,
    handoff_id                          TEXT REFERENCES agent_handoff(id) ON DELETE SET NULL,
    failure_reason                      TEXT,
    version                             INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at                          TEXT NOT NULL,
    updated_at                          TEXT NOT NULL,
    CHECK (json_valid(source_message_ids_json)),
    CHECK (lifecycle != 'handed_off' OR (project_id IS NOT NULL AND handoff_id IS NOT NULL))
);

CREATE TRIGGER product_genesis_main_chat_guard_insert
BEFORE INSERT ON product_genesis_session
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1 FROM agent_chat
            WHERE agent_chat.id = NEW.main_chat_id
              AND agent_chat.kind = 'account_main'
              AND agent_chat.account_id = NEW.account_id
        ) THEN RAISE(ABORT, 'Product Genesis Main Chat must belong to account')
        WHEN NEW.preferred_project_agent_identity_id IS NOT NULL
         AND NOT EXISTS (
            SELECT 1 FROM agent_identity
            WHERE agent_identity.id = NEW.preferred_project_agent_identity_id
              AND agent_identity.owner_id = NEW.account_id
        ) THEN RAISE(ABORT, 'preferred Project Agent must belong to account')
    END;
END;

CREATE TRIGGER product_genesis_main_chat_guard_update
BEFORE UPDATE OF account_id, main_chat_id, preferred_project_agent_identity_id
ON product_genesis_session
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1 FROM agent_chat
            WHERE agent_chat.id = NEW.main_chat_id
              AND agent_chat.kind = 'account_main'
              AND agent_chat.account_id = NEW.account_id
        ) THEN RAISE(ABORT, 'Product Genesis Main Chat must belong to account')
        WHEN NEW.preferred_project_agent_identity_id IS NOT NULL
         AND NOT EXISTS (
            SELECT 1 FROM agent_identity
            WHERE agent_identity.id = NEW.preferred_project_agent_identity_id
              AND agent_identity.owner_id = NEW.account_id
        ) THEN RAISE(ABORT, 'preferred Project Agent must belong to account')
    END;
END;

-- A Genesis prompt revision is the durable instruction snapshot for this
-- lifecycle. Later state/result updates may advance the lifecycle, but they
-- may not rewrite the prompt that admitted the discovery interaction.
CREATE TRIGGER product_genesis_prompt_immutable_update
BEFORE UPDATE OF prompt_revision, prompt_body ON product_genesis_session
WHEN OLD.prompt_revision != NEW.prompt_revision OR OLD.prompt_body != NEW.prompt_body
BEGIN
    SELECT RAISE(ABORT, 'Product Genesis prompt revisions are immutable');
END;

-- At most one active discovery/proposal exists per account and Main Chat.
-- Terminal rows remain as an immutable audit history and do not block a new
-- discovery session after cancellation or successful handoff.
CREATE UNIQUE INDEX idx_product_genesis_active_account
    ON product_genesis_session(account_id)
    WHERE lifecycle IN ('discovering', 'ready_for_project');
CREATE UNIQUE INDEX idx_product_genesis_active_chat
    ON product_genesis_session(main_chat_id)
    WHERE lifecycle IN ('discovering', 'ready_for_project');
CREATE INDEX idx_product_genesis_account_history
    ON product_genesis_session(account_id, created_at DESC, id DESC);
CREATE INDEX idx_product_genesis_chat_history
    ON product_genesis_session(main_chat_id, created_at DESC, id DESC);
CREATE INDEX idx_product_genesis_project
    ON product_genesis_session(project_id, created_at DESC, id DESC);
