-- Preserve handed-off Genesis rows when a Project or its chat is deleted.
-- Their lifecycle and transcript are historical; the Project and handoff FKs
-- are independently nullable and must be allowed to clear without rewriting
-- that history. V118 continues to fence all new Genesis writes.
PRAGMA foreign_keys = OFF;
BEGIN;
-- These Charter guards reference the table being rebuilt. The update guard
-- is superseded by the exact PR11 fence below; restore only the insert guard.
DROP TRIGGER project_charter_owner_guard_insert;
DROP TRIGGER project_charter_owner_guard_update;
CREATE TABLE product_genesis_session_v119 (
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
    updated_at                          TEXT NOT NULL, charter_id TEXT REFERENCES project_charter(id) ON DELETE SET NULL, charter_revision_id TEXT REFERENCES project_charter_revision(id) ON DELETE SET NULL, charter_approval_id TEXT REFERENCES project_charter_approval(id) ON DELETE SET NULL, charter_version INTEGER NOT NULL DEFAULT 0
    CHECK (charter_version >= 0),
    CHECK (json_valid(source_message_ids_json))
);
INSERT INTO product_genesis_session_v119 ("id", "account_id", "main_chat_id", "prompt_revision", "prompt_body", "maturity", "initial_idea", "lifecycle", "source_message_ids_json", "preferred_project_agent_identity_id", "project_id", "handoff_id", "failure_reason", "version", "created_at", "updated_at", "charter_id", "charter_revision_id", "charter_approval_id", "charter_version") SELECT "id", "account_id", "main_chat_id", "prompt_revision", "prompt_body", "maturity", "initial_idea", "lifecycle", "source_message_ids_json", "preferred_project_agent_identity_id", "project_id", "handoff_id", "failure_reason", "version", "created_at", "updated_at", "charter_id", "charter_revision_id", "charter_approval_id", "charter_version" FROM product_genesis_session;
DROP TABLE product_genesis_session;
ALTER TABLE product_genesis_session_v119 RENAME TO product_genesis_session;
CREATE INDEX idx_product_genesis_account_history
    ON product_genesis_session(account_id, created_at DESC, id DESC);
CREATE UNIQUE INDEX idx_product_genesis_active_account
    ON product_genesis_session(account_id)
    WHERE lifecycle IN ('discovering', 'ready_for_project');
CREATE UNIQUE INDEX idx_product_genesis_active_chat
    ON product_genesis_session(main_chat_id)
    WHERE lifecycle IN ('discovering', 'ready_for_project');
CREATE INDEX idx_product_genesis_chat_history
    ON product_genesis_session(main_chat_id, created_at DESC, id DESC);
CREATE INDEX idx_product_genesis_project
    ON product_genesis_session(project_id, created_at DESC, id DESC);
CREATE TRIGGER pr11_retired_insert_product_genesis_session
BEFORE INSERT ON product_genesis_session BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Product Genesis');
END;
CREATE TRIGGER product_genesis_charter_scope_guard
BEFORE UPDATE OF charter_id, charter_revision_id, charter_approval_id ON product_genesis_session
BEGIN
    SELECT CASE
        WHEN NEW.charter_id IS NOT NULL
         AND NOT EXISTS (
             SELECT 1 FROM project_charter
             WHERE id = NEW.charter_id AND genesis_session_id = NEW.id AND account_id = NEW.account_id
         ) THEN RAISE(ABORT, 'Genesis Charter must belong to Genesis session')
        WHEN NEW.charter_revision_id IS NOT NULL
         AND NOT EXISTS (
             SELECT 1 FROM project_charter_revision
             WHERE id = NEW.charter_revision_id AND charter_id = NEW.charter_id
         ) THEN RAISE(ABORT, 'Genesis Charter revision must belong to Charter')
        WHEN NEW.charter_approval_id IS NOT NULL
         AND NOT EXISTS (
             SELECT 1 FROM project_charter_approval
             WHERE id = NEW.charter_approval_id AND charter_id = NEW.charter_id
         ) THEN RAISE(ABORT, 'Genesis Charter approval must belong to Charter')
    END;
END;
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
CREATE TRIGGER product_genesis_prompt_immutable_update
BEFORE UPDATE OF prompt_revision, prompt_body ON product_genesis_session
WHEN OLD.prompt_revision != NEW.prompt_revision OR OLD.prompt_body != NEW.prompt_body
BEGIN
    SELECT RAISE(ABORT, 'Product Genesis prompt revisions are immutable');
END;
CREATE TRIGGER project_charter_owner_guard_insert
BEFORE INSERT ON project_charter
BEGIN
    SELECT CASE
        WHEN NEW.genesis_session_id IS NOT NULL
         AND NOT EXISTS (
             SELECT 1 FROM product_genesis_session
             WHERE id = NEW.genesis_session_id AND account_id = NEW.account_id
         ) THEN RAISE(ABORT, 'Charter Genesis owner must belong to account')
        WHEN NEW.project_id IS NOT NULL
         AND NOT EXISTS (
             SELECT 1 FROM project p
             WHERE p.id = NEW.project_id
               AND (
                   p.owner_id = NEW.account_id
                   OR EXISTS (
                       SELECT 1 FROM project_member member
                       WHERE member.project_id = p.id
                         AND member.user_id = NEW.account_id
                         AND member.role IN ('owner', 'admin')
                   )
               )
         ) THEN RAISE(ABORT, 'Charter Project owner does not belong to account')
    END;
END;
COMMIT;
PRAGMA foreign_keys = ON;

-- Plan PR11 FK maintenance correction. Historical rows remain semantically
-- immutable. A protected row may change only when SQLite applies an exact
-- ON DELETE SET NULL action: one or more approved FK columns transition
-- from a value to NULL, each referenced parent is already absent, and every
-- other column is byte-for-byte/value-for-value unchanged. This does not
-- consult project_deletion_guard and cannot authorize semantic UPDATEs.

-- These older guards reject mechanically maintained FK columns. V119
-- replaces their update protection with the stricter PR11 row-value fence
-- below; their semantic immutability remains enforced for every other
-- column. Other scope and immutable-delete guards remain in place.
DROP TRIGGER IF EXISTS "agent_chat_message_immutable_update";
DROP TRIGGER IF EXISTS "agent_handoff_immutable_update";
DROP TRIGGER IF EXISTS "context_manifest_immutable_update";
DROP TRIGGER IF EXISTS "memory_item_immutable_update";
DROP TRIGGER IF EXISTS "memory_lifecycle_assertion_immutable_update";
DROP TRIGGER IF EXISTS "project_charter_owner_guard_update";
DROP TRIGGER IF EXISTS "project_charter_approval_lifecycle_guard";
DROP TRIGGER IF EXISTS "project_charter_revision_immutable_update";
DROP TRIGGER IF EXISTS "project_release_media_pin_immutable_update";
DROP TRIGGER IF EXISTS "project_task_governance_immutable_update";

-- account_main_agent_binding: exact cleanup only for replaced_by_binding_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_account_main_agent_binding";
CREATE TRIGGER "pr11_retired_update_account_main_agent_binding"
BEFORE UPDATE ON "account_main_agent_binding"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."account_id" IS NEW."account_id"
    AND OLD."identity_id" IS NEW."identity_id"
    AND OLD."profile_id" IS NEW."profile_id"
    AND OLD."state" IS NEW."state"
    AND OLD."autonomy_policy_json" IS NEW."autonomy_policy_json"
    AND OLD."tool_policy_revision" IS NEW."tool_policy_revision"
    AND OLD."version" IS NEW."version"
    AND OLD."replacement_reason" IS NEW."replacement_reason"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND (OLD."replaced_by_binding_id" IS NEW."replaced_by_binding_id" OR (OLD."replaced_by_binding_id" IS NOT NULL AND NEW."replaced_by_binding_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "account_main_agent_binding" WHERE "account_main_agent_binding"."id" = OLD."replaced_by_binding_id")))
    AND (OLD."replaced_by_binding_id" IS NOT NEW."replaced_by_binding_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Main Agent binding');
END;

-- agent_chat_message: exact cleanup only for handoff_id, profile_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_chat_message";
CREATE TRIGGER "pr11_retired_update_agent_chat_message"
BEFORE UPDATE ON "agent_chat_message"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."chat_id" IS NEW."chat_id"
    AND OLD."sequence" IS NEW."sequence"
    AND OLD."author_type" IS NEW."author_type"
    AND OLD."author_id" IS NEW."author_id"
    AND OLD."content" IS NEW."content"
    AND OLD."content_guard_json" IS NEW."content_guard_json"
    AND OLD."sensitivity" IS NEW."sensitivity"
    AND OLD."status" IS NEW."status"
    AND OLD."outcome" IS NEW."outcome"
    AND OLD."model" IS NEW."model"
    AND OLD."session_id" IS NEW."session_id"
    AND OLD."context_manifest_id" IS NEW."context_manifest_id"
    AND OLD."token_usage_json" IS NEW."token_usage_json"
    AND OLD."duration_ms" IS NEW."duration_ms"
    AND OLD."error" IS NEW."error"
    AND OLD."correlation_id" IS NEW."correlation_id"
    AND OLD."causation_id" IS NEW."causation_id"
    AND OLD."source_type" IS NEW."source_type"
    AND OLD."source_id" IS NEW."source_id"
    AND OLD."source_message_id" IS NEW."source_message_id"
    AND OLD."source_room_id" IS NEW."source_room_id"
    AND OLD."source_conversation_id" IS NEW."source_conversation_id"
    AND OLD."source_sequence" IS NEW."source_sequence"
    AND OLD."source_metadata_json" IS NEW."source_metadata_json"
    AND OLD."created_at" IS NEW."created_at"
    AND (OLD."handoff_id" IS NEW."handoff_id" OR (OLD."handoff_id" IS NOT NULL AND NEW."handoff_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_handoff" WHERE "agent_handoff"."id" = OLD."handoff_id")))
    AND (OLD."profile_id" IS NEW."profile_id" OR (OLD."profile_id" IS NOT NULL AND NEW."profile_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_profile" WHERE "agent_profile"."id" = OLD."profile_id")))
    AND (OLD."handoff_id" IS NOT NEW."handoff_id" OR OLD."profile_id" IS NOT NEW."profile_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat');
END;

-- agent_chat_turn_job: exact cleanup only for profile_id, responder_identity_id, response_message_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_chat_turn_job";
CREATE TRIGGER "pr11_retired_update_agent_chat_turn_job"
BEFORE UPDATE ON "agent_chat_turn_job"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."chat_id" IS NEW."chat_id"
    AND OLD."triggering_message_id" IS NEW."triggering_message_id"
    AND OLD."canonical_scope_type" IS NEW."canonical_scope_type"
    AND OLD."canonical_scope_id" IS NEW."canonical_scope_id"
    AND OLD."status" IS NEW."status"
    AND OLD."dedupe_key" IS NEW."dedupe_key"
    AND OLD."lease_owner" IS NEW."lease_owner"
    AND OLD."leased_until" IS NEW."leased_until"
    AND OLD."attempt_count" IS NEW."attempt_count"
    AND OLD."max_attempts" IS NEW."max_attempts"
    AND OLD."next_attempt_at" IS NEW."next_attempt_at"
    AND OLD."error_code" IS NEW."error_code"
    AND OLD."error_message" IS NEW."error_message"
    AND OLD."correlation_id" IS NEW."correlation_id"
    AND OLD."causation_id" IS NEW."causation_id"
    AND OLD."causation_depth" IS NEW."causation_depth"
    AND OLD."version" IS NEW."version"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND (OLD."response_message_id" IS NEW."response_message_id" OR (OLD."response_message_id" IS NOT NULL AND NEW."response_message_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_chat_message" WHERE "agent_chat_message"."id" = OLD."response_message_id")))
    AND (OLD."profile_id" IS NEW."profile_id" OR (OLD."profile_id" IS NOT NULL AND NEW."profile_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_profile" WHERE "agent_profile"."id" = OLD."profile_id")))
    AND (OLD."responder_identity_id" IS NEW."responder_identity_id" OR (OLD."responder_identity_id" IS NOT NULL AND NEW."responder_identity_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_identity" WHERE "agent_identity"."id" = OLD."responder_identity_id")))
    AND (OLD."profile_id" IS NOT NEW."profile_id" OR OLD."responder_identity_id" IS NOT NEW."responder_identity_id" OR OLD."response_message_id" IS NOT NEW."response_message_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat turn');
END;

-- agent_commitment: exact cleanup only for originating_task_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_commitment";
CREATE TRIGGER "pr11_retired_update_agent_commitment"
BEFORE UPDATE ON "agent_commitment"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."owner_identity_id" IS NEW."owner_identity_id"
    AND OLD."scope_type" IS NEW."scope_type"
    AND OLD."scope_id" IS NEW."scope_id"
    AND OLD."title" IS NEW."title"
    AND OLD."description" IS NEW."description"
    AND OLD."status" IS NEW."status"
    AND OLD."due_at" IS NEW."due_at"
    AND OLD."correlation_id" IS NEW."correlation_id"
    AND OLD."originating_action_id" IS NEW."originating_action_id"
    AND OLD."evidence_required" IS NEW."evidence_required"
    AND OLD."cancellation_reason" IS NEW."cancellation_reason"
    AND OLD."blocked_reason" IS NEW."blocked_reason"
    AND OLD."completed_at" IS NEW."completed_at"
    AND OLD."cancelled_at" IS NEW."cancelled_at"
    AND OLD."version" IS NEW."version"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND (OLD."originating_task_id" IS NEW."originating_task_id" OR (OLD."originating_task_id" IS NOT NULL AND NEW."originating_task_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "task" WHERE "task"."id" = OLD."originating_task_id")))
    AND (OLD."originating_task_id" IS NOT NEW."originating_task_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent commitment');
END;

-- agent_handoff: exact cleanup only for author_identity_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_handoff";
CREATE TRIGGER "pr11_retired_update_agent_handoff"
BEFORE UPDATE ON "agent_handoff"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."source_chat_id" IS NEW."source_chat_id"
    AND OLD."target_chat_id" IS NEW."target_chat_id"
    AND OLD."source_message_id" IS NEW."source_message_id"
    AND OLD."source_turn_job_id" IS NEW."source_turn_job_id"
    AND OLD."target_message_id" IS NEW."target_message_id"
    AND OLD."target_turn_job_id" IS NEW."target_turn_job_id"
    AND OLD."content" IS NEW."content"
    AND OLD."content_guard_json" IS NEW."content_guard_json"
    AND OLD."source_revisions_json" IS NEW."source_revisions_json"
    AND OLD."status" IS NEW."status"
    AND OLD."error_code" IS NEW."error_code"
    AND OLD."correlation_id" IS NEW."correlation_id"
    AND OLD."causation_id" IS NEW."causation_id"
    AND OLD."dedupe_key" IS NEW."dedupe_key"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND (OLD."author_identity_id" IS NEW."author_identity_id" OR (OLD."author_identity_id" IS NOT NULL AND NEW."author_identity_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_identity" WHERE "agent_identity"."id" = OLD."author_identity_id")))
    AND (OLD."author_identity_id" IS NOT NEW."author_identity_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Main-to-Project handoff');
END;

-- agent_question: exact cleanup only for inbox_item_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_question";
CREATE TRIGGER "pr11_retired_update_agent_question"
BEFORE UPDATE ON "agent_question"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."recipient_identity_id" IS NEW."recipient_identity_id"
    AND OLD."scope_type" IS NEW."scope_type"
    AND OLD."scope_id" IS NEW."scope_id"
    AND OLD."status" IS NEW."status"
    AND OLD."question" IS NEW."question"
    AND OLD."context_json" IS NEW."context_json"
    AND OLD."answer" IS NEW."answer"
    AND OLD."asked_by_type" IS NEW."asked_by_type"
    AND OLD."asked_by_id" IS NEW."asked_by_id"
    AND OLD."answered_by_type" IS NEW."answered_by_type"
    AND OLD."answered_by_id" IS NEW."answered_by_id"
    AND OLD."due_at" IS NEW."due_at"
    AND OLD."correlation_id" IS NEW."correlation_id"
    AND OLD."version" IS NEW."version"
    AND OLD."answered_at" IS NEW."answered_at"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND (OLD."inbox_item_id" IS NEW."inbox_item_id" OR (OLD."inbox_item_id" IS NOT NULL AND NEW."inbox_item_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_inbox_item" WHERE "agent_inbox_item"."id" = OLD."inbox_item_id")))
    AND (OLD."inbox_item_id" IS NOT NEW."inbox_item_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent question');
END;

-- attention_projection: exact cleanup only for updated_by_user_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_attention_projection";
CREATE TRIGGER "pr11_retired_update_attention_projection"
BEFORE UPDATE ON "attention_projection"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."attention_type" IS NEW."attention_type"
    AND OLD."scope_type" IS NEW."scope_type"
    AND OLD."scope_id" IS NEW."scope_id"
    AND OLD."identity_id" IS NEW."identity_id"
    AND OLD."source_event_id" IS NEW."source_event_id"
    AND OLD."priority" IS NEW."priority"
    AND OLD."status" IS NEW."status"
    AND OLD."summary" IS NEW."summary"
    AND OLD."details_json" IS NEW."details_json"
    AND OLD."dedupe_key" IS NEW."dedupe_key"
    AND OLD."occurred_at" IS NEW."occurred_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND OLD."version" IS NEW."version"
    AND OLD."acknowledged_at" IS NEW."acknowledged_at"
    AND OLD."snoozed_until" IS NEW."snoozed_until"
    AND OLD."resolved_at" IS NEW."resolved_at"
    AND OLD."recommended_action" IS NEW."recommended_action"
    AND OLD."source_sequence" IS NEW."source_sequence"
    AND (OLD."updated_by_user_id" IS NEW."updated_by_user_id" OR (OLD."updated_by_user_id" IS NOT NULL AND NEW."updated_by_user_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "user" WHERE "user"."id" = OLD."updated_by_user_id")))
    AND (OLD."updated_by_user_id" IS NOT NEW."updated_by_user_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Attention projection');
END;

-- context_manifest: exact cleanup only for agent_session_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_context_manifest";
CREATE TRIGGER "pr11_retired_update_context_manifest"
BEFORE UPDATE ON "context_manifest"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."identity_id" IS NEW."identity_id"
    AND OLD."context_scope_id" IS NEW."context_scope_id"
    AND OLD."scope_type" IS NEW."scope_type"
    AND OLD."scope_id" IS NEW."scope_id"
    AND OLD."policy_revision" IS NEW."policy_revision"
    AND OLD."domain_revision" IS NEW."domain_revision"
    AND OLD."lcm_binding_revision" IS NEW."lcm_binding_revision"
    AND OLD."runtime_manifest_id" IS NEW."runtime_manifest_id"
    AND OLD."runtime_manifest_fingerprint" IS NEW."runtime_manifest_fingerprint"
    AND OLD."combined_fingerprint" IS NEW."combined_fingerprint"
    AND OLD."request_fingerprint" IS NEW."request_fingerprint"
    AND OLD."created_at" IS NEW."created_at"
    AND (OLD."agent_session_id" IS NEW."agent_session_id" OR (OLD."agent_session_id" IS NOT NULL AND NEW."agent_session_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_session" WHERE "agent_session"."id" = OLD."agent_session_id")))
    AND (OLD."agent_session_id" IS NOT NEW."agent_session_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;

-- memory_item: exact cleanup only for execution_id, owner_identity_id, publication_source_id, room_id, source_event_id, supersedes_id, task_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_memory_item";
CREATE TRIGGER "pr11_retired_update_memory_item"
BEFORE UPDATE ON "memory_item"
WHEN NOT (
    OLD."row_id" IS NEW."row_id"
    AND OLD."id" IS NEW."id"
    AND OLD."project_id" IS NEW."project_id"
    AND OLD."scope_type" IS NEW."scope_type"
    AND OLD."scope_id" IS NEW."scope_id"
    AND OLD."visibility" IS NEW."visibility"
    AND OLD."authority" IS NEW."authority"
    AND OLD."sensitivity" IS NEW."sensitivity"
    AND OLD."retention_priority" IS NEW."retention_priority"
    AND OLD."provenance_json" IS NEW."provenance_json"
    AND OLD."valid_from" IS NEW."valid_from"
    AND OLD."valid_until" IS NEW."valid_until"
    AND OLD."source_scope_type" IS NEW."source_scope_type"
    AND OLD."source_scope_id" IS NEW."source_scope_id"
    AND OLD."source_revision" IS NEW."source_revision"
    AND OLD."source_room_sequence" IS NEW."source_room_sequence"
    AND OLD."source_type" IS NEW."source_type"
    AND OLD."kind" IS NEW."kind"
    AND OLD."title" IS NEW."title"
    AND OLD."summary" IS NEW."summary"
    AND OLD."body" IS NEW."body"
    AND OLD."metadata_json" IS NEW."metadata_json"
    AND OLD."confidence" IS NEW."confidence"
    AND OLD."quality_score" IS NEW."quality_score"
    AND OLD."created_by_type" IS NEW."created_by_type"
    AND OLD."created_by_id" IS NEW."created_by_id"
    AND OLD."created_at" IS NEW."created_at"
    AND (OLD."source_event_id" IS NEW."source_event_id" OR (OLD."source_event_id" IS NOT NULL AND NEW."source_event_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "domain_event" WHERE "domain_event"."id" = OLD."source_event_id")))
    AND (OLD."supersedes_id" IS NEW."supersedes_id" OR (OLD."supersedes_id" IS NOT NULL AND NEW."supersedes_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "memory_item" WHERE "memory_item"."id" = OLD."supersedes_id")))
    AND (OLD."publication_source_id" IS NEW."publication_source_id" OR (OLD."publication_source_id" IS NOT NULL AND NEW."publication_source_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "memory_item" WHERE "memory_item"."id" = OLD."publication_source_id")))
    AND (OLD."owner_identity_id" IS NEW."owner_identity_id" OR (OLD."owner_identity_id" IS NOT NULL AND NEW."owner_identity_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_identity" WHERE "agent_identity"."id" = OLD."owner_identity_id")))
    AND (OLD."room_id" IS NEW."room_id" OR (OLD."room_id" IS NOT NULL AND NEW."room_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "legacy_room" WHERE "legacy_room"."id" = OLD."room_id")))
    AND (OLD."execution_id" IS NEW."execution_id" OR (OLD."execution_id" IS NOT NULL AND NEW."execution_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "execution" WHERE "execution"."id" = OLD."execution_id")))
    AND (OLD."task_id" IS NEW."task_id" OR (OLD."task_id" IS NOT NULL AND NEW."task_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "task" WHERE "task"."id" = OLD."task_id")))
    AND (OLD."execution_id" IS NOT NEW."execution_id" OR OLD."owner_identity_id" IS NOT NEW."owner_identity_id" OR OLD."publication_source_id" IS NOT NEW."publication_source_id" OR OLD."room_id" IS NOT NEW."room_id" OR OLD."source_event_id" IS NOT NEW."source_event_id" OR OLD."supersedes_id" IS NOT NEW."supersedes_id" OR OLD."task_id" IS NOT NEW."task_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;

-- memory_lifecycle_assertion: exact cleanup only for related_memory_id, source_event_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_memory_lifecycle_assertion";
CREATE TRIGGER "pr11_retired_update_memory_lifecycle_assertion"
BEFORE UPDATE ON "memory_lifecycle_assertion"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."memory_item_id" IS NEW."memory_item_id"
    AND OLD."assertion_type" IS NEW."assertion_type"
    AND OLD."reason" IS NEW."reason"
    AND OLD."evidence_json" IS NEW."evidence_json"
    AND OLD."asserted_by_type" IS NEW."asserted_by_type"
    AND OLD."asserted_by_id" IS NEW."asserted_by_id"
    AND OLD."created_at" IS NEW."created_at"
    AND (OLD."source_event_id" IS NEW."source_event_id" OR (OLD."source_event_id" IS NOT NULL AND NEW."source_event_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "domain_event" WHERE "domain_event"."id" = OLD."source_event_id")))
    AND (OLD."related_memory_id" IS NEW."related_memory_id" OR (OLD."related_memory_id" IS NOT NULL AND NEW."related_memory_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "memory_item" WHERE "memory_item"."id" = OLD."related_memory_id")))
    AND (OLD."related_memory_id" IS NOT NEW."related_memory_id" OR OLD."source_event_id" IS NOT NEW."source_event_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;

-- product_genesis_session: exact cleanup only for charter_approval_id, charter_id, charter_revision_id, handoff_id, preferred_project_agent_identity_id, project_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_product_genesis_session";
CREATE TRIGGER "pr11_retired_update_product_genesis_session"
BEFORE UPDATE ON "product_genesis_session"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."account_id" IS NEW."account_id"
    AND OLD."main_chat_id" IS NEW."main_chat_id"
    AND OLD."prompt_revision" IS NEW."prompt_revision"
    AND OLD."prompt_body" IS NEW."prompt_body"
    AND OLD."maturity" IS NEW."maturity"
    AND OLD."initial_idea" IS NEW."initial_idea"
    AND OLD."lifecycle" IS NEW."lifecycle"
    AND OLD."source_message_ids_json" IS NEW."source_message_ids_json"
    AND OLD."failure_reason" IS NEW."failure_reason"
    AND OLD."version" IS NEW."version"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND OLD."charter_version" IS NEW."charter_version"
    AND (OLD."charter_approval_id" IS NEW."charter_approval_id" OR (OLD."charter_approval_id" IS NOT NULL AND NEW."charter_approval_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "project_charter_approval" WHERE "project_charter_approval"."id" = OLD."charter_approval_id")))
    AND (OLD."charter_revision_id" IS NEW."charter_revision_id" OR (OLD."charter_revision_id" IS NOT NULL AND NEW."charter_revision_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "project_charter_revision" WHERE "project_charter_revision"."id" = OLD."charter_revision_id")))
    AND (OLD."charter_id" IS NEW."charter_id" OR (OLD."charter_id" IS NOT NULL AND NEW."charter_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "project_charter" WHERE "project_charter"."id" = OLD."charter_id")))
    AND (OLD."handoff_id" IS NEW."handoff_id" OR (OLD."handoff_id" IS NOT NULL AND NEW."handoff_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_handoff" WHERE "agent_handoff"."id" = OLD."handoff_id")))
    AND (OLD."project_id" IS NEW."project_id" OR (OLD."project_id" IS NOT NULL AND NEW."project_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "project" WHERE "project"."id" = OLD."project_id")))
    AND (OLD."preferred_project_agent_identity_id" IS NEW."preferred_project_agent_identity_id" OR (OLD."preferred_project_agent_identity_id" IS NOT NULL AND NEW."preferred_project_agent_identity_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_identity" WHERE "agent_identity"."id" = OLD."preferred_project_agent_identity_id")))
    AND (OLD."charter_approval_id" IS NOT NEW."charter_approval_id" OR OLD."charter_id" IS NOT NEW."charter_id" OR OLD."charter_revision_id" IS NOT NEW."charter_revision_id" OR OLD."handoff_id" IS NOT NEW."handoff_id" OR OLD."preferred_project_agent_identity_id" IS NOT NEW."preferred_project_agent_identity_id" OR OLD."project_id" IS NOT NEW."project_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Product Genesis');
END;

-- project_agent_binding: exact cleanup only for charter_id, charter_revision_id, replaced_by_binding_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_project_agent_binding";
CREATE TRIGGER "pr11_retired_update_project_agent_binding"
BEFORE UPDATE ON "project_agent_binding"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."project_id" IS NEW."project_id"
    AND OLD."identity_id" IS NEW."identity_id"
    AND OLD."profile_id" IS NEW."profile_id"
    AND OLD."state" IS NEW."state"
    AND OLD."autonomy_policy_json" IS NEW."autonomy_policy_json"
    AND OLD."permission_ceiling_json" IS NEW."permission_ceiling_json"
    AND OLD."subscriptions_json" IS NEW."subscriptions_json"
    AND OLD."wake_budget" IS NEW."wake_budget"
    AND OLD."version" IS NEW."version"
    AND OLD."replacement_reason" IS NEW."replacement_reason"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND OLD."operating_skill_revision_id" IS NEW."operating_skill_revision_id"
    AND OLD."policy_revision" IS NEW."policy_revision"
    AND OLD."policy_digest" IS NEW."policy_digest"
    AND OLD."charter_setup_required" IS NEW."charter_setup_required"
    AND (OLD."replaced_by_binding_id" IS NEW."replaced_by_binding_id" OR (OLD."replaced_by_binding_id" IS NOT NULL AND NEW."replaced_by_binding_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "project_agent_binding" WHERE "project_agent_binding"."id" = OLD."replaced_by_binding_id")))
    AND (OLD."charter_revision_id" IS NEW."charter_revision_id" OR (OLD."charter_revision_id" IS NOT NULL AND NEW."charter_revision_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "project_charter_revision" WHERE "project_charter_revision"."id" = OLD."charter_revision_id")))
    AND (OLD."charter_id" IS NEW."charter_id" OR (OLD."charter_id" IS NOT NULL AND NEW."charter_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "project_charter" WHERE "project_charter"."id" = OLD."charter_id")))
    AND (OLD."charter_id" IS NOT NEW."charter_id" OR OLD."charter_revision_id" IS NOT NEW."charter_revision_id" OR OLD."replaced_by_binding_id" IS NOT NEW."replaced_by_binding_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Agent binding');
END;

-- project_charter: exact cleanup only for genesis_session_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_project_charter";
CREATE TRIGGER "pr11_retired_update_project_charter"
BEFORE UPDATE ON "project_charter"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."account_id" IS NEW."account_id"
    AND OLD."project_id" IS NEW."project_id"
    AND OLD."current_draft_revision_id" IS NEW."current_draft_revision_id"
    AND OLD."current_approved_revision_id" IS NEW."current_approved_revision_id"
    AND OLD."project_mode" IS NEW."project_mode"
    AND OLD."maturity" IS NEW."maturity"
    AND OLD."lifecycle" IS NEW."lifecycle"
    AND OLD."version" IS NEW."version"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND (OLD."genesis_session_id" IS NEW."genesis_session_id" OR (OLD."genesis_session_id" IS NOT NULL AND NEW."genesis_session_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "product_genesis_session" WHERE "product_genesis_session"."id" = OLD."genesis_session_id")))
    AND (OLD."genesis_session_id" IS NOT NEW."genesis_session_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;

-- project_charter_amendment: exact cleanup only for approval_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_project_charter_amendment";
CREATE TRIGGER "pr11_retired_update_project_charter_amendment"
BEFORE UPDATE ON "project_charter_amendment"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."project_id" IS NEW."project_id"
    AND OLD."base_charter_revision_id" IS NEW."base_charter_revision_id"
    AND OLD."candidate_revision_id" IS NEW."candidate_revision_id"
    AND OLD."lifecycle" IS NEW."lifecycle"
    AND OLD."rationale" IS NEW."rationale"
    AND OLD."material_diff_json" IS NEW."material_diff_json"
    AND OLD."affected_records_json" IS NEW."affected_records_json"
    AND OLD."requested_principal_type" IS NEW."requested_principal_type"
    AND OLD."requested_principal_id" IS NEW."requested_principal_id"
    AND OLD."expected_project_version" IS NEW."expected_project_version"
    AND OLD."version" IS NEW."version"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND (OLD."approval_id" IS NEW."approval_id" OR (OLD."approval_id" IS NOT NULL AND NEW."approval_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "project_charter_approval" WHERE "project_charter_approval"."id" = OLD."approval_id")))
    AND (OLD."approval_id" IS NOT NEW."approval_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;

-- project_charter_approval: exact cleanup only for consumed_project_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_project_charter_approval";
CREATE TRIGGER "pr11_retired_update_project_charter_approval"
BEFORE UPDATE ON "project_charter_approval"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."approval_type" IS NEW."approval_type"
    AND OLD."charter_id" IS NEW."charter_id"
    AND OLD."revision_id" IS NEW."revision_id"
    AND OLD."content_digest" IS NEW."content_digest"
    AND OLD."rendered_digest" IS NEW."rendered_digest"
    AND OLD."expected_charter_version" IS NEW."expected_charter_version"
    AND OLD."approved_name" IS NEW."approved_name"
    AND OLD."approved_slug" IS NEW."approved_slug"
    AND OLD."selected_identity_id" IS NEW."selected_identity_id"
    AND OLD."selected_profile_id" IS NEW."selected_profile_id"
    AND OLD."selected_operating_skill_revision_id" IS NEW."selected_operating_skill_revision_id"
    AND OLD."selected_policy_revision" IS NEW."selected_policy_revision"
    AND OLD."selected_policy_digest" IS NEW."selected_policy_digest"
    AND OLD."approving_principal_type" IS NEW."approving_principal_type"
    AND OLD."approving_principal_id" IS NEW."approving_principal_id"
    AND OLD."authorization_basis" IS NEW."authorization_basis"
    AND OLD."authorization_action" IS NEW."authorization_action"
    AND OLD."explicit_event" IS NEW."explicit_event"
    AND OLD."authorization_occurred_at" IS NEW."authorization_occurred_at"
    AND OLD."source_action" IS NEW."source_action"
    AND OLD."lifecycle" IS NEW."lifecycle"
    AND OLD."idempotency_key" IS NEW."idempotency_key"
    AND OLD."consumed_at" IS NEW."consumed_at"
    AND OLD."version" IS NEW."version"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND OLD."approved_project_mode" IS NEW."approved_project_mode"
    AND OLD."approval_event_id" IS NEW."approval_event_id"
    AND (OLD."consumed_project_id" IS NEW."consumed_project_id" OR (OLD."consumed_project_id" IS NOT NULL AND NEW."consumed_project_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "project" WHERE "project"."id" = OLD."consumed_project_id")))
    AND (OLD."consumed_project_id" IS NOT NEW."consumed_project_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;

-- project_charter_revision: exact cleanup only for source_message_id, source_turn_job_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_project_charter_revision";
CREATE TRIGGER "pr11_retired_update_project_charter_revision"
BEFORE UPDATE ON "project_charter_revision"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."charter_id" IS NEW."charter_id"
    AND OLD."revision" IS NEW."revision"
    AND OLD."base_revision" IS NEW."base_revision"
    AND OLD."base_revision_id" IS NEW."base_revision_id"
    AND OLD."lifecycle" IS NEW."lifecycle"
    AND OLD."schema_version" IS NEW."schema_version"
    AND OLD."render_version" IS NEW."render_version"
    AND OLD."content_json" IS NEW."content_json"
    AND OLD."rendered_view" IS NEW."rendered_view"
    AND OLD."change_summary" IS NEW."change_summary"
    AND OLD."author_type" IS NEW."author_type"
    AND OLD."author_id" IS NEW."author_id"
    AND OLD."source_refs_json" IS NEW."source_refs_json"
    AND OLD."content_digest" IS NEW."content_digest"
    AND OLD."rendered_digest" IS NEW."rendered_digest"
    AND OLD."created_at" IS NEW."created_at"
    AND (OLD."source_turn_job_id" IS NEW."source_turn_job_id" OR (OLD."source_turn_job_id" IS NOT NULL AND NEW."source_turn_job_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_chat_turn_job" WHERE "agent_chat_turn_job"."id" = OLD."source_turn_job_id")))
    AND (OLD."source_message_id" IS NEW."source_message_id" OR (OLD."source_message_id" IS NOT NULL AND NEW."source_message_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "agent_chat_message" WHERE "agent_chat_message"."id" = OLD."source_message_id")))
    AND (OLD."source_message_id" IS NOT NEW."source_message_id" OR OLD."source_turn_job_id" IS NOT NEW."source_turn_job_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;

-- project_release_media_pin: exact cleanup only for legacy_task_media_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_project_release_media_pin";
CREATE TRIGGER "pr11_retired_update_project_release_media_pin"
BEFORE UPDATE ON "project_release_media_pin"
WHEN NOT (
    OLD."id" IS NEW."id"
    AND OLD."project_id" IS NEW."project_id"
    AND OLD."release_id" IS NEW."release_id"
    AND OLD."asset_id" IS NEW."asset_id"
    AND OLD."attachment_id" IS NEW."attachment_id"
    AND OLD."asset_checksum" IS NEW."asset_checksum"
    AND OLD."attachment_digest" IS NEW."attachment_digest"
    AND OLD."availability" IS NEW."availability"
    AND OLD."pin_digest" IS NEW."pin_digest"
    AND OLD."created_at" IS NEW."created_at"
    AND (OLD."legacy_task_media_id" IS NEW."legacy_task_media_id" OR (OLD."legacy_task_media_id" IS NOT NULL AND NEW."legacy_task_media_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "task_media" WHERE "task_media"."id" = OLD."legacy_task_media_id")))
    AND (OLD."legacy_task_media_id" IS NOT NEW."legacy_task_media_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Release media pin');
END;

-- project_task_governance: exact cleanup only for replacement_of_task_id.
DROP TRIGGER IF EXISTS "pr11_retired_update_project_task_governance";
CREATE TRIGGER "pr11_retired_update_project_task_governance"
BEFORE UPDATE ON "project_task_governance"
WHEN NOT (
    OLD."task_id" IS NEW."task_id"
    AND OLD."project_id" IS NEW."project_id"
    AND OLD."charter_revision_id" IS NEW."charter_revision_id"
    AND OLD."baseline_id" IS NEW."baseline_id"
    AND OLD."baseline_revision_id" IS NEW."baseline_revision_id"
    AND OLD."plan_item_id" IS NEW."plan_item_id"
    AND OLD."milestone_id" IS NEW."milestone_id"
    AND OLD."document_revisions_json" IS NEW."document_revisions_json"
    AND OLD."capability_class" IS NEW."capability_class"
    AND OLD."risk_class" IS NEW."risk_class"
    AND OLD."runnable" IS NEW."runnable"
    AND OLD."provenance_json" IS NEW."provenance_json"
    AND OLD."version" IS NEW."version"
    AND OLD."created_at" IS NEW."created_at"
    AND OLD."updated_at" IS NEW."updated_at"
    AND (OLD."replacement_of_task_id" IS NEW."replacement_of_task_id" OR (OLD."replacement_of_task_id" IS NOT NULL AND NEW."replacement_of_task_id" IS NULL AND NOT EXISTS (SELECT 1 FROM "task" WHERE "task"."id" = OLD."replacement_of_task_id")))
    AND (OLD."replacement_of_task_id" IS NOT NEW."replacement_of_task_id")
)
BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Task Governance');
END;

-- Project Chat history is removed only as part of the existing guarded
-- Project cascade. Keep direct deletion of immutable handoffs, receipts,
-- messages, and instructions prohibited, while permitting ProjectRepo to
-- remove them before their chats cascade away.
DROP TRIGGER IF EXISTS "agent_handoff_immutable_delete";
CREATE TRIGGER "agent_handoff_immutable_delete"
BEFORE DELETE ON "agent_handoff"
WHEN NOT EXISTS (
    SELECT 1
    FROM "agent_chat" AS chat
    JOIN "project_deletion_guard" AS guard ON guard."project_id" = chat."project_id"
    WHERE chat."kind" = 'project'
      AND (chat."id" = OLD."source_chat_id" OR chat."id" = OLD."target_chat_id")
)
BEGIN
    SELECT RAISE(ABORT, 'Agent handoffs are immutable outside Project teardown');
END;

DROP TRIGGER IF EXISTS "agent_handoff_delivery_immutable_delete";
CREATE TRIGGER "agent_handoff_delivery_immutable_delete"
BEFORE DELETE ON "agent_handoff_delivery"
WHEN NOT EXISTS (
    SELECT 1
    FROM "agent_handoff" AS handoff
    JOIN "agent_chat" AS chat
      ON chat."id" = handoff."source_chat_id" OR chat."id" = handoff."target_chat_id"
    JOIN "project_deletion_guard" AS guard ON guard."project_id" = chat."project_id"
    WHERE chat."kind" = 'project' AND handoff."id" = OLD."handoff_id"
)
BEGIN
    SELECT RAISE(ABORT, 'Handoff delivery receipts are immutable outside Project teardown');
END;

DROP TRIGGER IF EXISTS "agent_chat_instruction_immutable_delete";
CREATE TRIGGER "agent_chat_instruction_immutable_delete"
BEFORE DELETE ON "agent_chat_instruction_revision"
WHEN NOT EXISTS (
    SELECT 1
    FROM "agent_chat" AS chat
    JOIN "project_deletion_guard" AS guard ON guard."project_id" = chat."project_id"
    WHERE chat."kind" = 'project' AND chat."id" = OLD."chat_id"
)
BEGIN
    SELECT RAISE(ABORT, 'Agent Chat instructions are immutable outside Project teardown');
END;

DROP TRIGGER IF EXISTS "agent_chat_message_immutable_delete";
CREATE TRIGGER "agent_chat_message_immutable_delete"
BEFORE DELETE ON "agent_chat_message"
WHEN NOT EXISTS (
    SELECT 1
    FROM "agent_chat" AS chat
    JOIN "project_deletion_guard" AS guard ON guard."project_id" = chat."project_id"
    WHERE chat."kind" = 'project' AND chat."id" = OLD."chat_id"
)
BEGIN
    SELECT RAISE(ABORT, 'Agent Chat messages are immutable outside Project teardown');
END;
