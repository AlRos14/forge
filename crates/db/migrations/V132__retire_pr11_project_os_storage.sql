-- PR11 retired verticals remain readable only through PR12. Preserve exact stored
-- source values in the immutable PR13 archive, then remove their tables and
-- Project OS columns. Generic Task/WorkUnit/Execution/Role/Gate/Media and
-- immutable ProjectRelease history remain target storage.
-- Credential usage still reads agent_session_history and protected session
-- state remains encrypted history. Keep context_scope_id as provenance but detach
-- it from the old Agent cognition graph being dropped.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TEMP TABLE _pr13_pr11_expected (source_table TEXT PRIMARY KEY, rows INTEGER NOT NULL);
INSERT INTO _pr13_pr11_expected SELECT 'account_main_agent_binding', COUNT(*) FROM "account_main_agent_binding";
INSERT INTO _pr13_pr11_expected SELECT 'agent_action', COUNT(*) FROM "agent_action";
INSERT INTO _pr13_pr11_expected SELECT 'agent_action_approval', COUNT(*) FROM "agent_action_approval";
INSERT INTO _pr13_pr11_expected SELECT 'agent_action_execution', COUNT(*) FROM "agent_action_execution";
INSERT INTO _pr13_pr11_expected SELECT 'agent_chat', COUNT(*) FROM "agent_chat";
INSERT INTO _pr13_pr11_expected SELECT 'agent_chat_instruction_revision', COUNT(*) FROM "agent_chat_instruction_revision";
INSERT INTO _pr13_pr11_expected SELECT 'agent_chat_message', COUNT(*) FROM "agent_chat_message";
INSERT INTO _pr13_pr11_expected SELECT 'agent_chat_source_ref', COUNT(*) FROM "agent_chat_source_ref";
INSERT INTO _pr13_pr11_expected SELECT 'agent_chat_turn_job', COUNT(*) FROM "agent_chat_turn_job";
INSERT INTO _pr13_pr11_expected SELECT 'agent_commitment', COUNT(*) FROM "agent_commitment";
INSERT INTO _pr13_pr11_expected SELECT 'agent_commitment_evidence', COUNT(*) FROM "agent_commitment_evidence";
INSERT INTO _pr13_pr11_expected SELECT 'agent_commitment_lifecycle', COUNT(*) FROM "agent_commitment_lifecycle";
INSERT INTO _pr13_pr11_expected SELECT 'agent_commitment_transfer', COUNT(*) FROM "agent_commitment_transfer";
INSERT INTO _pr13_pr11_expected SELECT 'agent_connection_health', COUNT(*) FROM "agent_connection_health";
INSERT INTO _pr13_pr11_expected SELECT 'agent_context_scope', COUNT(*) FROM "agent_context_scope";
INSERT INTO _pr13_pr11_expected SELECT 'agent_handoff', COUNT(*) FROM "agent_handoff";
INSERT INTO _pr13_pr11_expected SELECT 'agent_handoff_delivery', COUNT(*) FROM "agent_handoff_delivery";
INSERT INTO _pr13_pr11_expected SELECT 'agent_inbox_item', COUNT(*) FROM "agent_inbox_item";
INSERT INTO _pr13_pr11_expected SELECT 'agent_lcm_entry', COUNT(*) FROM "agent_lcm_entry";
INSERT INTO _pr13_pr11_expected SELECT 'agent_lcm_node', COUNT(*) FROM "agent_lcm_node";
INSERT INTO _pr13_pr11_expected SELECT 'agent_lcm_operation', COUNT(*) FROM "agent_lcm_operation";
INSERT INTO _pr13_pr11_expected SELECT 'agent_lcm_timeline', COUNT(*) FROM "agent_lcm_timeline";
INSERT INTO _pr13_pr11_expected SELECT 'agent_question', COUNT(*) FROM "agent_question";
INSERT INTO _pr13_pr11_expected SELECT 'agent_wake_budget_window', COUNT(*) FROM "agent_wake_budget_window";
INSERT INTO _pr13_pr11_expected SELECT 'agent_wake_lease', COUNT(*) FROM "agent_wake_lease";
INSERT INTO _pr13_pr11_expected SELECT 'attention_consumer_health', COUNT(*) FROM "attention_consumer_health";
INSERT INTO _pr13_pr11_expected SELECT 'attention_projection', COUNT(*) FROM "attention_projection";
INSERT INTO _pr13_pr11_expected SELECT 'context_manifest', COUNT(*) FROM "context_manifest";
INSERT INTO _pr13_pr11_expected SELECT 'context_manifest_source', COUNT(*) FROM "context_manifest_source";
INSERT INTO _pr13_pr11_expected SELECT 'forge_memory_source_binding', COUNT(*) FROM "forge_memory_source_binding";
INSERT INTO _pr13_pr11_expected SELECT 'legacy_agent_turn_job', COUNT(*) FROM "legacy_agent_turn_job";
INSERT INTO _pr13_pr11_expected SELECT 'legacy_bounded_room_round', COUNT(*) FROM "legacy_bounded_room_round";
INSERT INTO _pr13_pr11_expected SELECT 'legacy_bounded_room_round_participant', COUNT(*) FROM "legacy_bounded_room_round_participant";
INSERT INTO _pr13_pr11_expected SELECT 'legacy_room', COUNT(*) FROM "legacy_room";
INSERT INTO _pr13_pr11_expected SELECT 'legacy_room_instruction_revision', COUNT(*) FROM "legacy_room_instruction_revision";
INSERT INTO _pr13_pr11_expected SELECT 'legacy_room_message', COUNT(*) FROM "legacy_room_message";
INSERT INTO _pr13_pr11_expected SELECT 'legacy_room_participant', COUNT(*) FROM "legacy_room_participant";
INSERT INTO _pr13_pr11_expected SELECT 'legacy_project_agent_membership', COUNT(*) FROM "legacy_project_agent_membership";
INSERT INTO _pr13_pr11_expected SELECT 'memory_item', COUNT(*) FROM "memory_item";
INSERT INTO _pr13_pr11_expected SELECT 'memory_item_legacy', COUNT(*) FROM "memory_item_legacy";
INSERT INTO _pr13_pr11_expected SELECT 'memory_lifecycle_assertion', COUNT(*) FROM "memory_lifecycle_assertion";
INSERT INTO _pr13_pr11_expected SELECT 'memory_source_receipt', COUNT(*) FROM "memory_source_receipt";
INSERT INTO _pr13_pr11_expected SELECT 'operating_skill', COUNT(*) FROM "operating_skill";
INSERT INTO _pr13_pr11_expected SELECT 'operating_skill_revision', COUNT(*) FROM "operating_skill_revision";
INSERT INTO _pr13_pr11_expected SELECT 'product_genesis_session', COUNT(*) FROM "product_genesis_session";
INSERT INTO _pr13_pr11_expected SELECT 'project_agent_binding', COUNT(*) FROM "project_agent_binding";
INSERT INTO _pr13_pr11_expected SELECT 'project_canonical_conflict', COUNT(*) FROM "project_canonical_conflict";
INSERT INTO _pr13_pr11_expected SELECT 'project_charter', COUNT(*) FROM "project_charter";
INSERT INTO _pr13_pr11_expected SELECT 'project_charter_amendment', COUNT(*) FROM "project_charter_amendment";
INSERT INTO _pr13_pr11_expected SELECT 'project_charter_approval', COUNT(*) FROM "project_charter_approval";
INSERT INTO _pr13_pr11_expected SELECT 'project_charter_approval_event', COUNT(*) FROM "project_charter_approval_event";
INSERT INTO _pr13_pr11_expected SELECT 'project_charter_revision', COUNT(*) FROM "project_charter_revision";
INSERT INTO _pr13_pr11_expected SELECT 'project_decision', COUNT(*) FROM "project_decision";
INSERT INTO _pr13_pr11_expected SELECT 'project_decision_candidate', COUNT(*) FROM "project_decision_candidate";
INSERT INTO _pr13_pr11_expected SELECT 'project_decision_link', COUNT(*) FROM "project_decision_link";
INSERT INTO _pr13_pr11_expected SELECT 'project_document', COUNT(*) FROM "project_document";
INSERT INTO _pr13_pr11_expected SELECT 'project_document_approval', COUNT(*) FROM "project_document_approval";
INSERT INTO _pr13_pr11_expected SELECT 'project_document_revision', COUNT(*) FROM "project_document_revision";
INSERT INTO _pr13_pr11_expected SELECT 'project_execution_baseline', COUNT(*) FROM "project_execution_baseline";
INSERT INTO _pr13_pr11_expected SELECT 'project_execution_baseline_approval', COUNT(*) FROM "project_execution_baseline_approval";
INSERT INTO _pr13_pr11_expected SELECT 'project_execution_baseline_revision', COUNT(*) FROM "project_execution_baseline_revision";
INSERT INTO _pr13_pr11_expected SELECT 'project_milestone', COUNT(*) FROM "project_milestone";
INSERT INTO _pr13_pr11_expected SELECT 'project_milestone_check', COUNT(*) FROM "project_milestone_check";
INSERT INTO _pr13_pr11_expected SELECT 'project_milestone_check_result', COUNT(*) FROM "project_milestone_check_result";
INSERT INTO _pr13_pr11_expected SELECT 'project_milestone_revision', COUNT(*) FROM "project_milestone_revision";
INSERT INTO _pr13_pr11_expected SELECT 'project_readiness_input', COUNT(*) FROM "project_readiness_input";
INSERT INTO _pr13_pr11_expected SELECT 'project_readiness_snapshot', COUNT(*) FROM "project_readiness_snapshot";
INSERT INTO _pr13_pr11_expected SELECT 'project_reconciliation_record', COUNT(*) FROM "project_reconciliation_record";
INSERT INTO _pr13_pr11_expected SELECT 'project_reconciliation_resolution', COUNT(*) FROM "project_reconciliation_resolution";
INSERT INTO _pr13_pr11_expected SELECT 'project_task_governance', COUNT(*) FROM "project_task_governance";
INSERT INTO _pr13_pr11_expected SELECT 'project_media_attachment', COUNT(*) FROM "project_media_attachment";
INSERT INTO _pr13_pr11_expected SELECT 'protected_legacy_session_ref', COUNT(*) FROM "protected_legacy_session_ref";
INSERT INTO _pr13_pr11_expected SELECT 'project_legacy_columns', COUNT(*) FROM project;
CREATE TEMP TABLE _pr13_pr11_check (ok INTEGER NOT NULL CHECK (ok = 1));

DROP TRIGGER pr13_legacy_record_history_no_insert;
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'account_main_agent_binding', CAST("id" AS TEXT), json_object('id', "id", 'account_id', "account_id", 'identity_id', "identity_id", 'profile_id', "profile_id", 'state', "state", 'autonomy_policy_json', "autonomy_policy_json", 'tool_policy_revision', "tool_policy_revision", 'version', "version", 'replaced_by_binding_id', "replaced_by_binding_id", 'replacement_reason', "replacement_reason", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "account_main_agent_binding";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_action', CAST("id" AS TEXT), json_object('id', "id", 'actor_identity_id', "actor_identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'operation', "operation", 'payload_json', "payload_json", 'payload_hash', "payload_hash", 'dedupe_key', "dedupe_key", 'correlation_id', "correlation_id", 'causation_id', "causation_id", 'causation_depth', "causation_depth", 'requested_permission', "requested_permission", 'policy_result', "policy_result", 'policy_reason', "policy_reason", 'status', "status", 'target_type', "target_type", 'target_id', "target_id", 'outcome_json', "outcome_json", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_action";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_action_approval', CAST("id" AS TEXT), json_object('id', "id", 'action_id', "action_id", 'approver_identity_id', "approver_identity_id", 'decision', "decision", 'reason', "reason", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_action_approval";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_action_execution', CAST("id" AS TEXT), json_object('id', "id", 'action_id', "action_id", 'attempt', "attempt", 'status', "status", 'result_json', "result_json", 'error', "error", 'executed_by_type', "executed_by_type", 'executed_by_id', "executed_by_id", 'idempotency_key', "idempotency_key", 'created_at', "created_at", 'completed_at', "completed_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_action_execution";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_chat', CAST("id" AS TEXT), json_object('id', "id", 'kind', "kind", 'account_id', "account_id", 'project_id', "project_id", 'status', "status", 'instruction_revision', "instruction_revision", 'message_count', "message_count", 'last_message_at', "last_message_at", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_chat";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_chat_instruction_revision', CAST("id" AS TEXT), json_object('id', "id", 'chat_id', "chat_id", 'source_type', "source_type", 'source_id', "source_id", 'revision', "revision", 'body', "body", 'content_guard_json', "content_guard_json", 'sensitivity', "sensitivity", 'created_by_type', "created_by_type", 'created_by_id', "created_by_id", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_chat_instruction_revision";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_chat_message', CAST("id" AS TEXT), json_object('id', "id", 'chat_id', "chat_id", 'sequence', "sequence", 'author_type', "author_type", 'author_id', "author_id", 'content', "content", 'content_guard_json', "content_guard_json", 'sensitivity', "sensitivity", 'status', "status", 'outcome', "outcome", 'model', "model", 'profile_id', "profile_id", 'session_id', "session_id", 'context_manifest_id', "context_manifest_id", 'token_usage_json', "token_usage_json", 'duration_ms', "duration_ms", 'error', "error", 'correlation_id', "correlation_id", 'causation_id', "causation_id", 'handoff_id', "handoff_id", 'source_type', "source_type", 'source_id', "source_id", 'source_message_id', "source_message_id", 'source_room_id', "source_room_id", 'source_conversation_id', "source_conversation_id", 'source_sequence', "source_sequence", 'source_metadata_json', "source_metadata_json", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_chat_message";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_chat_source_ref', json_object('chat_id', "chat_id", 'source_type', "source_type", 'source_id', "source_id"), json_object('chat_id', "chat_id", 'source_type', "source_type", 'source_id', "source_id", 'source_scope_type', "source_scope_type", 'source_scope_id', "source_scope_id", 'source_revision', "source_revision", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_chat_source_ref";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_chat_turn_job', CAST("id" AS TEXT), json_object('id', "id", 'chat_id', "chat_id", 'triggering_message_id', "triggering_message_id", 'responder_identity_id', "responder_identity_id", 'profile_id', "profile_id", 'canonical_scope_type', "canonical_scope_type", 'canonical_scope_id', "canonical_scope_id", 'status', "status", 'dedupe_key', "dedupe_key", 'lease_owner', "lease_owner", 'leased_until', "leased_until", 'attempt_count', "attempt_count", 'max_attempts', "max_attempts", 'next_attempt_at', "next_attempt_at", 'response_message_id', "response_message_id", 'error_code', "error_code", 'error_message', "error_message", 'correlation_id', "correlation_id", 'causation_id', "causation_id", 'causation_depth', "causation_depth", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_chat_turn_job";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_commitment', CAST("id" AS TEXT), json_object('id', "id", 'owner_identity_id', "owner_identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'title', "title", 'description', "description", 'status', "status", 'due_at', "due_at", 'correlation_id', "correlation_id", 'originating_action_id', "originating_action_id", 'originating_task_id', "originating_task_id", 'evidence_required', "evidence_required", 'cancellation_reason', "cancellation_reason", 'blocked_reason', "blocked_reason", 'completed_at', "completed_at", 'cancelled_at', "cancelled_at", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_commitment";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_commitment_evidence', CAST("id" AS TEXT), json_object('id', "id", 'commitment_id', "commitment_id", 'evidence_type', "evidence_type", 'evidence_id', "evidence_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'description', "description", 'metadata_json', "metadata_json", 'authorized_by_type', "authorized_by_type", 'authorized_by_id', "authorized_by_id", 'dedupe_key', "dedupe_key", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_commitment_evidence";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_commitment_lifecycle', CAST("id" AS TEXT), json_object('id', "id", 'commitment_id', "commitment_id", 'from_status', "from_status", 'to_status', "to_status", 'actor_type', "actor_type", 'actor_id', "actor_id", 'reason', "reason", 'evidence_id', "evidence_id", 'dedupe_key', "dedupe_key", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_commitment_lifecycle";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_commitment_transfer', CAST("id" AS TEXT), json_object('id', "id", 'commitment_id', "commitment_id", 'from_identity_id', "from_identity_id", 'to_identity_id', "to_identity_id", 'reason', "reason", 'actor_type', "actor_type", 'actor_id', "actor_id", 'dedupe_key', "dedupe_key", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_commitment_transfer";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_connection_health', CAST("profile_id" AS TEXT), json_object('profile_id', "profile_id", 'status', "status", 'capability_status_json', "capability_status_json", 'checked_at', "checked_at", 'error_code', "error_code", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_connection_health";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_context_scope', CAST("id" AS TEXT), json_object('id', "id", 'identity_id', "identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'project_id', "project_id", 'room_id', "room_id", 'task_id', "task_id", 'task_role', "task_role", 'workspace_access', "workspace_access", 'authority_json', "authority_json", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_context_scope";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_handoff', CAST("id" AS TEXT), json_object('id', "id", 'source_chat_id', "source_chat_id", 'target_chat_id', "target_chat_id", 'source_message_id', "source_message_id", 'source_turn_job_id', "source_turn_job_id", 'target_message_id', "target_message_id", 'target_turn_job_id', "target_turn_job_id", 'author_identity_id', "author_identity_id", 'content', "content", 'content_guard_json', "content_guard_json", 'source_revisions_json', "source_revisions_json", 'status', "status", 'error_code', "error_code", 'correlation_id', "correlation_id", 'causation_id', "causation_id", 'dedupe_key', "dedupe_key", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_handoff";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_handoff_delivery', json_object('handoff_id', "handoff_id", 'delivery_sequence', "delivery_sequence"), json_object('handoff_id', "handoff_id", 'delivery_sequence', "delivery_sequence", 'status', "status", 'target_message_id', "target_message_id", 'target_turn_job_id', "target_turn_job_id", 'error_code', "error_code", 'error_message', "error_message", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_handoff_delivery";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_inbox_item', CAST("id" AS TEXT), json_object('id', "id", 'recipient_identity_id', "recipient_identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'kind', "kind", 'status', "status", 'title', "title", 'body', "body", 'payload_json', "payload_json", 'source_type', "source_type", 'source_id', "source_id", 'correlation_id', "correlation_id", 'causation_id', "causation_id", 'dedupe_key', "dedupe_key", 'read_at', "read_at", 'acknowledged_at', "acknowledged_at", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_inbox_item";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_lcm_entry', json_object('timeline_id', "timeline_id", 'entry_id', "entry_id"), json_object('timeline_id', "timeline_id", 'entry_id', "entry_id", 'sequence', "sequence", 'content_json', "content_json", 'content_fingerprint', "content_fingerprint", 'source_json', "source_json", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_lcm_entry";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_lcm_node', json_object('timeline_id', "timeline_id", 'node_id', "node_id"), json_object('timeline_id', "timeline_id", 'node_id', "node_id", 'kind', "kind", 'range_start', "range_start", 'range_end', "range_end", 'edges_json', "edges_json", 'source_fingerprint', "source_fingerprint", 'summary_revision', "summary_revision", 'summary', "summary", 'policy_revision', "policy_revision", 'algorithm_revision', "algorithm_revision", 'sizer_revision', "sizer_revision", 'provenance_json', "provenance_json", 'token_count', "token_count", 'source_token_count', "source_token_count", 'classification_json', "classification_json", 'revision', "revision", 'superseded_by', "superseded_by", 'operation_id', "operation_id", 'operation_fingerprint', "operation_fingerprint", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_lcm_node";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_lcm_operation', json_object('timeline_id', "timeline_id", 'operation_id', "operation_id"), json_object('timeline_id', "timeline_id", 'operation_id', "operation_id", 'operation_kind', "operation_kind", 'operation_fingerprint', "operation_fingerprint", 'result_revision', "result_revision", 'result_entries', "result_entries", 'result_node_id', "result_node_id", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_lcm_operation";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_lcm_timeline', CAST("id" AS TEXT), json_object('id', "id", 'identity_id', "identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'authorization_revision', "authorization_revision", 'revision', "revision", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_lcm_timeline";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_question', CAST("id" AS TEXT), json_object('id', "id", 'recipient_identity_id', "recipient_identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'status', "status", 'question', "question", 'context_json', "context_json", 'answer', "answer", 'asked_by_type', "asked_by_type", 'asked_by_id', "asked_by_id", 'answered_by_type', "answered_by_type", 'answered_by_id', "answered_by_id", 'inbox_item_id', "inbox_item_id", 'due_at', "due_at", 'correlation_id', "correlation_id", 'version', "version", 'answered_at', "answered_at", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_question";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_wake_budget_window', json_object('identity_id', "identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id"), json_object('identity_id', "identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'window_started_at', "window_started_at", 'window_seconds', "window_seconds", 'admitted_count', "admitted_count", 'version', "version", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_wake_budget_window";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'agent_wake_lease', json_object('identity_id', "identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'incident_key', "incident_key"), json_object('identity_id', "identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'incident_key', "incident_key", 'lease_owner', "lease_owner", 'leased_until', "leased_until", 'reaction_depth', "reaction_depth", 'updated_at', "updated_at", 'cooldown_until', "cooldown_until", 'last_admitted_at', "last_admitted_at", 'admission_count', "admission_count", 'correlation_id', "correlation_id", 'causation_id', "causation_id"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "agent_wake_lease";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'attention_consumer_health', CAST("consumer_name" AS TEXT), json_object('consumer_name', "consumer_name", 'last_sequence', "last_sequence", 'last_started_at', "last_started_at", 'last_success_at', "last_success_at", 'last_error_at', "last_error_at", 'last_error_code', "last_error_code", 'last_error_message', "last_error_message", 'lease_owner', "lease_owner", 'lease_until', "lease_until", 'processed_events', "processed_events", 'version', "version", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "attention_consumer_health";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'attention_projection', CAST("id" AS TEXT), json_object('id', "id", 'attention_type', "attention_type", 'scope_type', "scope_type", 'scope_id', "scope_id", 'identity_id', "identity_id", 'source_event_id', "source_event_id", 'priority', "priority", 'status', "status", 'summary', "summary", 'details_json', "details_json", 'dedupe_key', "dedupe_key", 'occurred_at', "occurred_at", 'updated_at', "updated_at", 'version', "version", 'acknowledged_at', "acknowledged_at", 'snoozed_until', "snoozed_until", 'resolved_at', "resolved_at", 'updated_by_user_id', "updated_by_user_id", 'recommended_action', "recommended_action", 'source_sequence', "source_sequence"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "attention_projection";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'context_manifest', CAST("id" AS TEXT), json_object('id', "id", 'identity_id', "identity_id", 'agent_session_id', "agent_session_id", 'context_scope_id', "context_scope_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'policy_revision', "policy_revision", 'domain_revision', "domain_revision", 'lcm_binding_revision', "lcm_binding_revision", 'runtime_manifest_id', "runtime_manifest_id", 'runtime_manifest_fingerprint', "runtime_manifest_fingerprint", 'combined_fingerprint', "combined_fingerprint", 'request_fingerprint', "request_fingerprint", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "context_manifest";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'context_manifest_source', json_object('manifest_id', "manifest_id", 'ordinal', "ordinal"), json_object('manifest_id', "manifest_id", 'ordinal', "ordinal", 'source_id', "source_id", 'source_type', "source_type", 'source_revision', "source_revision", 'selection_reason', "selection_reason", 'disposition', "disposition", 'retention_priority', "retention_priority", 'fragment_fingerprint', "fragment_fingerprint"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "context_manifest_source";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'forge_memory_source_binding', CAST("id" AS TEXT), json_object('id', "id", 'identity_id', "identity_id", 'context_scope_id', "context_scope_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'account_id', "account_id", 'project_id', "project_id", 'room_id', "room_id", 'task_id', "task_id", 'policy_revision', "policy_revision", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "forge_memory_source_binding";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'legacy_agent_turn_job', CAST("id" AS TEXT), json_object('id', "id", 'room_id', "room_id", 'input_message_id', "input_message_id", 'responder_identity_id', "responder_identity_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'status', "status", 'dedupe_key', "dedupe_key", 'lease_owner', "lease_owner", 'leased_until', "leased_until", 'attempt_count', "attempt_count", 'response_message_id', "response_message_id", 'error', "error", 'correlation_id', "correlation_id", 'causation_id', "causation_id", 'causation_depth', "causation_depth", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "legacy_agent_turn_job";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'legacy_bounded_room_round', CAST("id" AS TEXT), json_object('id', "id", 'room_id', "room_id", 'prompt_message_id', "prompt_message_id", 'deadline_at', "deadline_at", 'response_contract_json', "response_contract_json", 'status', "status", 'synthesis_job_id', "synthesis_job_id", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at", 'max_participants', "max_participants", 'synthesis_identity_id', "synthesis_identity_id"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "legacy_bounded_room_round";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'legacy_bounded_room_round_participant', json_object('round_id', "round_id", 'identity_id', "identity_id"), json_object('round_id', "round_id", 'identity_id', "identity_id", 'turn_job_id', "turn_job_id", 'response_message_id', "response_message_id", 'status', "status"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "legacy_bounded_room_round_participant";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'legacy_room', CAST("id" AS TEXT), json_object('id', "id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'owner_user_id', "owner_user_id", 'owning_project_id', "owning_project_id", 'title', "title", 'status', "status", 'responder_policy', "responder_policy", 'default_responder_identity_id', "default_responder_identity_id", 'history_policy', "history_policy", 'message_count', "message_count", 'last_message_at', "last_message_at", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "legacy_room";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'legacy_room_instruction_revision', CAST("id" AS TEXT), json_object('id', "id", 'room_id', "room_id", 'revision', "revision", 'body', "body", 'content_guard_json', "content_guard_json", 'sensitivity', "sensitivity", 'created_by_type', "created_by_type", 'created_by_id', "created_by_id", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "legacy_room_instruction_revision";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'legacy_room_message', CAST("id" AS TEXT), json_object('id', "id", 'room_id', "room_id", 'author_type', "author_type", 'author_id', "author_id", 'addressed_identity_id', "addressed_identity_id", 'reply_to_message_id', "reply_to_message_id", 'content', "content", 'content_guard_json', "content_guard_json", 'sensitivity', "sensitivity", 'status', "status", 'outcome', "outcome", 'model', "model", 'profile_id', "profile_id", 'session_id', "session_id", 'token_usage_json', "token_usage_json", 'duration_ms', "duration_ms", 'error', "error", 'correlation_id', "correlation_id", 'source_event_id', "source_event_id", 'sequence', "sequence", 'created_at', "created_at", 'context_manifest_id', "context_manifest_id"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "legacy_room_message";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'legacy_room_participant', json_object('room_id', "room_id", 'participant_type', "participant_type", 'participant_id', "participant_id"), json_object('room_id', "room_id", 'participant_type', "participant_type", 'participant_id', "participant_id", 'role', "role", 'can_read_history', "can_read_history", 'state', "state", 'joined_at', "joined_at", 'removed_at', "removed_at", 'history_after_sequence', "history_after_sequence"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "legacy_room_participant";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'legacy_project_agent_membership', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'identity_id', "identity_id", 'role', "role", 'is_primary', "is_primary", 'state', "state", 'permission_ceiling', "permission_ceiling", 'autonomy_policy_json', "autonomy_policy_json", 'subscriptions_json', "subscriptions_json", 'wake_budget', "wake_budget", 'primary_session_id', "primary_session_id", 'created_by_user_id', "created_by_user_id", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "legacy_project_agent_membership";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'memory_item', CAST("row_id" AS TEXT), json_object('row_id', "row_id", 'id', "id", 'project_id', "project_id", 'task_id', "task_id", 'execution_id', "execution_id", 'room_id', "room_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'visibility', "visibility", 'owner_identity_id', "owner_identity_id", 'authority', "authority", 'sensitivity', "sensitivity", 'retention_priority', "retention_priority", 'provenance_json', "provenance_json", 'publication_source_id', "publication_source_id", 'supersedes_id', "supersedes_id", 'valid_from', "valid_from", 'valid_until', "valid_until", 'source_event_id', "source_event_id", 'source_scope_type', "source_scope_type", 'source_scope_id', "source_scope_id", 'source_revision', "source_revision", 'source_room_sequence', "source_room_sequence", 'source_type', "source_type", 'kind', "kind", 'title', "title", 'summary', "summary", 'body', "body", 'metadata_json', "metadata_json", 'confidence', "confidence", 'quality_score', "quality_score", 'created_by_type', "created_by_type", 'created_by_id', "created_by_id", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "memory_item";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'memory_item_legacy', CAST("row_id" AS TEXT), json_object('row_id', "row_id", 'id', "id", 'project_id', "project_id", 'task_id', "task_id", 'execution_id', "execution_id", 'room_id', "room_id", 'scope_type', "scope_type", 'scope_id', "scope_id", 'visibility', "visibility", 'owner_identity_id', "owner_identity_id", 'authority', "authority", 'provenance_json', "provenance_json", 'publication_source_id', "publication_source_id", 'supersedes_id', "supersedes_id", 'valid_from', "valid_from", 'valid_until', "valid_until", 'source_event_id', "source_event_id", 'source_type', "source_type", 'kind', "kind", 'title', "title", 'summary', "summary", 'body', "body", 'metadata_json', "metadata_json", 'confidence', "confidence", 'quality_score', "quality_score", 'created_by_type', "created_by_type", 'created_by_id', "created_by_id", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "memory_item_legacy";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'memory_lifecycle_assertion', CAST("id" AS TEXT), json_object('id', "id", 'memory_item_id', "memory_item_id", 'assertion_type', "assertion_type", 'related_memory_id', "related_memory_id", 'reason', "reason", 'evidence_json', "evidence_json", 'asserted_by_type', "asserted_by_type", 'asserted_by_id', "asserted_by_id", 'source_event_id', "source_event_id", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "memory_lifecycle_assertion";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'memory_source_receipt', json_object('source_type', "source_type", 'source_scope_type', "source_scope_type", 'source_scope_id', "source_scope_id", 'source_ref', "source_ref"), json_object('source_type', "source_type", 'source_scope_type', "source_scope_type", 'source_scope_id', "source_scope_id", 'source_ref', "source_ref", 'memory_item_id', "memory_item_id", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "memory_source_receipt";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'operating_skill', CAST("id" AS TEXT), json_object('id', "id", 'skill_key', "skill_key", 'current_revision_id', "current_revision_id", 'lifecycle', "lifecycle", 'version', "version", 'created_by_type', "created_by_type", 'created_by_id', "created_by_id", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "operating_skill";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'operating_skill_revision', CAST("id" AS TEXT), json_object('id', "id", 'operating_skill_id', "operating_skill_id", 'skill_key', "skill_key", 'revision', "revision", 'schema_version', "schema_version", 'render_version', "render_version", 'canonical_body', "canonical_body", 'policy_json', "policy_json", 'policy_digest', "policy_digest", 'content_digest', "content_digest", 'created_by_type', "created_by_type", 'created_by_id', "created_by_id", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "operating_skill_revision";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'product_genesis_session', CAST("id" AS TEXT), json_object('id', "id", 'account_id', "account_id", 'main_chat_id', "main_chat_id", 'prompt_revision', "prompt_revision", 'prompt_body', "prompt_body", 'maturity', "maturity", 'initial_idea', "initial_idea", 'lifecycle', "lifecycle", 'source_message_ids_json', "source_message_ids_json", 'preferred_project_agent_identity_id', "preferred_project_agent_identity_id", 'project_id', "project_id", 'handoff_id', "handoff_id", 'failure_reason', "failure_reason", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at", 'charter_id', "charter_id", 'charter_revision_id', "charter_revision_id", 'charter_approval_id', "charter_approval_id", 'charter_version', "charter_version"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "product_genesis_session";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_agent_binding', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'identity_id', "identity_id", 'profile_id', "profile_id", 'state', "state", 'autonomy_policy_json', "autonomy_policy_json", 'permission_ceiling_json', "permission_ceiling_json", 'subscriptions_json', "subscriptions_json", 'wake_budget', "wake_budget", 'version', "version", 'replaced_by_binding_id', "replaced_by_binding_id", 'replacement_reason', "replacement_reason", 'created_at', "created_at", 'updated_at', "updated_at", 'operating_skill_revision_id', "operating_skill_revision_id", 'policy_revision', "policy_revision", 'policy_digest', "policy_digest", 'charter_id', "charter_id", 'charter_revision_id', "charter_revision_id", 'charter_setup_required', "charter_setup_required"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_agent_binding";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_canonical_conflict', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'domain', "domain", 'governing_record_type', "governing_record_type", 'governing_record_id', "governing_record_id", 'governing_record_revision', "governing_record_revision", 'governing_record_digest', "governing_record_digest", 'conflicting_record_type', "conflicting_record_type", 'conflicting_record_id', "conflicting_record_id", 'conflicting_record_revision', "conflicting_record_revision", 'conflicting_record_digest', "conflicting_record_digest", 'affected_paths_json', "affected_paths_json", 'conflict_code', "conflict_code", 'description', "description", 'detected_by_type', "detected_by_type", 'detected_by_id', "detected_by_id", 'authorization_basis', "authorization_basis", 'authorization_action', "authorization_action", 'explicit_event', "explicit_event", 'authorization_occurred_at', "authorization_occurred_at", 'idempotency_key', "idempotency_key", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_canonical_conflict";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_charter', CAST("id" AS TEXT), json_object('id', "id", 'account_id', "account_id", 'genesis_session_id', "genesis_session_id", 'project_id', "project_id", 'current_draft_revision_id', "current_draft_revision_id", 'current_approved_revision_id', "current_approved_revision_id", 'project_mode', "project_mode", 'maturity', "maturity", 'lifecycle', "lifecycle", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_charter";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_charter_amendment', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'base_charter_revision_id', "base_charter_revision_id", 'candidate_revision_id', "candidate_revision_id", 'lifecycle', "lifecycle", 'rationale', "rationale", 'material_diff_json', "material_diff_json", 'affected_records_json', "affected_records_json", 'requested_principal_type', "requested_principal_type", 'requested_principal_id', "requested_principal_id", 'expected_project_version', "expected_project_version", 'approval_id', "approval_id", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_charter_amendment";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_charter_approval', CAST("id" AS TEXT), json_object('id', "id", 'approval_type', "approval_type", 'charter_id', "charter_id", 'revision_id', "revision_id", 'content_digest', "content_digest", 'rendered_digest', "rendered_digest", 'expected_charter_version', "expected_charter_version", 'approved_name', "approved_name", 'approved_slug', "approved_slug", 'selected_identity_id', "selected_identity_id", 'selected_profile_id', "selected_profile_id", 'selected_operating_skill_revision_id', "selected_operating_skill_revision_id", 'selected_policy_revision', "selected_policy_revision", 'selected_policy_digest', "selected_policy_digest", 'approving_principal_type', "approving_principal_type", 'approving_principal_id', "approving_principal_id", 'authorization_basis', "authorization_basis", 'authorization_action', "authorization_action", 'explicit_event', "explicit_event", 'authorization_occurred_at', "authorization_occurred_at", 'source_action', "source_action", 'lifecycle', "lifecycle", 'idempotency_key', "idempotency_key", 'consumed_project_id', "consumed_project_id", 'consumed_at', "consumed_at", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at", 'approved_project_mode', "approved_project_mode", 'approval_event_id', "approval_event_id"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_charter_approval";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_charter_approval_event', CAST("id" AS TEXT), json_object('id', "id", 'approval_id', "approval_id", 'lifecycle', "lifecycle", 'principal_type', "principal_type", 'principal_id', "principal_id", 'authorization_basis', "authorization_basis", 'action', "action", 'explicit_event', "explicit_event", 'reason', "reason", 'idempotency_key', "idempotency_key", 'occurred_at', "occurred_at", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_charter_approval_event";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_charter_revision', CAST("id" AS TEXT), json_object('id', "id", 'charter_id', "charter_id", 'revision', "revision", 'base_revision', "base_revision", 'base_revision_id', "base_revision_id", 'lifecycle', "lifecycle", 'schema_version', "schema_version", 'render_version', "render_version", 'content_json', "content_json", 'rendered_view', "rendered_view", 'change_summary', "change_summary", 'author_type', "author_type", 'author_id', "author_id", 'source_message_id', "source_message_id", 'source_turn_job_id', "source_turn_job_id", 'source_refs_json', "source_refs_json", 'content_digest', "content_digest", 'rendered_digest', "rendered_digest", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_charter_revision";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_decision', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'state', "state", 'decision_class', "decision_class", 'question', "question", 'context_json', "context_json", 'options_json', "options_json", 'selected_outcome', "selected_outcome", 'rationale', "rationale", 'principal_type', "principal_type", 'principal_id', "principal_id", 'authority_basis', "authority_basis", 'authorization_action', "authorization_action", 'explicit_event', "explicit_event", 'authorization_occurred_at', "authorization_occurred_at", 'charter_revision_id', "charter_revision_id", 'baseline_revision_id', "baseline_revision_id", 'source_refs_json', "source_refs_json", 'affected_records_json', "affected_records_json", 'supersedes_decision_id', "supersedes_decision_id", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_decision";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_decision_candidate', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'lifecycle', "lifecycle", 'question', "question", 'context_json', "context_json", 'options_json', "options_json", 'selected_outcome', "selected_outcome", 'rationale', "rationale", 'principal_type', "principal_type", 'principal_id', "principal_id", 'source_refs_json', "source_refs_json", 'expected_project_version', "expected_project_version", 'effective_decision_id', "effective_decision_id", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_decision_candidate";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_decision_link', json_object('decision_id', "decision_id", 'link_kind', "link_kind", 'record_id', "record_id"), json_object('decision_id', "decision_id", 'project_id', "project_id", 'link_kind', "link_kind", 'record_id', "record_id", 'record_revision', "record_revision", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_decision_link";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_document', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'kind', "kind", 'title', "title", 'lifecycle', "lifecycle", 'approval_policy', "approval_policy", 'current_draft_revision_id', "current_draft_revision_id", 'current_approved_revision_id', "current_approved_revision_id", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_document";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_document_approval', CAST("id" AS TEXT), json_object('id', "id", 'document_id', "document_id", 'revision_id', "revision_id", 'principal_type', "principal_type", 'principal_id', "principal_id", 'authorization_basis', "authorization_basis", 'authorization_action', "authorization_action", 'explicit_event', "explicit_event", 'authorization_occurred_at', "authorization_occurred_at", 'content_digest', "content_digest", 'rendered_digest', "rendered_digest", 'lifecycle', "lifecycle", 'idempotency_key', "idempotency_key", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_document_approval";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_document_revision', CAST("id" AS TEXT), json_object('id', "id", 'document_id', "document_id", 'revision', "revision", 'base_revision', "base_revision", 'base_revision_id', "base_revision_id", 'lifecycle', "lifecycle", 'schema_version', "schema_version", 'render_version', "render_version", 'content_json', "content_json", 'rendered_view', "rendered_view", 'change_summary', "change_summary", 'author_type', "author_type", 'author_id', "author_id", 'source_refs_json', "source_refs_json", 'content_digest', "content_digest", 'rendered_digest', "rendered_digest", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_document_revision";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_execution_baseline', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'current_revision_id', "current_revision_id", 'lifecycle', "lifecycle", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_execution_baseline";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_execution_baseline_approval', CAST("id" AS TEXT), json_object('id', "id", 'baseline_id', "baseline_id", 'revision_id', "revision_id", 'expected_project_version', "expected_project_version", 'principal_type', "principal_type", 'principal_id', "principal_id", 'authorization_basis', "authorization_basis", 'authorization_action', "authorization_action", 'explicit_event', "explicit_event", 'authorization_occurred_at', "authorization_occurred_at", 'content_digest', "content_digest", 'rendered_digest', "rendered_digest", 'lifecycle', "lifecycle", 'idempotency_key', "idempotency_key", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_execution_baseline_approval";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_execution_baseline_revision', CAST("id" AS TEXT), json_object('id', "id", 'baseline_id', "baseline_id", 'revision', "revision", 'base_revision', "base_revision", 'base_revision_id', "base_revision_id", 'lifecycle', "lifecycle", 'charter_revision_id', "charter_revision_id", 'document_revisions_json', "document_revisions_json", 'plan_items_json', "plan_items_json", 'milestone_id', "milestone_id", 'milestone_ids_json', "milestone_ids_json", 'milestone_definition_revision_ids_json', "milestone_definition_revision_ids_json", 'primary_milestone_id', "primary_milestone_id", 'release_policy_json', "release_policy_json", 'release_policy_revision', "release_policy_revision", 'release_policy_digest', "release_policy_digest", 'acceptance_matrix_json', "acceptance_matrix_json", 'capability_classes_json', "capability_classes_json", 'risk_classes_json', "risk_classes_json", 'adaptive_envelope_json', "adaptive_envelope_json", 'elevated_operations_json', "elevated_operations_json", 'exclusions_json', "exclusions_json", 'rollback_recovery_json', "rollback_recovery_json", 'schema_version', "schema_version", 'render_version', "render_version", 'rendered_view', "rendered_view", 'content_digest', "content_digest", 'rendered_digest', "rendered_digest", 'source_refs_json', "source_refs_json", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_execution_baseline_revision";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_milestone', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'milestone_sequence', "milestone_sequence", 'milestone_key', "milestone_key", 'display_label', "display_label", 'current_definition_revision_id', "current_definition_revision_id", 'lifecycle', "lifecycle", 'blocker_reason_json', "blocker_reason_json", 'stale_reason_json', "stale_reason_json", 'reconciliation_reason_json', "reconciliation_reason_json", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_milestone";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_milestone_check', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'milestone_id', "milestone_id", 'definition_revision_id', "definition_revision_id", 'check_key', "check_key", 'description', "description", 'required', "required", 'source_kind', "source_kind", 'expected_result', "expected_result", 'evidence_required', "evidence_required", 'version', "version", 'current_result_id', "current_result_id", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_milestone_check";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_milestone_check_result', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'milestone_id', "milestone_id", 'check_id', "check_id", 'definition_revision_id', "definition_revision_id", 'outcome', "outcome", 'source_kind', "source_kind", 'source_manifest_json', "source_manifest_json", 'input_digest', "input_digest", 'governing_charter_revision_id', "governing_charter_revision_id", 'governing_baseline_revision_id', "governing_baseline_revision_id", 'principal_type', "principal_type", 'principal_id', "principal_id", 'authorization_basis', "authorization_basis", 'authorization_action', "authorization_action", 'authorization_occurred_at', "authorization_occurred_at", 'expected_version', "expected_version", 'explicit_event', "explicit_event", 'idempotency_key', "idempotency_key", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_milestone_check_result";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_milestone_revision', CAST("id" AS TEXT), json_object('id', "id", 'milestone_id', "milestone_id", 'revision', "revision", 'base_revision', "base_revision", 'base_revision_id', "base_revision_id", 'lifecycle', "lifecycle", 'display_label', "display_label", 'outcome', "outcome", 'included_scope_json', "included_scope_json", 'excluded_scope_json', "excluded_scope_json", 'charter_revision_id', "charter_revision_id", 'document_revisions_json', "document_revisions_json", 'task_selection_json', "task_selection_json", 'dependencies_json', "dependencies_json", 'risks_json', "risks_json", 'acceptance_checks_json', "acceptance_checks_json", 'evidence_requirements_json', "evidence_requirements_json", 'known_issues_json', "known_issues_json", 'change_summary', "change_summary", 'schema_version', "schema_version", 'render_version', "render_version", 'rendered_view', "rendered_view", 'content_digest', "content_digest", 'rendered_digest', "rendered_digest", 'author_type', "author_type", 'author_id', "author_id", 'source_refs_json', "source_refs_json", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_milestone_revision";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_readiness_input', json_object('readiness_snapshot_id', "readiness_snapshot_id", 'ordinal', "ordinal"), json_object('readiness_snapshot_id', "readiness_snapshot_id", 'ordinal', "ordinal", 'source_kind', "source_kind", 'source_id', "source_id", 'source_version', "source_version", 'source_digest', "source_digest", 'availability', "availability", 'disposition', "disposition", 'metadata_json', "metadata_json"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_readiness_input";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_readiness_snapshot', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'milestone_id', "milestone_id", 'definition_revision_id', "definition_revision_id", 'baseline_id', "baseline_id", 'baseline_revision_id', "baseline_revision_id", 'baseline_digest', "baseline_digest", 'release_policy_revision', "release_policy_revision", 'release_policy_digest', "release_policy_digest", 'input_manifest_json', "input_manifest_json", 'event_watermark', "event_watermark", 'outcome', "outcome", 'blocking_reasons_json', "blocking_reasons_json", 'check_results_json', "check_results_json", 'waiver_manifest_json', "waiver_manifest_json", 'evidence_manifest_json', "evidence_manifest_json", 'commit_context_json', "commit_context_json", 'computing_policy_revision', "computing_policy_revision", 'readiness_digest', "readiness_digest", 'principal_type', "principal_type", 'principal_id', "principal_id", 'authorization_basis', "authorization_basis", 'authorization_action', "authorization_action", 'authorization_occurred_at', "authorization_occurred_at", 'expected_milestone_version', "expected_milestone_version", 'explicit_event', "explicit_event", 'idempotency_key', "idempotency_key", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_readiness_snapshot";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_reconciliation_record', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'conflict_id', "conflict_id", 'record_type', "record_type", 'record_id', "record_id", 'record_revision', "record_revision", 'record_digest', "record_digest", 'governing_record_type', "governing_record_type", 'governing_record_id', "governing_record_id", 'governing_record_revision', "governing_record_revision", 'governing_record_digest', "governing_record_digest", 'state', "state", 'current_resolution_id', "current_resolution_id", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_reconciliation_record";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_reconciliation_resolution', CAST("id" AS TEXT), json_object('id', "id", 'reconciliation_id', "reconciliation_id", 'action', "action", 'principal_type', "principal_type", 'principal_id', "principal_id", 'authorization_basis', "authorization_basis", 'authorization_action', "authorization_action", 'explicit_event', "explicit_event", 'authorization_occurred_at', "authorization_occurred_at", 'reason', "reason", 'occurred_at', "occurred_at", 'idempotency_key', "idempotency_key", 'created_at', "created_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_reconciliation_resolution";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_task_governance', CAST("task_id" AS TEXT), json_object('task_id', "task_id", 'project_id', "project_id", 'charter_revision_id', "charter_revision_id", 'baseline_id', "baseline_id", 'baseline_revision_id', "baseline_revision_id", 'plan_item_id', "plan_item_id", 'milestone_id', "milestone_id", 'document_revisions_json', "document_revisions_json", 'capability_class', "capability_class", 'risk_class', "risk_class", 'runnable', "runnable", 'replacement_of_task_id', "replacement_of_task_id", 'provenance_json', "provenance_json", 'version', "version", 'created_at', "created_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_task_governance";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_media_attachment', CAST("id" AS TEXT), json_object('id', "id", 'project_id', "project_id", 'asset_id', "asset_id", 'attachment_kind', "attachment_kind", 'task_media_id', "task_media_id", 'task_id', "task_id", 'milestone_id', "milestone_id", 'milestone_check_id', "milestone_check_id", 'source_task_id', "source_task_id", 'source_execution_id', "source_execution_id", 'source_validation_id', "source_validation_id", 'acceptance_check_ids_json', "acceptance_check_ids_json", 'caption', "caption", 'evidence_kind', "evidence_kind", 'checksum', "checksum", 'availability', "availability", 'project_url', "project_url", 'author_type', "author_type", 'author_id', "author_id", 'authorization_json', "authorization_json", 'version', "version", 'created_at', "created_at", 'deleted_at', "deleted_at", 'updated_at', "updated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "project_media_attachment";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'protected_legacy_session_ref', CAST("room_id" AS TEXT), json_object('room_id', "room_id", 'opaque_session_ref', "opaque_session_ref", 'access_class', "access_class", 'migrated_at', "migrated_at"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM "protected_legacy_session_ref";
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_legacy_columns', CAST(id AS TEXT), json_object('id', "id", 'charter_status', "charter_status", 'charter_setup_required', "charter_setup_required", 'current_charter_id', "current_charter_id", 'current_charter_revision_id', "current_charter_revision_id", 'current_charter_version', "current_charter_version", 'primary_milestone_id', "primary_milestone_id"), strftime('%Y-%m-%dT%H:%M:%fZ', 'now') FROM project;

CREATE TRIGGER pr13_legacy_record_history_no_insert BEFORE INSERT ON pr13_legacy_record_history BEGIN SELECT RAISE(ABORT, 'PR13 legacy history is migration-only'); END;

-- The exact teardown guard still authorizes target Task/Execution/Gate cascades
-- and immutable media tombstone cleanup. Remove only its obsolete milestone hook.
DROP TRIGGER IF EXISTS task_media_project_attachment_after_delete;
DROP TRIGGER IF EXISTS task_media_project_asset_after_insert;
DROP TRIGGER IF EXISTS work_unit_execution_admission_guard;
DROP TRIGGER IF EXISTS domain_event_reject_legacy_room_insert;
DROP TRIGGER IF EXISTS domain_event_reject_legacy_room_update;
-- Retired tables had mutually-referencing guards and immutability triggers.
-- Remove those owners as one pre-drop stage so no trigger executes against
-- a parent table that has already been removed.
DROP TRIGGER IF EXISTS "account_main_binding_identity_profile_guard_insert";
DROP TRIGGER IF EXISTS "account_main_binding_identity_profile_guard_update";
DROP TRIGGER IF EXISTS "agent_action_reject_legacy_room_insert";
DROP TRIGGER IF EXISTS "agent_action_reject_legacy_room_update";
DROP TRIGGER IF EXISTS "agent_chat_context_scope_guard_insert";
DROP TRIGGER IF EXISTS "agent_chat_context_scope_guard_update";
DROP TRIGGER IF EXISTS "agent_chat_instruction_immutable_delete";
DROP TRIGGER IF EXISTS "agent_chat_instruction_immutable_update";
DROP TRIGGER IF EXISTS "agent_chat_message_immutable_delete";
DROP TRIGGER IF EXISTS "agent_commitment_reject_legacy_room_insert";
DROP TRIGGER IF EXISTS "agent_commitment_reject_legacy_room_update";
DROP TRIGGER IF EXISTS "agent_context_scope_reject_legacy_room_insert";
DROP TRIGGER IF EXISTS "agent_context_scope_reject_legacy_room_update";
DROP TRIGGER IF EXISTS "agent_handoff_delivery_immutable_delete";
DROP TRIGGER IF EXISTS "agent_handoff_delivery_immutable_update";
DROP TRIGGER IF EXISTS "agent_handoff_immutable_delete";
DROP TRIGGER IF EXISTS "agent_inbox_item_reject_legacy_room_insert";
DROP TRIGGER IF EXISTS "agent_inbox_item_reject_legacy_room_update";
DROP TRIGGER IF EXISTS "agent_lcm_entry_immutable_update";
DROP TRIGGER IF EXISTS "agent_lcm_entry_truncate_guard";
DROP TRIGGER IF EXISTS "agent_lcm_node_immutable_update";
DROP TRIGGER IF EXISTS "agent_lcm_node_no_delete";
DROP TRIGGER IF EXISTS "agent_lcm_timeline_reject_legacy_room_insert";
DROP TRIGGER IF EXISTS "agent_lcm_timeline_reject_legacy_room_update";
DROP TRIGGER IF EXISTS "agent_question_reject_legacy_room_insert";
DROP TRIGGER IF EXISTS "agent_question_reject_legacy_room_update";
DROP TRIGGER IF EXISTS "agent_wake_budget_window_reject_legacy_room_insert";
DROP TRIGGER IF EXISTS "agent_wake_budget_window_reject_legacy_room_update";
DROP TRIGGER IF EXISTS "agent_wake_lease_reject_legacy_room_insert";
DROP TRIGGER IF EXISTS "agent_wake_lease_reject_legacy_room_update";
DROP TRIGGER IF EXISTS "context_manifest_immutable_delete";
DROP TRIGGER IF EXISTS "context_manifest_reject_legacy_room_insert";
DROP TRIGGER IF EXISTS "context_manifest_reject_legacy_room_update";
DROP TRIGGER IF EXISTS "context_manifest_source_immutable_delete";
DROP TRIGGER IF EXISTS "context_manifest_source_immutable_update";
DROP TRIGGER IF EXISTS "forge_memory_source_binding_immutable_delete";
DROP TRIGGER IF EXISTS "forge_memory_source_binding_immutable_update";
DROP TRIGGER IF EXISTS "forge_memory_source_binding_reject_legacy_room_insert";
DROP TRIGGER IF EXISTS "forge_memory_source_binding_reject_legacy_room_update";
DROP TRIGGER IF EXISTS "memory_item_ad";
DROP TRIGGER IF EXISTS "memory_item_ai";
DROP TRIGGER IF EXISTS "operating_skill_current_revision_guard";
DROP TRIGGER IF EXISTS "operating_skill_revision_immutable_delete";
DROP TRIGGER IF EXISTS "operating_skill_revision_immutable_update";
DROP TRIGGER IF EXISTS "operating_skill_revision_scope_guard_insert";
DROP TRIGGER IF EXISTS "operating_skill_revision_scope_guard_update";
DROP TRIGGER IF EXISTS "pr11_retired_insert_account_main_agent_binding";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_action";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_action_approval";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_action_execution";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_chat";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_chat_instruction_revision";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_chat_message";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_chat_source_ref";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_chat_turn_job";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_commitment";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_commitment_evidence";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_commitment_lifecycle";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_commitment_transfer";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_context_scope";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_handoff";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_handoff_delivery";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_inbox_item";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_lcm_entry";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_lcm_node";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_lcm_operation";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_lcm_timeline";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_question";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_wake_budget_window";
DROP TRIGGER IF EXISTS "pr11_retired_insert_agent_wake_lease";
DROP TRIGGER IF EXISTS "pr11_retired_insert_attention_consumer_health";
DROP TRIGGER IF EXISTS "pr11_retired_insert_attention_projection";
DROP TRIGGER IF EXISTS "pr11_retired_insert_context_manifest";
DROP TRIGGER IF EXISTS "pr11_retired_insert_context_manifest_source";
DROP TRIGGER IF EXISTS "pr11_retired_insert_forge_memory_source_binding";
DROP TRIGGER IF EXISTS "pr11_retired_insert_memory_item";
DROP TRIGGER IF EXISTS "pr11_retired_insert_memory_lifecycle_assertion";
DROP TRIGGER IF EXISTS "pr11_retired_insert_operating_skill";
DROP TRIGGER IF EXISTS "pr11_retired_insert_operating_skill_revision";
DROP TRIGGER IF EXISTS "pr11_retired_insert_product_genesis_session";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_agent_binding";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_charter";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_charter_amendment";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_charter_approval";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_charter_approval_event";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_charter_revision";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_decision";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_decision_candidate";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_decision_link";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_document";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_document_approval";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_document_revision";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_execution_baseline";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_execution_baseline_approval";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_execution_baseline_revision";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_milestone";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_milestone_check";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_milestone_check_result";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_milestone_revision";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_readiness_input";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_readiness_snapshot";
DROP TRIGGER IF EXISTS "pr11_retired_insert_project_task_governance";
DROP TRIGGER IF EXISTS "pr11_retired_update_account_main_agent_binding";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_action";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_action_approval";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_action_execution";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_chat";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_chat_instruction_revision";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_chat_message";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_chat_source_ref";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_chat_turn_job";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_commitment";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_commitment_evidence";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_commitment_lifecycle";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_commitment_transfer";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_context_scope";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_handoff";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_handoff_delivery";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_inbox_item";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_lcm_entry";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_lcm_node";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_lcm_operation";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_lcm_timeline";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_question";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_wake_budget_window";
DROP TRIGGER IF EXISTS "pr11_retired_update_agent_wake_lease";
DROP TRIGGER IF EXISTS "pr11_retired_update_attention_consumer_health";
DROP TRIGGER IF EXISTS "pr11_retired_update_attention_projection";
DROP TRIGGER IF EXISTS "pr11_retired_update_context_manifest";
DROP TRIGGER IF EXISTS "pr11_retired_update_context_manifest_source";
DROP TRIGGER IF EXISTS "pr11_retired_update_forge_memory_source_binding";
DROP TRIGGER IF EXISTS "pr11_retired_update_memory_item";
DROP TRIGGER IF EXISTS "pr11_retired_update_memory_lifecycle_assertion";
DROP TRIGGER IF EXISTS "pr11_retired_update_operating_skill";
DROP TRIGGER IF EXISTS "pr11_retired_update_operating_skill_revision";
DROP TRIGGER IF EXISTS "pr11_retired_update_product_genesis_session";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_agent_binding";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_charter";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_charter_amendment";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_charter_approval";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_charter_approval_event";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_charter_revision";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_decision";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_decision_candidate";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_decision_link";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_document";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_document_approval";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_document_revision";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_execution_baseline";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_execution_baseline_approval";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_execution_baseline_revision";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_milestone";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_milestone_check";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_milestone_check_result";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_milestone_revision";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_readiness_input";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_readiness_snapshot";
DROP TRIGGER IF EXISTS "pr11_retired_update_project_task_governance";
DROP TRIGGER IF EXISTS "product_genesis_charter_scope_guard";
DROP TRIGGER IF EXISTS "product_genesis_main_chat_guard_insert";
DROP TRIGGER IF EXISTS "product_genesis_main_chat_guard_update";
DROP TRIGGER IF EXISTS "product_genesis_prompt_immutable_update";
DROP TRIGGER IF EXISTS "project_binding_identity_profile_guard_insert";
DROP TRIGGER IF EXISTS "project_binding_identity_profile_guard_update";
DROP TRIGGER IF EXISTS "project_binding_operating_skill_scope_guard";
DROP TRIGGER IF EXISTS "project_binding_operating_skill_scope_guard_update";
DROP TRIGGER IF EXISTS "project_canonical_conflict_immutable_delete";
DROP TRIGGER IF EXISTS "project_canonical_conflict_immutable_update";
DROP TRIGGER IF EXISTS "project_canonical_conflict_scope_guard";
DROP TRIGGER IF EXISTS "project_charter_approval_event_immutable_delete";
DROP TRIGGER IF EXISTS "project_charter_approval_event_immutable_update";
DROP TRIGGER IF EXISTS "project_charter_approval_event_scope_guard";
DROP TRIGGER IF EXISTS "project_charter_approval_immutable_delete";
DROP TRIGGER IF EXISTS "project_charter_approval_immutable_update";
DROP TRIGGER IF EXISTS "project_charter_approval_receipt_event_scope_guard";
DROP TRIGGER IF EXISTS "project_charter_approval_receipt_event_scope_guard_insert";
DROP TRIGGER IF EXISTS "project_charter_approval_receipt_immutable_update";
DROP TRIGGER IF EXISTS "project_charter_approval_scope_guard";
DROP TRIGGER IF EXISTS "project_charter_owner_guard_insert";
DROP TRIGGER IF EXISTS "project_charter_pointer_guard_insert_charter";
DROP TRIGGER IF EXISTS "project_charter_revision_base_guard";
DROP TRIGGER IF EXISTS "project_charter_revision_base_scope_guard";
DROP TRIGGER IF EXISTS "project_charter_revision_immutable_delete";
DROP TRIGGER IF EXISTS "project_charter_revision_pointer_guard_update";
DROP TRIGGER IF EXISTS "project_decision_immutable_delete";
DROP TRIGGER IF EXISTS "project_decision_immutable_update";
DROP TRIGGER IF EXISTS "project_decision_link_scope_guard";
DROP TRIGGER IF EXISTS "project_decision_scope_guard";
DROP TRIGGER IF EXISTS "project_document_approval_immutable_delete";
DROP TRIGGER IF EXISTS "project_document_approval_immutable_update";
DROP TRIGGER IF EXISTS "project_document_approval_lifecycle_guard";
DROP TRIGGER IF EXISTS "project_document_approval_scope_guard";
DROP TRIGGER IF EXISTS "project_document_pointer_guard_update";
DROP TRIGGER IF EXISTS "project_document_revision_base_scope_guard";
DROP TRIGGER IF EXISTS "project_document_revision_immutable_delete";
DROP TRIGGER IF EXISTS "project_document_revision_immutable_update";
DROP TRIGGER IF EXISTS "project_execution_baseline_approval_immutable_delete";
DROP TRIGGER IF EXISTS "project_execution_baseline_approval_immutable_update";
DROP TRIGGER IF EXISTS "project_execution_baseline_approval_lifecycle_guard";
DROP TRIGGER IF EXISTS "project_execution_baseline_approval_scope_guard";
DROP TRIGGER IF EXISTS "project_execution_baseline_pointer_scope_guard";
DROP TRIGGER IF EXISTS "project_execution_baseline_revision_base_scope_guard";
DROP TRIGGER IF EXISTS "project_execution_baseline_revision_immutable_delete";
DROP TRIGGER IF EXISTS "project_execution_baseline_revision_immutable_update";
DROP TRIGGER IF EXISTS "project_execution_baseline_revision_scope_guard";
DROP TRIGGER IF EXISTS "project_media_attachment_gc_guard_after_insert";
DROP TRIGGER IF EXISTS "project_media_attachment_immutable_identity_update";
DROP TRIGGER IF EXISTS "project_media_attachment_live_asset_guard_update";
DROP TRIGGER IF EXISTS "project_media_attachment_scope_guard_insert";
DROP TRIGGER IF EXISTS "project_media_attachment_scope_guard_update";
DROP TRIGGER IF EXISTS "project_milestone_check_result_immutable_delete";
DROP TRIGGER IF EXISTS "project_milestone_check_result_immutable_update";
DROP TRIGGER IF EXISTS "project_milestone_check_result_scope_guard";
DROP TRIGGER IF EXISTS "project_milestone_check_scope_guard";
DROP TRIGGER IF EXISTS "project_milestone_identity_guard_insert";
DROP TRIGGER IF EXISTS "project_milestone_identity_guard_update";
DROP TRIGGER IF EXISTS "project_milestone_pointer_guard_insert";
DROP TRIGGER IF EXISTS "project_milestone_pointer_guard_update";
DROP TRIGGER IF EXISTS "project_milestone_revision_base_scope_guard";
DROP TRIGGER IF EXISTS "project_milestone_revision_charter_scope_guard";
DROP TRIGGER IF EXISTS "project_milestone_revision_immutable_delete";
DROP TRIGGER IF EXISTS "project_milestone_revision_immutable_update";
DROP TRIGGER IF EXISTS "project_readiness_input_immutable_delete";
DROP TRIGGER IF EXISTS "project_readiness_input_immutable_update";
DROP TRIGGER IF EXISTS "project_readiness_snapshot_immutable_delete";
DROP TRIGGER IF EXISTS "project_readiness_snapshot_immutable_update";
DROP TRIGGER IF EXISTS "project_readiness_snapshot_scope_guard";
DROP TRIGGER IF EXISTS "project_reconciliation_resolution_immutable_delete";
DROP TRIGGER IF EXISTS "project_reconciliation_resolution_immutable_update";
DROP TRIGGER IF EXISTS "project_reconciliation_resolution_scope_guard";
DROP TRIGGER IF EXISTS "project_reconciliation_scope_guard";
DROP TRIGGER IF EXISTS "project_reconciliation_update_guard";
DROP TRIGGER IF EXISTS "project_task_governance_runnable_guard_insert";
DROP TRIGGER IF EXISTS "project_task_governance_runnable_guard_update";
DROP TRIGGER IF EXISTS "project_task_governance_scope_guard_insert";
DROP TRIGGER IF EXISTS "room_instruction_revision_immutable_delete";
DROP TRIGGER IF EXISTS "room_instruction_revision_immutable_update";
DROP TRIGGER IF EXISTS "room_message_immutable_delete";
DROP TRIGGER IF EXISTS "room_message_immutable_update";


DROP TRIGGER IF EXISTS project_charter_pointer_guard_insert;
DROP TRIGGER IF EXISTS project_charter_pointer_guard_update;
DROP TRIGGER IF EXISTS project_primary_milestone_guard_insert;
DROP TRIGGER IF EXISTS project_primary_milestone_guard_update;

-- FTS tables are derived indexes; the exact raw memory rows are archived above.
DROP TABLE memory_item_fts;
DROP TABLE "account_main_agent_binding";
DROP TABLE "agent_action";
DROP TABLE "agent_action_approval";
DROP TABLE "agent_action_execution";
DROP TABLE "agent_chat";
DROP TABLE "agent_chat_instruction_revision";
DROP TABLE "agent_chat_message";
DROP TABLE "agent_chat_source_ref";
DROP TABLE "agent_chat_turn_job";
DROP TABLE "agent_commitment";
DROP TABLE "agent_commitment_evidence";
DROP TABLE "agent_commitment_lifecycle";
DROP TABLE "agent_commitment_transfer";
DROP TABLE "agent_connection_health";
DROP TABLE "agent_handoff";
DROP TABLE "agent_handoff_delivery";
DROP TABLE "agent_inbox_item";
DROP TABLE "agent_lcm_entry";
DROP TABLE "agent_lcm_node";
DROP TABLE "agent_lcm_operation";
DROP TABLE "agent_lcm_timeline";
DROP TABLE "agent_question";
DROP TABLE "agent_wake_budget_window";
DROP TABLE "agent_wake_lease";
DROP TABLE "attention_consumer_health";
DROP TABLE "attention_projection";
DROP TABLE "context_manifest";
DROP TABLE "context_manifest_source";
DROP TABLE "forge_memory_source_binding";
DROP TABLE "legacy_agent_turn_job";
DROP TABLE "legacy_bounded_room_round";
DROP TABLE "legacy_bounded_room_round_participant";
DROP TABLE "legacy_room";
DROP TABLE "legacy_room_instruction_revision";
DROP TABLE "legacy_room_message";
DROP TABLE "legacy_room_participant";
DROP TABLE "legacy_project_agent_membership";
DROP TABLE "memory_item_legacy";
DROP TABLE "memory_lifecycle_assertion";
DROP TABLE "memory_source_receipt";
DROP TABLE "operating_skill";
DROP TABLE "operating_skill_revision";
DROP TABLE "product_genesis_session";
DROP TABLE "project_agent_binding";
DROP TABLE "project_canonical_conflict";
DROP TABLE "project_charter";
DROP TABLE "project_charter_amendment";
DROP TABLE "project_charter_approval";
DROP TABLE "project_charter_approval_event";
DROP TABLE "project_charter_revision";
DROP TABLE "project_decision";
DROP TABLE "project_decision_candidate";
DROP TABLE "project_decision_link";
DROP TABLE "project_document";
DROP TABLE "project_document_approval";
DROP TABLE "project_document_revision";
DROP TABLE "project_execution_baseline";
DROP TABLE "project_execution_baseline_approval";
DROP TABLE "project_execution_baseline_revision";
DROP TABLE "project_milestone";
DROP TABLE "project_milestone_check";
DROP TABLE "project_milestone_check_result";
DROP TABLE "project_milestone_revision";
DROP TABLE "project_readiness_input";
DROP TABLE "project_readiness_snapshot";
DROP TABLE "project_reconciliation_record";
DROP TABLE "project_reconciliation_resolution";
DROP TABLE "project_task_governance";
DROP TABLE memory_item;
DROP TABLE protected_legacy_session_ref;
DROP TABLE project_media_attachment;

-- Keep exact historical context_scope_id while detaching the old context graph.
CREATE TABLE agent_session_history__pr13 (
    id TEXT PRIMARY KEY,
    identity_id TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    profile_id TEXT NOT NULL REFERENCES agent_profile(id),
    context_scope_id TEXT NOT NULL,
    backend_kind TEXT NOT NULL CHECK (backend_kind IN ('cli', 'native')),
    runtime_session_id TEXT,
    status TEXT NOT NULL DEFAULT 'ready' CHECK (status IN (
        'starting', 'ready', 'running', 'suspended', 'degraded', 'failed', 'cancelled', 'replaced'
    )),
    capabilities_json TEXT NOT NULL DEFAULT '{}',
    connection_status TEXT NOT NULL DEFAULT 'unknown' CHECK (connection_status IN (
        'unknown', 'healthy', 'degraded', 'unavailable'
    )),
    predecessor_session_id TEXT REFERENCES agent_session_history__pr13(id) ON DELETE SET NULL,
    replaced_by_session_id TEXT REFERENCES agent_session_history__pr13(id) ON DELETE SET NULL,
    last_activity_at TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
INSERT INTO agent_session_history__pr13 (
    id, identity_id, profile_id, context_scope_id, backend_kind, runtime_session_id,
    status, capabilities_json, connection_status, predecessor_session_id,
    replaced_by_session_id, last_activity_at, version, created_at, updated_at
)
SELECT id, identity_id, profile_id, context_scope_id, backend_kind, runtime_session_id,
       status, capabilities_json, connection_status, predecessor_session_id,
       replaced_by_session_id, last_activity_at, version, created_at, updated_at
FROM agent_session_history;
DROP TABLE agent_session_history;
ALTER TABLE agent_session_history__pr13 RENAME TO agent_session_history;
CREATE INDEX idx_agent_session_history_identity_activity ON agent_session_history(identity_id, last_activity_at);
CREATE INDEX idx_agent_session_history_identity_created ON agent_session_history(identity_id, created_at DESC, id DESC);

DROP TABLE agent_context_scope;

CREATE TRIGGER work_unit_execution_admission_guard
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
 AND (
    NEW.status != 'running'
    OR
    NEW.work_unit_version IS NULL
    OR NEW.actor_kind IS NULL OR NEW.actor_id IS NULL
    OR NEW.purpose IS NULL
    OR (NEW.actor_kind = 'agent' AND NOT EXISTS (
        SELECT 1 FROM agent_current a
        WHERE a.id = NEW.actor_id AND NEW.agent_id = a.id
          AND (
              SELECT COUNT(*) FROM execution e
              WHERE e.agent_id = a.id AND e.status = 'running'
          ) < a.max_concurrent_tasks
    ))
    OR NOT EXISTS (
        SELECT 1 FROM work_unit w
        WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
          AND w.version = NEW.work_unit_version AND w.status = 'open'
          AND w.role = NEW.role
          AND ((w.requires_integration = 0 AND NEW.workspace_id IS NULL)
               OR (w.requires_integration = 1 AND NEW.status = 'running'
                   AND NEW.actor_kind = 'agent' AND NEW.agent_id = NEW.actor_id))
          AND (w.assigned_actor_id IS NULL
               OR (w.assigned_actor_kind = NEW.actor_kind AND w.assigned_actor_id = NEW.actor_id))
          AND ((w.requires_integration = 1 AND EXISTS (
              SELECT 1 FROM workspace_scope ws
              JOIN workspace bound_workspace ON bound_workspace.id = ws.workspace_id
              WHERE ws.workspace_id = NEW.workspace_id AND ws.task_id = NEW.task_id
                AND ws.scope_kind = 'work_unit' AND ws.work_unit_id = w.id
                AND bound_workspace.status = 'ready'
                AND bound_workspace.repo_id = (SELECT t.repo_id FROM task t WHERE t.id = w.task_id)
          )) OR (w.requires_integration = 0 AND NEW.workspace_id IS NULL))
          AND EXISTS (
              SELECT 1 FROM task_role tr JOIN role_membership rm ON rm.task_role_id = tr.id
              WHERE tr.task_id = w.task_id AND tr.role = w.role
                AND rm.actor_kind = NEW.actor_kind AND rm.actor_id = NEW.actor_id
                AND rm.status = 'active'
          )
          AND NOT EXISTS (
              SELECT 1 FROM execution active
              WHERE active.work_unit_id = w.id AND active.status = 'running'
          )
          AND NOT EXISTS (
              SELECT 1 FROM work_unit_dependency d
              JOIN work_unit p ON p.id = d.depends_on_work_unit_id AND p.task_id = d.task_id
              WHERE d.work_unit_id = w.id AND d.task_id = w.task_id
                AND (p.status != 'completed' OR
                     (p.requires_integration = 1 AND NOT EXISTS (
                         SELECT 1 FROM work_unit_integration i
                         WHERE i.work_unit_id = p.id AND i.task_id = p.task_id
                           AND i.outcome = 'success'
                     )))
          )
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Execution admission is not authorized, runnable, or Agent reached concurrent execution capacity');
END;

-- Preserve shared Task media asset creation/cleanup, without manufacturing a
-- Project OS attachment row for each task_media write.
CREATE TRIGGER task_media_project_asset_after_insert
AFTER INSERT ON task_media
BEGIN
    INSERT OR IGNORE INTO media_asset (
        id, project_id, legacy_task_media_id, display_filename, content_type,
        byte_size, storage_key, checksum, availability, gc_state,
        created_at, updated_at
    )
    SELECT NEW.id, t.project_id, NEW.id, NEW.display_filename, NEW.content_type,
           NEW.byte_size, NEW.storage_key, NULL, 'available', 'referenced',
           NEW.created_at, NEW.created_at
    FROM task t WHERE t.id = NEW.task_id;

    UPDATE task_media
    SET asset_id = COALESCE(
        asset_id,
        (SELECT id FROM media_asset WHERE storage_key = NEW.storage_key ORDER BY id LIMIT 1)
    )
    WHERE id = NEW.id;
END;

CREATE TRIGGER task_media_project_attachment_after_delete
AFTER UPDATE OF deleted_at ON task_media
WHEN OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL
BEGIN
    UPDATE media_asset
    SET gc_state = CASE
            WHEN EXISTS (SELECT 1 FROM project_release_history_media_pin p WHERE p.asset_id = NEW.asset_id)
                THEN 'referenced' ELSE 'gc_candidate' END,
        gc_candidate_at = CASE
            WHEN EXISTS (SELECT 1 FROM project_release_history_media_pin p WHERE p.asset_id = NEW.asset_id)
                THEN NULL ELSE NEW.deleted_at END,
        deleted_at = CASE
            WHEN EXISTS (SELECT 1 FROM project_release_history_media_pin p WHERE p.asset_id = NEW.asset_id)
                THEN NULL ELSE NEW.deleted_at END,
        gc_lease_owner = NULL,
        gc_lease_expires_at = NULL,
        version = version + 1,
        updated_at = NEW.deleted_at
    WHERE id = NEW.asset_id;
END;

-- Drop only legacy Project OS columns in place. Target triggers on workspace,
-- Task, and media continue to refer to the same Project table identity.
ALTER TABLE project DROP COLUMN charter_status;
ALTER TABLE project DROP COLUMN charter_setup_required;
ALTER TABLE project DROP COLUMN current_charter_id;
ALTER TABLE project DROP COLUMN current_charter_revision_id;
ALTER TABLE project DROP COLUMN current_charter_version;
ALTER TABLE project DROP COLUMN primary_milestone_id;

INSERT INTO _pr13_pr11_check SELECT (SELECT COUNT(*) FROM pr13_legacy_record_history h WHERE h.source_table = e.source_table) = e.rows FROM _pr13_pr11_expected e;
INSERT INTO _pr13_pr11_check SELECT NOT EXISTS (SELECT 1 FROM sqlite_master WHERE type='table' AND name IN ('account_main_agent_binding', 'agent_action', 'agent_action_approval', 'agent_action_execution', 'agent_chat', 'agent_chat_instruction_revision', 'agent_chat_message', 'agent_chat_source_ref', 'agent_chat_turn_job', 'agent_commitment', 'agent_commitment_evidence', 'agent_commitment_lifecycle', 'agent_commitment_transfer', 'agent_connection_health', 'agent_context_scope', 'agent_handoff', 'agent_handoff_delivery', 'agent_inbox_item', 'agent_lcm_entry', 'agent_lcm_node', 'agent_lcm_operation', 'agent_lcm_timeline', 'agent_question', 'agent_wake_budget_window', 'agent_wake_lease', 'attention_consumer_health', 'attention_projection', 'context_manifest', 'context_manifest_source', 'forge_memory_source_binding', 'legacy_agent_turn_job', 'legacy_bounded_room_round', 'legacy_bounded_room_round_participant', 'legacy_room', 'legacy_room_instruction_revision', 'legacy_room_message', 'legacy_room_participant', 'legacy_project_agent_membership', 'memory_item', 'memory_item_legacy', 'memory_lifecycle_assertion', 'memory_source_receipt', 'operating_skill', 'operating_skill_revision', 'product_genesis_session', 'project_agent_binding', 'project_canonical_conflict', 'project_charter', 'project_charter_amendment', 'project_charter_approval', 'project_charter_approval_event', 'project_charter_revision', 'project_decision', 'project_decision_candidate', 'project_decision_link', 'project_document', 'project_document_approval', 'project_document_revision', 'project_execution_baseline', 'project_execution_baseline_approval', 'project_execution_baseline_revision', 'project_milestone', 'project_milestone_check', 'project_milestone_check_result', 'project_milestone_revision', 'project_readiness_input', 'project_readiness_snapshot', 'project_reconciliation_record', 'project_reconciliation_resolution', 'project_task_governance', 'project_media_attachment', 'protected_legacy_session_ref'));
INSERT INTO _pr13_pr11_check SELECT NOT EXISTS (SELECT 1 FROM pragma_table_info('project') WHERE name IN ('charter_status','charter_setup_required','current_charter_id','current_charter_revision_id','current_charter_version','primary_milestone_id'));
INSERT INTO _pr13_pr11_check SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_list('agent_session_history') WHERE "table" = 'agent_context_scope');
INSERT INTO _pr13_pr11_check SELECT NOT EXISTS (SELECT 1 FROM sqlite_master source JOIN pragma_foreign_key_list(source.name) fk WHERE source.type='table' AND fk."table" IN ('account_main_agent_binding', 'agent_action', 'agent_action_approval', 'agent_action_execution', 'agent_chat', 'agent_chat_instruction_revision', 'agent_chat_message', 'agent_chat_source_ref', 'agent_chat_turn_job', 'agent_commitment', 'agent_commitment_evidence', 'agent_commitment_lifecycle', 'agent_commitment_transfer', 'agent_connection_health', 'agent_context_scope', 'agent_handoff', 'agent_handoff_delivery', 'agent_inbox_item', 'agent_lcm_entry', 'agent_lcm_node', 'agent_lcm_operation', 'agent_lcm_timeline', 'agent_question', 'agent_wake_budget_window', 'agent_wake_lease', 'attention_consumer_health', 'attention_projection', 'context_manifest', 'context_manifest_source', 'forge_memory_source_binding', 'legacy_agent_turn_job', 'legacy_bounded_room_round', 'legacy_bounded_room_round_participant', 'legacy_room', 'legacy_room_instruction_revision', 'legacy_room_message', 'legacy_room_participant', 'legacy_project_agent_membership', 'memory_item', 'memory_item_legacy', 'memory_lifecycle_assertion', 'memory_source_receipt', 'operating_skill', 'operating_skill_revision', 'product_genesis_session', 'project_agent_binding', 'project_canonical_conflict', 'project_charter', 'project_charter_amendment', 'project_charter_approval', 'project_charter_approval_event', 'project_charter_revision', 'project_decision', 'project_decision_candidate', 'project_decision_link', 'project_document', 'project_document_approval', 'project_document_revision', 'project_execution_baseline', 'project_execution_baseline_approval', 'project_execution_baseline_revision', 'project_milestone', 'project_milestone_check', 'project_milestone_check_result', 'project_milestone_revision', 'project_readiness_input', 'project_readiness_snapshot', 'project_reconciliation_record', 'project_reconciliation_resolution', 'project_task_governance', 'project_media_attachment', 'protected_legacy_session_ref'));
INSERT INTO _pr13_pr11_check SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO _pr13_pr11_check SELECT (SELECT integrity_check FROM pragma_integrity_check LIMIT 1) = 'ok';
DROP TABLE _pr13_pr11_check;
DROP TABLE _pr13_pr11_expected;
INSERT INTO _migration (version, name, applied_at) VALUES (132, 'retire_pr11_project_os_storage', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
PRAGMA foreign_keys = ON;
