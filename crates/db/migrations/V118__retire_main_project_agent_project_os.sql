-- Plan PR11: preserve Main Agent, Project Agent, and Project OS history while
-- removing their runtime write and authority paths.  No legacy table is
-- dropped and no generic Task-scoped record is fabricated.

CREATE TABLE pr11_vertical_migration_issue (
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK (disposition IN (
        'migrated', 'historical_only', 'already_represented',
        'ambiguous', 'not_applicable'
    )),
    reason TEXT NOT NULL,
    details_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    PRIMARY KEY (source_kind, source_id),
    CHECK (json_valid(details_json))
);

-- Capture the exact RoleMembership rows that lose their only valid Project
-- scope when Project Agent bindings stop granting authority.  Agent identity
-- and Task membership history are preserved; only ineligible active/suspended
-- memberships are ended.  The Project owner/member and global-visibility
-- eligibility rules match services::project_actor_scope.
DROP TABLE IF EXISTS temp.pr11_binding_only_membership;
CREATE TEMP TABLE pr11_binding_only_membership AS
SELECT rm.id AS membership_id,
       rm.task_role_id,
       tr.task_id,
       tr.role,
       t.project_id,
       p.owner_id AS project_owner_id,
       rm.actor_id AS agent_id,
       rm.status AS prior_status,
       EXISTS (
           SELECT 1 FROM project_agent_binding b
           WHERE b.project_id = t.project_id
             AND b.identity_id = rm.actor_id
             AND b.state = 'active'
       ) AS had_active_binding
FROM role_membership rm
JOIN task_role tr ON tr.id = rm.task_role_id
JOIN task t ON t.id = tr.task_id
JOIN project p ON p.id = t.project_id
WHERE rm.actor_kind = 'agent'
  AND rm.status IN ('active', 'suspended')
  AND EXISTS (
      SELECT 1 FROM project_agent_binding b
      WHERE b.project_id = t.project_id
        AND b.identity_id = rm.actor_id
        AND b.state = 'active'
  )
  AND NOT EXISTS (
      SELECT 1
      FROM agent_current a
      WHERE a.id = rm.actor_id
        AND (
            a.visibility = 'global'
            OR (
                a.visibility = 'account'
                AND a.owner_id IS NOT NULL
                AND (
                    a.owner_id = p.owner_id
                    OR EXISTS (
                        SELECT 1 FROM project_member pm
                        WHERE pm.project_id = p.id AND pm.user_id = a.owner_id
                    )
                )
            )
        )
  );

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'role_membership', membership_id, 'historical_only',
       'ended because the retired active Project Agent binding was the sole Project eligibility',
       json_object(
           'project_id', project_id,
           'task_id', task_id,
           'task_role_id', task_role_id,
           'role', role,
           'agent_id', agent_id,
           'prior_status', prior_status,
           'had_active_project_agent_binding', had_active_binding
       ),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM temp.pr11_binding_only_membership WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

UPDATE role_membership
SET status = 'ended',
    ended_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
    version = version + 1
WHERE id IN (SELECT membership_id FROM temp.pr11_binding_only_membership)
  AND status IN ('active', 'suspended');

-- Clear a compatibility projection only when it directly names the Agent
-- whose membership was ended. Never select a surviving Actor as a replacement;
-- the TaskRole/RoleMembership set remains the complete authority.
UPDATE task_role_assignment AS assignment
SET assignee_type = NULL,
    assignee_id = NULL,
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE assignment.assignee_type = 'agent'
  AND EXISTS (
    SELECT 1
    FROM temp.pr11_binding_only_membership affected
    JOIN task_role role ON role.id = affected.task_role_id
    WHERE role.task_id = assignment.task_id
      AND affected.agent_id = assignment.assignee_id
      AND role.role = CASE lower(trim(assignment.role_name))
          WHEN 'coder' THEN 'implementer'
          WHEN 'worker' THEN 'implementer'
          WHEN 'assignee' THEN 'implementer'
          WHEN 'executor' THEN 'implementer'
          ELSE lower(trim(assignment.role_name))
      END
);

UPDATE task
SET assignee_type = CASE WHEN EXISTS (
        SELECT 1 FROM temp.pr11_binding_only_membership affected
        WHERE affected.task_id = task.id
          AND affected.role = 'implementer'
          AND affected.agent_id = task.assignee_id
    ) THEN NULL ELSE assignee_type END,
    assignee_id = CASE WHEN EXISTS (
        SELECT 1 FROM temp.pr11_binding_only_membership affected
        WHERE affected.task_id = task.id
          AND affected.role = 'implementer'
          AND affected.agent_id = task.assignee_id
    ) THEN NULL ELSE assignee_id END,
    version = version + 1,
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE EXISTS (
    SELECT 1 FROM temp.pr11_binding_only_membership affected
    WHERE affected.task_id = task.id
);

-- The durable Task event becomes visible to event consumers only after this
-- migration transaction commits.  OrchestratorRuntime treats task.updated as
-- a generic Task wake; it never reads the retired binding or legacy projection.
INSERT INTO domain_event (
    id, event_type, entity_type, entity_id, actor_type, actor_id,
    scope_type, scope_id, correlation_id, causation_id, causation_depth,
    dedupe_key, payload_json, created_at
)
SELECT lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
           lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
           substr('89ab', 1 + (abs(random()) % 4), 1) ||
           lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
       'task.updated', 'task', affected.task_id, 'migration', NULL,
       'task', affected.task_id, 'pr11:' || affected.task_id, NULL, 0,
       'pr11:project-agent-scope:' || affected.task_id,
       json_object('change_kind', 'role_membership_scope_revoked',
                   'project_id', affected.project_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM (SELECT DISTINCT task_id, project_id
      FROM temp.pr11_binding_only_membership) AS affected
WHERE true
ON CONFLICT(dedupe_key) WHERE dedupe_key IS NOT NULL DO NOTHING;

-- V071's two AFTER INSERT triggers silently create new cognitive vertical
-- records for every direct SQL user/Project insert. Keep the historical
-- identity, immutability, and scope guards; remove only these row creators.
DROP TRIGGER IF EXISTS user_agent_chat_after_insert;
DROP TRIGGER IF EXISTS project_agent_chat_after_insert;

-- Seed exact, restart-safe historical dispositions before fencing new writes.
-- No row in these domains has the complete Task/producer/decider provenance
-- required for an automatic generic conversion. Existing Tasks and their
-- V2 rows remain their own authority.
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'account_main_agent_binding', id, 'historical_only',
       'Main Agent binding is retained for audit and no longer grants authority',
       json_object('state', state, 'identity_id', identity_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM account_main_agent_binding WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_agent_binding', id, 'historical_only',
       'Project Agent binding is retained for audit and no longer grants Task authority',
       json_object('project_id', project_id, 'state', state, 'identity_id', identity_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_agent_binding WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_chat', id, 'historical_only',
       'Agent Chat transcript is preserved without a cognition runtime',
       json_object('kind', kind, 'status', status),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_chat WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_chat_turn_job', id, 'historical_only',
       'Pre-cutover turn job is permanently unclaimable and has no fabricated result',
       json_object('status', status, 'attempt_count', attempt_count),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_chat_turn_job WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_handoff', id, 'historical_only',
       'Main-to-Project handoff has no proven Task-scoped Handoff equivalent',
       json_object('source_chat_id', source_chat_id, 'target_chat_id', target_chat_id,
                   'status', status),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_handoff WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'product_genesis_session', id, 'historical_only',
       'Genesis state is preserved and is not continued or converted into a Project or Task',
       json_object('lifecycle', lifecycle, 'project_id', project_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM product_genesis_session WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_action', id, 'historical_only',
       'AgentAction cannot authorize or replay a generic Task operation',
       json_object('operation', operation, 'status', status, 'target_type', target_type,
                   'target_id', target_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_action WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_commitment', id,
       CASE WHEN originating_task_id IS NOT NULL AND EXISTS (
                    SELECT 1 FROM task t WHERE t.id = agent_commitment.originating_task_id
            ) THEN 'already_represented' ELSE 'historical_only' END,
       'Commitment history is retained; an existing originating Task remains the only Task authority',
       json_object('originating_task_id', originating_task_id, 'status', status),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_commitment WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_inbox_item', id, 'historical_only',
       'Inbox projection is preserved and is not copied to Message or Handoff',
       json_object('scope_type', scope_type, 'scope_id', scope_id, 'status', status),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_inbox_item WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_question', id, 'historical_only',
       'Question history has no exact generic Task Message or Handoff provenance',
       json_object('scope_type', scope_type, 'scope_id', scope_id, 'status', status),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_question WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'memory_item', id, 'historical_only',
       'Semantic memory is preserved without promotion into Artifact or runtime context',
       json_object('source_type', source_type, 'task_id', task_id,
                   'execution_id', execution_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM memory_item WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'context_manifest', id, 'historical_only',
       'Context manifest is preserved as historical Agent cognition provenance',
       json_object('scope_type', scope_type, 'scope_id', scope_id,
                   'combined_fingerprint', combined_fingerprint),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM context_manifest WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_lcm_timeline', id, 'historical_only',
       'Lossless Agent context timeline remains historical and is not Task Artifact output',
       json_object('scope_type', scope_type, 'scope_id', scope_id,
                   'revision', revision),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_lcm_timeline WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'attention_projection', id, 'historical_only',
       'Attention projection is not current Task authority or Agent dispatch input',
       json_object('attention_type', attention_type, 'status', status),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM attention_projection WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_charter', id, 'historical_only',
       'Charter and approvals remain historical and cannot authorize new work',
       json_object('project_id', project_id, 'lifecycle', lifecycle),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_charter WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_document', id, 'historical_only',
       'Project Document is preserved because exact Task and producer provenance is not proven',
       json_object('project_id', project_id, 'kind', kind),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_document WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_decision', id, 'historical_only',
       'Project Decision is preserved without a synthetic Proposal or generic Decision',
       json_object('project_id', project_id, 'decision_class', decision_class,
                   'state', state),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_decision WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_execution_baseline', id, 'historical_only',
       'Execution Baseline is retained but does not admit Task Execution',
       json_object('project_id', project_id, 'lifecycle', lifecycle),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_execution_baseline WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_task_governance', task_id, 'already_represented',
       'The existing Task remains; legacy governance is no longer an admission input',
       json_object('project_id', project_id, 'runnable', runnable,
                   'baseline_id', baseline_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_task_governance WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_milestone', id, 'historical_only',
       'Milestone and checks remain historical; current readiness comes from Gates',
       json_object('project_id', project_id, 'lifecycle', lifecycle),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_milestone WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_readiness_snapshot', id, 'historical_only',
       'Readiness snapshot is preserved and cannot make a current Task ready to merge',
       json_object('project_id', project_id, 'outcome', outcome),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_readiness_snapshot WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_release', id, 'historical_only',
       'Release and media pins remain historical and are not recalculated or deleted',
       json_object('project_id', project_id, 'release_identifier', release_identifier,
                   'release_sequence', release_sequence),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_release WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

-- Record child snapshots as well as their parents so the audit count covers
-- approvals, revisions, and immutable delivery/evidence rows individually.
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_chat_message', id, 'historical_only',
       'Agent Chat message remains an immutable transcript row', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_chat_message WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_chat_instruction_revision', id, 'historical_only',
       'Agent Chat instruction remains historical configuration', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_chat_instruction_revision WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_action_approval', id, 'historical_only',
       'AgentAction approval cannot authorize generic Proposal or Decision work', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_action_approval WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_action_execution', id, 'historical_only',
       'AgentAction execution receipt cannot be replayed after cutover', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_action_execution WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_commitment_evidence', id, 'historical_only',
       'Commitment evidence remains in its original provenance domain', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_commitment_evidence WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_commitment_transfer', id, 'historical_only',
       'Commitment transfer remains historical and does not create a Handoff', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_commitment_transfer WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_commitment_lifecycle', id, 'historical_only',
       'Commitment lifecycle receipt remains historical', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_commitment_lifecycle WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'memory_lifecycle_assertion', id, 'historical_only',
       'Memory lifecycle assertion remains historical semantic-memory evidence', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM memory_lifecycle_assertion WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'operating_skill', id, 'historical_only',
       'Forge-owned Main/Project Agent operating skill is no longer runtime authority', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM operating_skill WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'operating_skill_revision', id, 'historical_only',
       'Forge-owned Main/Project Agent operating skill revision is preserved', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM operating_skill_revision WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_charter_revision', id, 'historical_only',
       'Charter revision is preserved without becoming a generic Artifact', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_charter_revision WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_charter_approval', id, 'historical_only',
       'Charter approval cannot authorize a new Project or Task', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_charter_approval WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_charter_approval_event', id, 'historical_only',
       'Charter approval event remains historical authorization evidence', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_charter_approval_event WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_charter_amendment', id, 'historical_only',
       'Charter amendment remains historical and is not converted into Proposal/Decision', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_charter_amendment WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_document_revision', id, 'historical_only',
       'Project Document revision lacks proven Task and producer provenance for Artifact', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_document_revision WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_document_approval', id, 'historical_only',
       'Project Document approval remains historical and does not create a Decision', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_document_approval WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_decision_candidate', id, 'historical_only',
       'Project Decision candidate is preserved without a synthetic Task Proposal', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_decision_candidate WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_decision_link', json_array(decision_id, link_kind, record_id), 'historical_only',
       'Project Decision link remains historical and does not attach current Gate evidence',
       json_object('decision_id', decision_id, 'project_id', project_id,
                   'link_kind', link_kind, 'record_id', record_id,
                   'record_revision', record_revision),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_decision_link WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_execution_baseline_revision', id, 'historical_only',
       'Execution Baseline revision remains historical Project OS state', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_execution_baseline_revision WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_execution_baseline_approval', id, 'historical_only',
       'Execution Baseline approval no longer authorizes Task Execution', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_execution_baseline_approval WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_milestone_revision', id, 'historical_only',
       'Milestone revision is preserved and does not affect Task or Gate state', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_milestone_revision WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_milestone_check', id, 'historical_only',
       'Milestone check remains historical and cannot satisfy a current Gate', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_milestone_check WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_milestone_check_result', id, 'historical_only',
       'Milestone check result remains historical and cannot satisfy a current Gate', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_milestone_check_result WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_release_media_pin', id, 'historical_only',
       'Release media pin remains intact so media references and bytes are preserved', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_release_media_pin WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_chat_source_ref', json_array(chat_id, source_type, source_id), 'historical_only',
       'Agent Chat source reference remains transcript provenance',
       json_object('chat_id', chat_id, 'source_type', source_type, 'source_id', source_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_chat_source_ref WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_handoff_delivery', json_array(handoff_id, delivery_sequence), 'historical_only',
       'Main-to-Project delivery receipt remains immutable history',
       json_object('handoff_id', handoff_id, 'delivery_sequence', delivery_sequence,
                   'status', status),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_handoff_delivery WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_context_scope', id, 'historical_only',
       'Agent cognitive context scope is preserved without granting Task authority',
       json_object('identity_id', identity_id, 'scope_type', scope_type, 'scope_id', scope_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_context_scope WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'forge_memory_source_binding', id, 'historical_only',
       'Memory source binding remains historical context provenance',
       json_object('identity_id', identity_id, 'scope_type', scope_type, 'scope_id', scope_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM forge_memory_source_binding WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'context_manifest_source', json_array(manifest_id, ordinal), 'historical_only',
       'Context manifest source selection remains immutable cognition provenance',
       json_object('manifest_id', manifest_id, 'ordinal', ordinal,
                   'source_type', source_type, 'source_id', source_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM context_manifest_source WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_lcm_entry', json_array(timeline_id, entry_id), 'historical_only',
       'Agent LCM entry is preserved without runtime semantic-memory use',
       json_object('timeline_id', timeline_id, 'entry_id', entry_id, 'sequence', sequence),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_lcm_entry WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_lcm_node', json_array(timeline_id, node_id), 'historical_only',
       'Agent LCM summary node is preserved without Artifact promotion',
       json_object('timeline_id', timeline_id, 'node_id', node_id, 'kind', kind),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_lcm_node WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_lcm_operation', json_array(timeline_id, operation_id), 'historical_only',
       'Agent LCM operation receipt remains cognitive history',
       json_object('timeline_id', timeline_id, 'operation_id', operation_id,
                   'operation_kind', operation_kind),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_lcm_operation WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_wake_lease', json_array(identity_id, scope_type, scope_id, incident_key), 'historical_only',
       'Agent wake lease is not consumed by the Task OrchestratorRuntime',
       json_object('identity_id', identity_id, 'scope_type', scope_type, 'scope_id', scope_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_wake_lease WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'agent_wake_budget_window', json_array(identity_id, scope_type, scope_id), 'historical_only',
       'Agent wake budget is no longer Task dispatch authority',
       json_object('identity_id', identity_id, 'scope_type', scope_type, 'scope_id', scope_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM agent_wake_budget_window WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'attention_consumer_health', consumer_name, 'historical_only',
       'Attention consumer health is no longer runtime dispatch state', '{}',
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM attention_consumer_health WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_readiness_input', json_array(readiness_snapshot_id, ordinal), 'historical_only',
       'Readiness input remains pinned to its historical snapshot and cannot satisfy a current Gate',
       json_object('readiness_snapshot_id', readiness_snapshot_id, 'ordinal', ordinal,
                   'source_kind', source_kind, 'source_id', source_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_readiness_input WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;
INSERT INTO pr11_vertical_migration_issue
    (source_kind, source_id, disposition, reason, details_json, created_at)
SELECT 'project_release_reference', json_array(release_id, ordinal), 'historical_only',
       'Release reference remains pinned to its historical release record',
       json_object('release_id', release_id, 'ordinal', ordinal,
                   'reference_kind', reference_kind, 'record_id', record_id),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_release_reference WHERE true
ON CONFLICT(source_kind, source_id) DO NOTHING;

-- Fence all legacy insert/update writers, including SQL callers that bypass
-- services. DELETE remains available for the existing Project/Account
-- deletion lifecycle and its historical FK behavior. This deliberately
-- leaves V071/V076/V088 immutability and scope guards intact.
CREATE TRIGGER pr11_retired_insert_account_main_agent_binding
BEFORE INSERT ON account_main_agent_binding BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Main Agent binding');
END;
CREATE TRIGGER pr11_retired_update_account_main_agent_binding
BEFORE UPDATE ON account_main_agent_binding BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Main Agent binding');
END;
CREATE TRIGGER pr11_retired_insert_project_agent_binding
BEFORE INSERT ON project_agent_binding BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Agent binding');
END;
CREATE TRIGGER pr11_retired_update_project_agent_binding
BEFORE UPDATE ON project_agent_binding BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Agent binding');
END;
CREATE TRIGGER pr11_retired_insert_agent_chat
BEFORE INSERT ON agent_chat BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat');
END;
CREATE TRIGGER pr11_retired_update_agent_chat
BEFORE UPDATE ON agent_chat BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat');
END;
CREATE TRIGGER pr11_retired_insert_agent_chat_message
BEFORE INSERT ON agent_chat_message BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat');
END;
CREATE TRIGGER pr11_retired_update_agent_chat_message
BEFORE UPDATE ON agent_chat_message BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat');
END;
CREATE TRIGGER pr11_retired_insert_agent_chat_turn_job
BEFORE INSERT ON agent_chat_turn_job BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat turn');
END;
CREATE TRIGGER pr11_retired_update_agent_chat_turn_job
BEFORE UPDATE ON agent_chat_turn_job BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat turn');
END;
CREATE TRIGGER pr11_retired_insert_agent_chat_instruction_revision
BEFORE INSERT ON agent_chat_instruction_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat instruction');
END;
CREATE TRIGGER pr11_retired_update_agent_chat_instruction_revision
BEFORE UPDATE ON agent_chat_instruction_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat instruction');
END;
CREATE TRIGGER pr11_retired_insert_agent_chat_source_ref
BEFORE INSERT ON agent_chat_source_ref BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat source');
END;
CREATE TRIGGER pr11_retired_update_agent_chat_source_ref
BEFORE UPDATE ON agent_chat_source_ref BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent Chat source');
END;
CREATE TRIGGER pr11_retired_insert_agent_handoff
BEFORE INSERT ON agent_handoff BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Main-to-Project handoff');
END;
CREATE TRIGGER pr11_retired_update_agent_handoff
BEFORE UPDATE ON agent_handoff BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Main-to-Project handoff');
END;
CREATE TRIGGER pr11_retired_insert_agent_handoff_delivery
BEFORE INSERT ON agent_handoff_delivery BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Main-to-Project handoff');
END;
CREATE TRIGGER pr11_retired_update_agent_handoff_delivery
BEFORE UPDATE ON agent_handoff_delivery BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Main-to-Project handoff');
END;
CREATE TRIGGER pr11_retired_insert_product_genesis_session
BEFORE INSERT ON product_genesis_session BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Product Genesis');
END;
CREATE TRIGGER pr11_retired_update_product_genesis_session
BEFORE UPDATE ON product_genesis_session BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Product Genesis');
END;

CREATE TRIGGER pr11_retired_insert_agent_action
BEFORE INSERT ON agent_action BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: AgentAction');
END;
CREATE TRIGGER pr11_retired_update_agent_action
BEFORE UPDATE ON agent_action BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: AgentAction');
END;
CREATE TRIGGER pr11_retired_insert_agent_action_approval
BEFORE INSERT ON agent_action_approval BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: AgentAction');
END;
CREATE TRIGGER pr11_retired_update_agent_action_approval
BEFORE UPDATE ON agent_action_approval BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: AgentAction');
END;
CREATE TRIGGER pr11_retired_insert_agent_action_execution
BEFORE INSERT ON agent_action_execution BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: AgentAction');
END;
CREATE TRIGGER pr11_retired_update_agent_action_execution
BEFORE UPDATE ON agent_action_execution BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: AgentAction');
END;
CREATE TRIGGER pr11_retired_insert_agent_commitment
BEFORE INSERT ON agent_commitment BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent commitment');
END;
CREATE TRIGGER pr11_retired_update_agent_commitment
BEFORE UPDATE ON agent_commitment BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent commitment');
END;
CREATE TRIGGER pr11_retired_insert_agent_commitment_evidence
BEFORE INSERT ON agent_commitment_evidence BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent commitment');
END;
CREATE TRIGGER pr11_retired_update_agent_commitment_evidence
BEFORE UPDATE ON agent_commitment_evidence BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent commitment');
END;
CREATE TRIGGER pr11_retired_insert_agent_commitment_transfer
BEFORE INSERT ON agent_commitment_transfer BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent commitment');
END;
CREATE TRIGGER pr11_retired_update_agent_commitment_transfer
BEFORE UPDATE ON agent_commitment_transfer BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent commitment');
END;
CREATE TRIGGER pr11_retired_insert_agent_commitment_lifecycle
BEFORE INSERT ON agent_commitment_lifecycle BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent commitment');
END;
CREATE TRIGGER pr11_retired_update_agent_commitment_lifecycle
BEFORE UPDATE ON agent_commitment_lifecycle BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent commitment');
END;
CREATE TRIGGER pr11_retired_insert_agent_inbox_item
BEFORE INSERT ON agent_inbox_item BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent inbox');
END;
CREATE TRIGGER pr11_retired_update_agent_inbox_item
BEFORE UPDATE ON agent_inbox_item BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent inbox');
END;
CREATE TRIGGER pr11_retired_insert_agent_question
BEFORE INSERT ON agent_question BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent question');
END;
CREATE TRIGGER pr11_retired_update_agent_question
BEFORE UPDATE ON agent_question BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent question');
END;

CREATE TRIGGER pr11_retired_insert_memory_item
BEFORE INSERT ON memory_item BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_update_memory_item
BEFORE UPDATE ON memory_item BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_insert_memory_lifecycle_assertion
BEFORE INSERT ON memory_lifecycle_assertion BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_update_memory_lifecycle_assertion
BEFORE UPDATE ON memory_lifecycle_assertion BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_insert_forge_memory_source_binding
BEFORE INSERT ON forge_memory_source_binding BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_update_forge_memory_source_binding
BEFORE UPDATE ON forge_memory_source_binding BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_insert_agent_context_scope
BEFORE INSERT ON agent_context_scope BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_update_agent_context_scope
BEFORE UPDATE ON agent_context_scope BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_insert_context_manifest
BEFORE INSERT ON context_manifest BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_update_context_manifest
BEFORE UPDATE ON context_manifest BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_insert_context_manifest_source
BEFORE INSERT ON context_manifest_source BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_update_context_manifest_source
BEFORE UPDATE ON context_manifest_source BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_insert_agent_lcm_timeline
BEFORE INSERT ON agent_lcm_timeline BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_update_agent_lcm_timeline
BEFORE UPDATE ON agent_lcm_timeline BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_insert_agent_lcm_entry
BEFORE INSERT ON agent_lcm_entry BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_update_agent_lcm_entry
BEFORE UPDATE ON agent_lcm_entry BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_insert_agent_lcm_node
BEFORE INSERT ON agent_lcm_node BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_update_agent_lcm_node
BEFORE UPDATE ON agent_lcm_node BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_insert_agent_lcm_operation
BEFORE INSERT ON agent_lcm_operation BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_update_agent_lcm_operation
BEFORE UPDATE ON agent_lcm_operation BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent semantic memory');
END;
CREATE TRIGGER pr11_retired_insert_attention_projection
BEFORE INSERT ON attention_projection BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Attention projection');
END;
CREATE TRIGGER pr11_retired_update_attention_projection
BEFORE UPDATE ON attention_projection BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Attention projection');
END;
CREATE TRIGGER pr11_retired_insert_attention_consumer_health
BEFORE INSERT ON attention_consumer_health BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Attention projection');
END;
CREATE TRIGGER pr11_retired_update_attention_consumer_health
BEFORE UPDATE ON attention_consumer_health BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Attention projection');
END;
CREATE TRIGGER pr11_retired_insert_agent_wake_lease
BEFORE INSERT ON agent_wake_lease BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent wake');
END;
CREATE TRIGGER pr11_retired_update_agent_wake_lease
BEFORE UPDATE ON agent_wake_lease BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent wake');
END;
CREATE TRIGGER pr11_retired_insert_agent_wake_budget_window
BEFORE INSERT ON agent_wake_budget_window BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent wake');
END;
CREATE TRIGGER pr11_retired_update_agent_wake_budget_window
BEFORE UPDATE ON agent_wake_budget_window BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Agent wake');
END;

CREATE TRIGGER pr11_retired_insert_operating_skill
BEFORE INSERT ON operating_skill BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project operating skill');
END;
CREATE TRIGGER pr11_retired_update_operating_skill
BEFORE UPDATE ON operating_skill BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project operating skill');
END;
CREATE TRIGGER pr11_retired_insert_operating_skill_revision
BEFORE INSERT ON operating_skill_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project operating skill');
END;
CREATE TRIGGER pr11_retired_update_operating_skill_revision
BEFORE UPDATE ON operating_skill_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project operating skill');
END;

CREATE TRIGGER pr11_retired_insert_project_charter
BEFORE INSERT ON project_charter BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;
CREATE TRIGGER pr11_retired_update_project_charter
BEFORE UPDATE ON project_charter BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;
CREATE TRIGGER pr11_retired_insert_project_charter_revision
BEFORE INSERT ON project_charter_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;
CREATE TRIGGER pr11_retired_update_project_charter_revision
BEFORE UPDATE ON project_charter_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;
CREATE TRIGGER pr11_retired_insert_project_charter_approval
BEFORE INSERT ON project_charter_approval BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;
CREATE TRIGGER pr11_retired_update_project_charter_approval
BEFORE UPDATE ON project_charter_approval BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;
CREATE TRIGGER pr11_retired_insert_project_charter_approval_event
BEFORE INSERT ON project_charter_approval_event BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;
CREATE TRIGGER pr11_retired_update_project_charter_approval_event
BEFORE UPDATE ON project_charter_approval_event BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;
CREATE TRIGGER pr11_retired_insert_project_charter_amendment
BEFORE INSERT ON project_charter_amendment BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;
CREATE TRIGGER pr11_retired_update_project_charter_amendment
BEFORE UPDATE ON project_charter_amendment BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Charter');
END;

CREATE TRIGGER pr11_retired_insert_project_document
BEFORE INSERT ON project_document BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Document');
END;
CREATE TRIGGER pr11_retired_update_project_document
BEFORE UPDATE ON project_document BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Document');
END;
CREATE TRIGGER pr11_retired_insert_project_document_revision
BEFORE INSERT ON project_document_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Document');
END;
CREATE TRIGGER pr11_retired_update_project_document_revision
BEFORE UPDATE ON project_document_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Document');
END;
CREATE TRIGGER pr11_retired_insert_project_document_approval
BEFORE INSERT ON project_document_approval BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Document');
END;
CREATE TRIGGER pr11_retired_update_project_document_approval
BEFORE UPDATE ON project_document_approval BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Document');
END;

CREATE TRIGGER pr11_retired_insert_project_decision_candidate
BEFORE INSERT ON project_decision_candidate BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Decision');
END;
CREATE TRIGGER pr11_retired_update_project_decision_candidate
BEFORE UPDATE ON project_decision_candidate BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Decision');
END;
CREATE TRIGGER pr11_retired_insert_project_decision
BEFORE INSERT ON project_decision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Decision');
END;
CREATE TRIGGER pr11_retired_update_project_decision
BEFORE UPDATE ON project_decision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Decision');
END;
CREATE TRIGGER pr11_retired_insert_project_decision_link
BEFORE INSERT ON project_decision_link BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Decision');
END;
CREATE TRIGGER pr11_retired_update_project_decision_link
BEFORE UPDATE ON project_decision_link BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Decision');
END;

CREATE TRIGGER pr11_retired_insert_project_execution_baseline
BEFORE INSERT ON project_execution_baseline BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Execution Baseline');
END;
CREATE TRIGGER pr11_retired_update_project_execution_baseline
BEFORE UPDATE ON project_execution_baseline BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Execution Baseline');
END;
CREATE TRIGGER pr11_retired_insert_project_execution_baseline_revision
BEFORE INSERT ON project_execution_baseline_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Execution Baseline');
END;
CREATE TRIGGER pr11_retired_update_project_execution_baseline_revision
BEFORE UPDATE ON project_execution_baseline_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Execution Baseline');
END;
CREATE TRIGGER pr11_retired_insert_project_execution_baseline_approval
BEFORE INSERT ON project_execution_baseline_approval BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Execution Baseline');
END;
CREATE TRIGGER pr11_retired_update_project_execution_baseline_approval
BEFORE UPDATE ON project_execution_baseline_approval BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Execution Baseline');
END;
CREATE TRIGGER pr11_retired_insert_project_task_governance
BEFORE INSERT ON project_task_governance BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Task Governance');
END;
CREATE TRIGGER pr11_retired_update_project_task_governance
BEFORE UPDATE ON project_task_governance BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Task Governance');
END;

CREATE TRIGGER pr11_retired_insert_project_milestone
BEFORE INSERT ON project_milestone BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Milestone');
END;
CREATE TRIGGER pr11_retired_update_project_milestone
BEFORE UPDATE ON project_milestone BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Milestone');
END;
CREATE TRIGGER pr11_retired_insert_project_milestone_revision
BEFORE INSERT ON project_milestone_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Milestone');
END;
CREATE TRIGGER pr11_retired_update_project_milestone_revision
BEFORE UPDATE ON project_milestone_revision BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Milestone');
END;
CREATE TRIGGER pr11_retired_insert_project_milestone_check
BEFORE INSERT ON project_milestone_check BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Milestone');
END;
CREATE TRIGGER pr11_retired_update_project_milestone_check
BEFORE UPDATE ON project_milestone_check BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Milestone');
END;
CREATE TRIGGER pr11_retired_insert_project_milestone_check_result
BEFORE INSERT ON project_milestone_check_result BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Milestone');
END;
CREATE TRIGGER pr11_retired_update_project_milestone_check_result
BEFORE UPDATE ON project_milestone_check_result BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Milestone');
END;
CREATE TRIGGER pr11_retired_insert_project_readiness_snapshot
BEFORE INSERT ON project_readiness_snapshot BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Readiness');
END;
CREATE TRIGGER pr11_retired_update_project_readiness_snapshot
BEFORE UPDATE ON project_readiness_snapshot BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Readiness');
END;
CREATE TRIGGER pr11_retired_insert_project_readiness_input
BEFORE INSERT ON project_readiness_input BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Readiness');
END;
CREATE TRIGGER pr11_retired_update_project_readiness_input
BEFORE UPDATE ON project_readiness_input BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Readiness');
END;
CREATE TRIGGER pr11_retired_insert_project_release
BEFORE INSERT ON project_release BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Release');
END;
CREATE TRIGGER pr11_retired_update_project_release
BEFORE UPDATE ON project_release BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Release');
END;
CREATE TRIGGER pr11_retired_insert_project_release_reference
BEFORE INSERT ON project_release_reference BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Release');
END;
CREATE TRIGGER pr11_retired_update_project_release_reference
BEFORE UPDATE ON project_release_reference BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Release');
END;
CREATE TRIGGER pr11_retired_insert_project_release_media_pin
BEFORE INSERT ON project_release_media_pin BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Release media pin');
END;
CREATE TRIGGER pr11_retired_update_project_release_media_pin
BEFORE UPDATE ON project_release_media_pin BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Release media pin');
END;

-- Replace only the legacy Project OS admission predicates. WorkspaceLease
-- remains authoritative for exact Task, Actor, role membership, Execution,
-- repository, and capability-profile bindings. Charter, Project Agent, and
-- Project Task Governance no longer participate.
DROP TRIGGER IF EXISTS workspace_lease_scope_guard_insert;
CREATE TRIGGER workspace_lease_scope_guard_insert
BEFORE INSERT ON workspace_lease
WHEN NEW.status = 'active'
BEGIN
    SELECT CASE
        WHEN NEW.issuing_principal_type != 'system'
          OR NEW.issuing_principal_id != 'task-service-scheduler'
        THEN RAISE(ABORT, 'Workspace lease may only be issued by the scheduler')
        WHEN NOT EXISTS (
            SELECT 1
            FROM task t
            JOIN project p ON p.id = t.project_id
            JOIN repo r ON r.id = NEW.repository_binding_id
            JOIN execution e ON e.id = NEW.execution_id AND e.task_id = t.id
            JOIN task_role tr ON tr.task_id = t.id
            JOIN role_membership rm ON rm.task_role_id = tr.id
            WHERE t.id = NEW.task_id
              AND t.project_id = NEW.project_id
              AND t.version = NEW.task_version
              AND t.deleted_at IS NULL
              AND t.repo_id = NEW.repository_binding_id
              AND r.project_id = t.project_id
              AND e.status = 'running'
              AND e.actor_kind = 'agent'
              AND e.actor_id = NEW.assigned_principal_id
              AND e.agent_id = NEW.assigned_principal_id
              AND NEW.assigned_principal_type = 'agent'
              AND rm.actor_kind = e.actor_kind
              AND rm.actor_id = e.actor_id
              AND rm.status = 'active'
              AND tr.role = CASE lower(trim(e.role))
                  WHEN 'coder' THEN 'implementer'
                  WHEN 'worker' THEN 'implementer'
                  WHEN 'assignee' THEN 'implementer'
                  WHEN 'executor' THEN 'implementer'
                  ELSE lower(trim(e.role))
              END
              AND CASE NEW.role WHEN 'worker' THEN 'implementer'
                                ELSE lower(trim(NEW.role)) END = tr.role
              AND (
                  EXISTS (
                      SELECT 1 FROM agent_current a
                      WHERE a.id = e.actor_id
                        AND (
                            a.visibility = 'global'
                            OR (
                                a.visibility = 'account'
                                AND a.owner_id IS NOT NULL
                                AND (
                                    a.owner_id = p.owner_id
                                    OR EXISTS (
                                        SELECT 1 FROM project_member pm
                                        WHERE pm.project_id = p.id AND pm.user_id = a.owner_id
                                    )
                                )
                            )
                        )
                  )
              )
              AND json_valid(NEW.capabilities_json)
              AND json_array_length(NEW.capabilities_json) = 1
              AND json_extract(NEW.capabilities_json, '$[0]') =
                  CASE WHEN lower(trim(e.role)) = 'reviewer'
                             OR e.purpose IN ('plan', 'review', 'investigate', 'validate')
                             OR t.task_type IN ('planning', 'discovery', 'review', 'validation')
                       THEN 'repository_read' ELSE 'repository_write' END
              AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
              AND NEW.capability_profile_digest =
                  CASE json_extract(NEW.capabilities_json, '$[0]')
                      WHEN 'repository_read' THEN 'sha256:6035ec533a0bdb74c461ea9ea2d7147a2e47ba7c8b54c8b732052ceec23e8234'
                      WHEN 'repository_write' THEN 'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8'
                      ELSE ''
                  END
        ) THEN RAISE(ABORT, 'Workspace lease Task is stale or lacks current TaskRole/Execution authority')
    END;
END;

DROP TRIGGER IF EXISTS workspace_lease_active_renewal_guard;
CREATE TRIGGER workspace_lease_active_renewal_guard
BEFORE UPDATE ON workspace_lease
WHEN OLD.status = 'active' AND NEW.status = 'active'
BEGIN
    SELECT CASE
        WHEN NEW.expires_at <= OLD.expires_at OR NEW.updated_at IS OLD.updated_at
        THEN RAISE(ABORT, 'Workspace lease renewal must extend expiry')
        WHEN NOT EXISTS (
            SELECT 1
            FROM task t
            JOIN project p ON p.id = t.project_id
            JOIN repo r ON r.id = NEW.repository_binding_id
            JOIN execution e ON e.id = NEW.execution_id AND e.task_id = t.id
            JOIN task_role tr ON tr.task_id = t.id
            JOIN role_membership rm ON rm.task_role_id = tr.id
            WHERE t.id = NEW.task_id
              AND t.project_id = NEW.project_id
              AND t.deleted_at IS NULL
              AND t.repo_id = NEW.repository_binding_id
              AND r.project_id = t.project_id
              AND e.status = 'running'
              AND e.actor_kind = 'agent'
              AND e.actor_id = NEW.assigned_principal_id
              AND e.agent_id = NEW.assigned_principal_id
              AND NEW.assigned_principal_type = 'agent'
              AND rm.actor_kind = e.actor_kind
              AND rm.actor_id = e.actor_id
              AND rm.status = 'active'
              AND tr.role = CASE lower(trim(e.role))
                  WHEN 'coder' THEN 'implementer'
                  WHEN 'worker' THEN 'implementer'
                  WHEN 'assignee' THEN 'implementer'
                  WHEN 'executor' THEN 'implementer'
                  ELSE lower(trim(e.role))
              END
              AND CASE NEW.role WHEN 'worker' THEN 'implementer'
                                ELSE lower(trim(NEW.role)) END = tr.role
              AND EXISTS (
                  SELECT 1 FROM agent_current a
                  WHERE a.id = e.actor_id
                    AND (
                        a.visibility = 'global'
                        OR (
                            a.visibility = 'account'
                            AND a.owner_id IS NOT NULL
                            AND (
                                a.owner_id = p.owner_id
                                OR EXISTS (
                                    SELECT 1 FROM project_member pm
                                    WHERE pm.project_id = p.id AND pm.user_id = a.owner_id
                                )
                            )
                      )
                  )
              )
              AND json_valid(NEW.capabilities_json)
              AND json_array_length(NEW.capabilities_json) = 1
              AND json_extract(NEW.capabilities_json, '$[0]') =
                  CASE WHEN lower(trim(e.role)) = 'reviewer'
                             OR e.purpose IN ('plan', 'review', 'investigate', 'validate')
                             OR t.task_type IN ('planning', 'discovery', 'review', 'validation')
                       THEN 'repository_read' ELSE 'repository_write' END
              AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
              AND NEW.capability_profile_digest =
                  CASE json_extract(NEW.capabilities_json, '$[0]')
                      WHEN 'repository_read' THEN 'sha256:6035ec533a0bdb74c461ea9ea2d7147a2e47ba7c8b54c8b732052ceec23e8234'
                      WHEN 'repository_write' THEN 'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8'
                      ELSE ''
                  END
        ) THEN RAISE(ABORT, 'Workspace lease renewal lacks current TaskRole/Execution authority')
    END;
END;

DROP TABLE temp.pr11_binding_only_membership;
