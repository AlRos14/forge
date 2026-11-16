-- Phase 2 persistence of the coordinator-verification-and-pr-loop plan (A3):
-- the GitHub credential reference (D4), the frozen PR feedback policy (D5), the
-- immutable external feedback ledger and its triage, and PR external actions
-- (push_update / post_comment). Additive only: task_integration_operation and
-- credential_handle are not rebuilt (credential_handle.provider has no CHECK).
-- Applies by version, so it also applies on a database that already ran V174.

-- ---------------------------------------------------------------------------
-- Credential reference and feedback policy on the repo's PR provider config.
-- ---------------------------------------------------------------------------
ALTER TABLE pr_provider_config ADD COLUMN credential_handle_id TEXT
    REFERENCES credential_handle(id) ON DELETE RESTRICT;
ALTER TABLE pr_provider_config ADD COLUMN credential_owner_user_id TEXT
    REFERENCES user(id) ON DELETE RESTRICT;
ALTER TABLE pr_provider_config ADD COLUMN feedback_policy_json TEXT NOT NULL
    DEFAULT '{"schema_version":1,"ingest":false,"trusted_author_associations":["OWNER","MEMBER","COLLABORATOR"],"trusted_logins":[],"max_items_per_poll":50,"push_corrections":"human_approval","post_summary":"off"}'
    CHECK (json_valid(feedback_policy_json)
       AND json_extract(feedback_policy_json, '$.schema_version') = 1);

CREATE TRIGGER pr_provider_config_credential_guard_insert
BEFORE INSERT ON pr_provider_config
WHEN (NEW.credential_handle_id IS NULL) != (NEW.credential_owner_user_id IS NULL)
  OR (NEW.credential_handle_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM credential_handle h
        WHERE h.id = NEW.credential_handle_id
          AND h.owner_user_id = NEW.credential_owner_user_id
          AND h.provider = 'github' AND h.status = 'configured'))
BEGIN
    SELECT RAISE(ABORT, 'PR provider credential must be a configured github handle owned by the recorded user');
END;

CREATE TRIGGER pr_provider_config_credential_guard_update
BEFORE UPDATE OF credential_handle_id, credential_owner_user_id ON pr_provider_config
WHEN (NEW.credential_handle_id IS NULL) != (NEW.credential_owner_user_id IS NULL)
  OR ((NEW.credential_handle_id IS NOT OLD.credential_handle_id
       OR NEW.credential_owner_user_id IS NOT OLD.credential_owner_user_id)
      AND NEW.credential_handle_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM credential_handle h
        WHERE h.id = NEW.credential_handle_id
          AND h.owner_user_id = NEW.credential_owner_user_id
          AND h.provider = 'github' AND h.status = 'configured'))
BEGIN
    SELECT RAISE(ABORT, 'PR provider credential must be a configured github handle owned by the recorded user');
END;

-- Policy shape (schema_version 1): closed enums, bounded lists.
CREATE TRIGGER pr_provider_config_feedback_policy_guard_insert
BEFORE INSERT ON pr_provider_config
WHEN COALESCE(json_type(NEW.feedback_policy_json), '') != 'object'
  OR COALESCE(json_type(NEW.feedback_policy_json, '$.ingest'), '') NOT IN ('true', 'false')
  OR COALESCE(json_type(NEW.feedback_policy_json, '$.trusted_author_associations'), '') != 'array'
  OR COALESCE(json_type(NEW.feedback_policy_json, '$.trusted_logins'), '') != 'array'
  OR COALESCE(json_type(NEW.feedback_policy_json, '$.max_items_per_poll'), '') != 'integer'
  OR COALESCE(json_extract(NEW.feedback_policy_json, '$.max_items_per_poll'), 0) NOT BETWEEN 1 AND 200
  OR COALESCE(json_extract(NEW.feedback_policy_json, '$.push_corrections'), '') NOT IN ('human_approval', 'automatic')
  OR COALESCE(json_extract(NEW.feedback_policy_json, '$.post_summary'), '') NOT IN ('human_approval', 'automatic', 'off')
BEGIN
    SELECT RAISE(ABORT, 'invalid PR feedback policy');
END;

CREATE TRIGGER pr_provider_config_feedback_policy_guard_update
BEFORE UPDATE OF feedback_policy_json ON pr_provider_config
WHEN COALESCE(json_type(NEW.feedback_policy_json), '') != 'object'
  OR COALESCE(json_type(NEW.feedback_policy_json, '$.ingest'), '') NOT IN ('true', 'false')
  OR COALESCE(json_type(NEW.feedback_policy_json, '$.trusted_author_associations'), '') != 'array'
  OR COALESCE(json_type(NEW.feedback_policy_json, '$.trusted_logins'), '') != 'array'
  OR COALESCE(json_type(NEW.feedback_policy_json, '$.max_items_per_poll'), '') != 'integer'
  OR COALESCE(json_extract(NEW.feedback_policy_json, '$.max_items_per_poll'), 0) NOT BETWEEN 1 AND 200
  OR COALESCE(json_extract(NEW.feedback_policy_json, '$.push_corrections'), '') NOT IN ('human_approval', 'automatic')
  OR COALESCE(json_extract(NEW.feedback_policy_json, '$.post_summary'), '') NOT IN ('human_approval', 'automatic', 'off')
BEGIN
    SELECT RAISE(ABORT, 'invalid PR feedback policy');
END;

-- A referenced credential keeps its identity: the handle cannot change owner or
-- provider while a repo's PR provider config points at it.
CREATE TRIGGER credential_handle_scm_reference_guard
BEFORE UPDATE OF provider, owner_user_id ON credential_handle
WHEN (NEW.provider IS NOT OLD.provider OR NEW.owner_user_id IS NOT OLD.owner_user_id)
 AND (EXISTS (SELECT 1 FROM pr_provider_config c WHERE c.credential_handle_id = OLD.id)
      OR (NEW.provider = 'github' AND EXISTS (
            SELECT 1 FROM agent_profile p WHERE p.credential_ref = OLD.id)))
BEGIN
    SELECT RAISE(ABORT, 'SCM credential handle identity is fixed while it is referenced');
END;

-- An SCM credential never backs an Agent profile (and vice versa).
CREATE TRIGGER agent_profile_credential_not_scm_insert
BEFORE INSERT ON agent_profile
WHEN NEW.credential_ref IS NOT NULL AND EXISTS (
    SELECT 1 FROM credential_handle WHERE id = NEW.credential_ref AND provider = 'github')
BEGIN
    SELECT RAISE(ABORT, 'SCM credentials cannot back an Agent profile');
END;

CREATE TRIGGER agent_profile_credential_not_scm_update
BEFORE UPDATE OF credential_ref ON agent_profile
WHEN NEW.credential_ref IS NOT NULL AND EXISTS (
    SELECT 1 FROM credential_handle WHERE id = NEW.credential_ref AND provider = 'github')
BEGIN
    SELECT RAISE(ABORT, 'SCM credentials cannot back an Agent profile');
END;

-- ---------------------------------------------------------------------------
-- Frozen credential reference and policy on the remote PR admission. The new
-- columns are NULL on historical admissions. remote_pr_history (V116) is not
-- widened: it keeps a RESTRICT foreign key to its admission, which retains
-- these columns immutably, so a snapshot never needs its own copy.
-- ---------------------------------------------------------------------------
ALTER TABLE remote_pr_admission ADD COLUMN credential_handle_id TEXT;
ALTER TABLE remote_pr_admission ADD COLUMN credential_owner_user_id TEXT;
ALTER TABLE remote_pr_admission ADD COLUMN feedback_policy_json TEXT;
ALTER TABLE remote_pr_admission ADD COLUMN feedback_policy_digest TEXT
    CHECK (feedback_policy_digest IS NULL OR length(feedback_policy_digest) = 64);

DROP TRIGGER remote_pr_admission_insert_guard;
CREATE TRIGGER remote_pr_admission_insert_guard
BEFORE INSERT ON remote_pr_admission
WHEN NOT EXISTS (
    SELECT 1 FROM task_integration_operation merge_op
    JOIN task_integration_operation publish_op
      ON publish_op.id = NEW.publish_operation_id
     AND publish_op.parent_operation_id = merge_op.id
     AND publish_op.task_id = merge_op.task_id
    JOIN pr_metadata metadata
      ON metadata.id = NEW.metadata_id AND metadata.task_id = merge_op.task_id
    JOIN task t ON t.id = merge_op.task_id
    JOIN repo r ON r.id = t.repo_id
    JOIN pr_provider_config config
      ON config.id = NEW.provider_config_id AND config.repo_id = r.id
    JOIN task_lifecycle lifecycle ON lifecycle.task_id = merge_op.task_id
    WHERE merge_op.id = NEW.task_merge_operation_id
      AND merge_op.task_id = NEW.task_id
      AND merge_op.kind = 'task_merge' AND merge_op.status = 'running'
      AND merge_op.remote_waiting = 1
      AND publish_op.kind = 'publish_pr' AND publish_op.status = 'running'
      AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
      AND metadata.task_merge_operation_id = merge_op.id
      AND metadata.publish_operation_id = publish_op.id
      AND metadata.provider_type = NEW.provider_type
      AND metadata.source_branch = NEW.source_branch
      AND metadata.target_branch = NEW.target_branch
      AND r.remote_url = NEW.remote_repo_identity
      AND config.provider_type = NEW.provider_type
      AND config.updated_at = NEW.provider_config_revision
      AND config.base_url IS NEW.provider_base_url
      AND config.token_secret_ref IS NEW.token_secret_ref
      AND config.credential_handle_id IS NEW.credential_handle_id
      AND config.credential_owner_user_id IS NEW.credential_owner_user_id
      AND config.feedback_policy_json IS NEW.feedback_policy_json
      AND (NEW.feedback_policy_json IS NULL) = (NEW.feedback_policy_digest IS NULL)
      AND lifecycle.state = 'merging' AND lifecycle.reason_ref = merge_op.id
)
BEGIN
    SELECT RAISE(ABORT, 'remote PR admission is missing its exact TaskMerge, PublishPr, or metadata authority');
END;

DROP TRIGGER remote_pr_admission_identity_immutable;
CREATE TRIGGER remote_pr_admission_identity_immutable
BEFORE UPDATE ON remote_pr_admission
WHEN NEW.task_merge_operation_id IS NOT OLD.task_merge_operation_id
  OR NEW.publish_operation_id IS NOT OLD.publish_operation_id
  OR NEW.metadata_id IS NOT OLD.metadata_id
  OR NEW.task_id IS NOT OLD.task_id
  OR NEW.provider_config_id IS NOT OLD.provider_config_id
  OR NEW.provider_type IS NOT OLD.provider_type
  OR NEW.provider_config_revision IS NOT OLD.provider_config_revision
  OR NEW.provider_config_digest IS NOT OLD.provider_config_digest
  OR NEW.provider_base_url IS NOT OLD.provider_base_url
  OR NEW.token_secret_ref IS NOT OLD.token_secret_ref
  OR NEW.remote_repo_identity IS NOT OLD.remote_repo_identity
  OR NEW.source_branch IS NOT OLD.source_branch
  OR NEW.target_branch IS NOT OLD.target_branch
  OR NEW.admitted_source_sha IS NOT OLD.admitted_source_sha
  OR NEW.credential_handle_id IS NOT OLD.credential_handle_id
  OR NEW.credential_owner_user_id IS NOT OLD.credential_owner_user_id
  OR NEW.feedback_policy_json IS NOT OLD.feedback_policy_json
  OR NEW.feedback_policy_digest IS NOT OLD.feedback_policy_digest
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'remote PR admission subject and provider identity are immutable');
END;

-- ---------------------------------------------------------------------------
-- Immutable external feedback ledger. External content is untrusted data; a row
-- is evidence of what the provider said, never an instruction.
-- ---------------------------------------------------------------------------
CREATE TABLE external_feedback_item (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    admission_id TEXT NOT NULL
        REFERENCES remote_pr_admission(task_merge_operation_id) ON DELETE RESTRICT,
    provider_item_id TEXT NOT NULL CHECK (length(trim(provider_item_id)) BETWEEN 1 AND 128),
    kind TEXT NOT NULL CHECK (kind IN ('review', 'review_comment', 'issue_comment')),
    author_login TEXT NOT NULL CHECK (length(author_login) BETWEEN 1 AND 128),
    author_association TEXT NOT NULL CHECK (length(author_association) <= 32),
    review_state TEXT CHECK (review_state IN
        ('CHANGES_REQUESTED', 'COMMENTED', 'APPROVED', 'DISMISSED')),
    path TEXT CHECK (path IS NULL OR length(path) <= 1024),
    line INTEGER,
    commit_sha TEXT CHECK (commit_sha IS NULL OR length(commit_sha) BETWEEN 7 AND 128),
    body TEXT NOT NULL CHECK (length(body) <= 16384),
    body_truncated INTEGER NOT NULL DEFAULT 0 CHECK (body_truncated IN (0, 1)),
    body_digest TEXT NOT NULL CHECK (length(body_digest) = 64),
    provider_url TEXT CHECK (provider_url IS NULL OR length(provider_url) <= 2048),
    trust_class TEXT NOT NULL CHECK (trust_class IN ('trusted', 'untrusted', 'forge_authored')),
    head_state TEXT NOT NULL CHECK (head_state IN ('current', 'stale_head')),
    ingested_at TEXT NOT NULL,
    UNIQUE(admission_id, provider_item_id, body_digest),
    UNIQUE(id, task_id)
);
CREATE INDEX idx_external_feedback_item_admission
    ON external_feedback_item(admission_id, ingested_at, id);
CREATE INDEX idx_external_feedback_item_task
    ON external_feedback_item(task_id, ingested_at DESC, id DESC);

-- Ingestion is allowed only for the Task's own open admission whose FROZEN
-- policy enabled it; changing the repo policy afterwards never widens this.
CREATE TRIGGER external_feedback_item_insert_guard
BEFORE INSERT ON external_feedback_item
WHEN NOT EXISTS (
    SELECT 1 FROM remote_pr_admission a
    WHERE a.task_merge_operation_id = NEW.admission_id
      AND a.task_id = NEW.task_id
      AND a.state = 'open'
      AND a.feedback_policy_json IS NOT NULL
      AND json_extract(a.feedback_policy_json, '$.ingest') = 1
)
BEGIN
    SELECT RAISE(ABORT, 'external feedback requires the Task''s open admission with ingestion enabled in its frozen policy');
END;

-- Immutable except the one-way head marker (a later push or a new head makes
-- earlier comments stale; nothing returns to current).
CREATE TRIGGER external_feedback_item_immutable
BEFORE UPDATE ON external_feedback_item
WHEN NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.admission_id IS NOT OLD.admission_id
  OR NEW.provider_item_id IS NOT OLD.provider_item_id
  OR NEW.kind IS NOT OLD.kind OR NEW.author_login IS NOT OLD.author_login
  OR NEW.author_association IS NOT OLD.author_association
  OR NEW.review_state IS NOT OLD.review_state OR NEW.path IS NOT OLD.path
  OR NEW.line IS NOT OLD.line OR NEW.commit_sha IS NOT OLD.commit_sha
  OR NEW.body IS NOT OLD.body OR NEW.body_truncated IS NOT OLD.body_truncated
  OR NEW.body_digest IS NOT OLD.body_digest OR NEW.provider_url IS NOT OLD.provider_url
  OR NEW.trust_class IS NOT OLD.trust_class OR NEW.ingested_at IS NOT OLD.ingested_at
  OR NOT (NEW.head_state = OLD.head_state
          OR (OLD.head_state = 'current' AND NEW.head_state = 'stale_head'))
BEGIN
    SELECT RAISE(ABORT, 'external feedback is immutable evidence');
END;

CREATE TRIGGER external_feedback_item_immutable_delete
BEFORE DELETE ON external_feedback_item
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g
                 ON g.project_id = t.project_id WHERE t.id = OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'external feedback is preserved outside Project teardown');
END;

-- The exact, bounded (<= 20) set of trusted items one external_feedback request
-- triages. source_subject_id of that request is the admission id and
-- source_subject_digest the digest of this set (computed by the service).
CREATE TABLE task_external_feedback_set_item (
    request_id TEXT NOT NULL REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    feedback_item_id TEXT NOT NULL,
    body_digest TEXT NOT NULL CHECK (length(body_digest) = 64),
    position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 19),
    PRIMARY KEY (request_id, feedback_item_id),
    UNIQUE (request_id, position),
    FOREIGN KEY (feedback_item_id, task_id)
        REFERENCES external_feedback_item(id, task_id) ON DELETE RESTRICT
);

CREATE TRIGGER task_external_feedback_set_item_guard
BEFORE INSERT ON task_external_feedback_set_item
WHEN NOT EXISTS (
    SELECT 1 FROM task_coordination_request r
    JOIN external_feedback_item i ON i.id = NEW.feedback_item_id AND i.task_id = r.task_id
    WHERE r.id = NEW.request_id AND r.task_id = NEW.task_id
      AND r.kind = 'external_feedback' AND r.state = 'requested'
      AND r.source_subject_kind = 'external_feedback_set'
      AND r.source_subject_id = i.admission_id
      AND i.trust_class = 'trusted' AND i.head_state = 'current'
      AND i.body_digest = NEW.body_digest
)
BEGIN
    SELECT RAISE(ABORT, 'FORGE_COORDINATION_EXTERNAL_FEEDBACK_STALE');
END;

CREATE TRIGGER task_external_feedback_set_item_immutable
BEFORE UPDATE ON task_external_feedback_set_item
BEGIN
    SELECT RAISE(ABORT, 'external feedback set membership is immutable');
END;

CREATE TRIGGER task_external_feedback_set_item_immutable_delete
BEFORE DELETE ON task_external_feedback_set_item
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g
                 ON g.project_id = t.project_id WHERE t.id = OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'external feedback set membership is preserved outside Project teardown');
END;

-- One immutable coordinator decision per item of the request.
CREATE TABLE task_external_feedback_triage (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    request_id TEXT NOT NULL REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    feedback_item_id TEXT NOT NULL,
    body_digest TEXT NOT NULL CHECK (length(body_digest) = 64),
    disposition TEXT NOT NULL CHECK (disposition IN ('correct', 'recheck', 'human', 'dismiss')),
    rationale TEXT NOT NULL,
    instructions TEXT NOT NULL,
    decision_digest TEXT NOT NULL CHECK (length(decision_digest) = 64),
    execution_id TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    UNIQUE(request_id, feedback_item_id),
    FOREIGN KEY (feedback_item_id, task_id)
        REFERENCES external_feedback_item(id, task_id) ON DELETE RESTRICT
);
CREATE INDEX idx_task_external_feedback_triage_item
    ON task_external_feedback_triage(feedback_item_id, created_at);

CREATE TRIGGER task_external_feedback_triage_current_request_guard
BEFORE INSERT ON task_external_feedback_triage
WHEN NOT EXISTS (
    SELECT 1 FROM task_coordination_request request
    JOIN task_coordination c ON c.task_id = request.task_id
    JOIN task_external_feedback_set_item s
      ON s.request_id = request.id AND s.task_id = request.task_id
     AND s.feedback_item_id = NEW.feedback_item_id AND s.body_digest = NEW.body_digest
    WHERE request.id = NEW.request_id AND request.kind = 'external_feedback'
      AND request.task_id = NEW.task_id AND request.execution_id = NEW.execution_id
      AND request.state IN ('running', 'uncertain')
      AND c.authorization_version = request.authorization_version
      AND c.revoked_at IS NULL AND c.team_stale = 0 AND c.mode = request.mode
)
BEGIN
    SELECT RAISE(ABORT, 'FORGE_COORDINATION_EXTERNAL_FEEDBACK_STALE');
END;

CREATE TRIGGER task_external_feedback_triage_immutable
BEFORE UPDATE ON task_external_feedback_triage
BEGIN
    SELECT RAISE(ABORT, 'external feedback triage is immutable evidence');
END;

CREATE TRIGGER task_external_feedback_triage_immutable_delete
BEFORE DELETE ON task_external_feedback_triage
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g
                 ON g.project_id = t.project_id WHERE t.id = OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'external feedback triage is preserved outside Project teardown');
END;

-- ---------------------------------------------------------------------------
-- external_feedback coordination requests: source guard and triaged guard.
-- (V172 guards kind/origin/state; V173 owns the exact source of this kind.)
-- ---------------------------------------------------------------------------
CREATE TRIGGER task_coordination_request_external_feedback_source_guard
BEFORE INSERT ON task_coordination_request
WHEN NEW.kind = 'external_feedback' AND NOT EXISTS (
    SELECT 1 FROM remote_pr_admission a
    WHERE a.task_merge_operation_id = NEW.source_subject_id
      AND a.task_id = NEW.task_id
      AND a.state = 'open'
      AND a.feedback_policy_json IS NOT NULL
      AND json_extract(a.feedback_policy_json, '$.ingest') = 1
)
BEGIN
    SELECT RAISE(ABORT, 'FORGE_COORDINATION_EXTERNAL_FEEDBACK_STALE');
END;

DROP TRIGGER task_coordination_request_triaged_guard;
CREATE TRIGGER task_coordination_request_triaged_guard
 BEFORE UPDATE OF state ON task_coordination_request WHEN NEW.state='triaged'
 BEGIN
  SELECT CASE
   WHEN NEW.kind='review_findings' AND NOT EXISTS(
    SELECT 1 FROM task_review_triage triage WHERE triage.task_id=NEW.task_id
     AND triage.review_report_artifact_id=NEW.review_report_artifact_id
     AND triage.report_digest=NEW.review_report_digest AND triage.execution_id=NEW.execution_id
     AND triage.authorization_version=NEW.authorization_version)
   THEN RAISE(ABORT,'FORGE_COORDINATION_REVIEW_DECISION_STALE')
   WHEN NEW.kind='validation_findings' AND NOT EXISTS(
    SELECT 1 FROM task_validation_triage triage WHERE triage.request_id=NEW.id
     AND triage.task_id=NEW.task_id AND triage.execution_id=NEW.execution_id
     AND triage.authorization_version=NEW.authorization_version)
   THEN RAISE(ABORT,'FORGE_COORDINATION_VALIDATION_DECISION_STALE')
   WHEN NEW.kind='external_feedback' AND (
    NOT EXISTS(SELECT 1 FROM task_external_feedback_set_item s WHERE s.request_id=NEW.id)
    OR EXISTS(SELECT 1 FROM task_external_feedback_set_item s WHERE s.request_id=NEW.id
      AND NOT EXISTS(SELECT 1 FROM task_external_feedback_triage t
        WHERE t.request_id=NEW.id AND t.task_id=NEW.task_id
         AND t.feedback_item_id=s.feedback_item_id AND t.body_digest=s.body_digest
         AND t.execution_id=NEW.execution_id)))
   THEN RAISE(ABORT,'FORGE_COORDINATION_EXTERNAL_FEEDBACK_STALE')
   WHEN NEW.kind='work_unit_plan' OR NEW.execution_id IS NULL
   THEN RAISE(ABORT,'FORGE_COORDINATION_DECISION_STALE')
  END;
 END;

-- ---------------------------------------------------------------------------
-- PR external actions (push_update / post_comment). A separate table instead
-- of widening task_integration_operation.kind (see plan, Decision N0).
-- ---------------------------------------------------------------------------
CREATE TABLE pr_external_action (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    admission_id TEXT NOT NULL
        REFERENCES remote_pr_admission(task_merge_operation_id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('push_update', 'post_comment')),
    state TEXT NOT NULL CHECK (state IN ('proposed', 'approved', 'running', 'succeeded',
        'failed', 'outcome_unknown', 'reconciliation_required', 'rejected', 'abandoned')),
    authorization_mode TEXT NOT NULL CHECK (authorization_mode IN ('human_approval', 'automatic')),
    gate_evaluation_id TEXT REFERENCES gate_evaluation(id) ON DELETE RESTRICT,
    subject_commit_sha TEXT,
    expected_remote_head_sha TEXT,
    summary_artifact_id TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    body_digest TEXT CHECK (body_digest IS NULL OR length(body_digest) = 64),
    proposal_id TEXT REFERENCES proposal(id) ON DELETE RESTRICT,
    decision_id TEXT REFERENCES decision(id) ON DELETE RESTRICT,
    idempotency_key TEXT NOT NULL UNIQUE CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    provider_event_id TEXT,
    provider_result_json TEXT CHECK (provider_result_json IS NULL OR json_valid(provider_result_json)),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    finished_at TEXT,
    CHECK (kind != 'push_update' OR (gate_evaluation_id IS NOT NULL
        AND subject_commit_sha IS NOT NULL AND expected_remote_head_sha IS NOT NULL)),
    CHECK (kind != 'post_comment' OR (summary_artifact_id IS NOT NULL AND body_digest IS NOT NULL)),
    CHECK (authorization_mode = 'automatic'
        OR state IN ('proposed', 'rejected', 'abandoned') OR decision_id IS NOT NULL),
    CHECK ((state IN ('succeeded', 'failed', 'rejected', 'abandoned')) = (finished_at IS NOT NULL))
);
CREATE UNIQUE INDEX idx_pr_external_action_open ON pr_external_action(task_id)
    WHERE state IN ('proposed', 'approved', 'running', 'outcome_unknown', 'reconciliation_required');
CREATE INDEX idx_pr_external_action_task
    ON pr_external_action(task_id, created_at DESC, id DESC);
CREATE INDEX idx_pr_external_action_admission
    ON pr_external_action(admission_id, created_at, id);

-- Birth: the Task's own open admission; `automatic` only when the FROZEN policy
-- says so; no integration operation other than the waiting TaskMerge running.
CREATE TRIGGER pr_external_action_insert_guard
BEFORE INSERT ON pr_external_action
WHEN NEW.version != 1 OR NEW.finished_at IS NOT NULL
  OR NEW.proposal_id IS NOT NULL OR NEW.decision_id IS NOT NULL
  OR NEW.state NOT IN ('proposed', 'approved')
  OR (NEW.state = 'approved' AND NEW.authorization_mode != 'automatic')
  OR NOT EXISTS (
      SELECT 1 FROM remote_pr_admission a
      WHERE a.task_merge_operation_id = NEW.admission_id
        AND a.task_id = NEW.task_id AND a.state = 'open'
        AND a.feedback_policy_json IS NOT NULL
        AND (NEW.authorization_mode != 'automatic'
             OR json_extract(a.feedback_policy_json,
                  CASE NEW.kind WHEN 'push_update' THEN '$.push_corrections'
                                ELSE '$.post_summary' END) = 'automatic')
        AND (NEW.kind != 'post_comment'
             OR json_extract(a.feedback_policy_json, '$.post_summary') != 'off'))
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_ACTION_NOT_AUTHORIZED');
END;

CREATE TRIGGER pr_external_action_scope_guard
BEFORE INSERT ON pr_external_action
WHEN EXISTS (SELECT 1 FROM task_integration_operation op
             WHERE op.task_id = NEW.task_id AND op.status = 'running'
               AND op.id != NEW.admission_id)
  OR (NEW.kind = 'push_update' AND NOT EXISTS (
        SELECT 1 FROM gate_evaluation e
        JOIN gate_evaluation_currentness currentness ON currentness.evaluation_id = e.id
        WHERE e.id = NEW.gate_evaluation_id AND e.task_id = NEW.task_id
          AND e.outcome = 'satisfied' AND currentness.inputs_current = 1))
  OR (NEW.kind = 'post_comment' AND NOT EXISTS (
        SELECT 1 FROM artifact a
        WHERE a.id = NEW.summary_artifact_id AND a.task_id = NEW.task_id))
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_ACTION_NOT_AUTHORIZED');
END;

CREATE TRIGGER pr_external_action_update_guard
BEFORE UPDATE ON pr_external_action
WHEN NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.admission_id IS NOT OLD.admission_id OR NEW.kind IS NOT OLD.kind
  OR NEW.authorization_mode IS NOT OLD.authorization_mode
  OR NEW.gate_evaluation_id IS NOT OLD.gate_evaluation_id
  OR NEW.subject_commit_sha IS NOT OLD.subject_commit_sha
  OR NEW.expected_remote_head_sha IS NOT OLD.expected_remote_head_sha
  OR NEW.summary_artifact_id IS NOT OLD.summary_artifact_id
  OR NEW.body_digest IS NOT OLD.body_digest
  OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.created_at IS NOT OLD.created_at
  OR (OLD.proposal_id IS NOT NULL AND NEW.proposal_id IS NOT OLD.proposal_id)
  OR (OLD.decision_id IS NOT NULL AND NEW.decision_id IS NOT OLD.decision_id)
BEGIN
    SELECT RAISE(ABORT, 'PR external action identity is immutable');
END;

CREATE TRIGGER pr_external_action_transition_guard
BEFORE UPDATE ON pr_external_action
WHEN OLD.state IN ('succeeded', 'failed', 'rejected', 'abandoned')
  OR NEW.version != OLD.version + 1
  OR (NEW.state != OLD.state AND (OLD.state, NEW.state) NOT IN (VALUES
        ('proposed', 'approved'), ('proposed', 'rejected'), ('proposed', 'abandoned'),
        ('approved', 'running'), ('approved', 'abandoned'),
        ('running', 'succeeded'), ('running', 'failed'),
        ('running', 'outcome_unknown'), ('running', 'reconciliation_required'),
        ('outcome_unknown', 'succeeded'), ('outcome_unknown', 'failed'),
        ('outcome_unknown', 'reconciliation_required'),
        ('reconciliation_required', 'succeeded'), ('reconciliation_required', 'failed'),
        ('reconciliation_required', 'abandoned')))
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_ACTION_STALE');
END;

-- A Proposal is attached once, while proposed, and must target this action.
CREATE TRIGGER pr_external_action_proposal_guard
BEFORE UPDATE OF proposal_id ON pr_external_action
WHEN NEW.proposal_id IS NOT NULL AND NEW.proposal_id IS NOT OLD.proposal_id
 AND (OLD.state != 'proposed' OR NOT EXISTS (
      SELECT 1 FROM proposal p
      WHERE p.id = NEW.proposal_id AND p.task_id = NEW.task_id
        AND p.target_kind = 'pr_external_action' AND p.target_id = NEW.id))
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_ACTION_NOT_AUTHORIZED');
END;

-- A human approval/rejection is the exact Decision on this action's Proposal.
CREATE TRIGGER pr_external_action_decision_guard
BEFORE UPDATE OF decision_id ON pr_external_action
WHEN NEW.decision_id IS NOT NULL AND NEW.decision_id IS NOT OLD.decision_id
 AND (OLD.state != 'proposed' OR NEW.state NOT IN ('approved', 'rejected')
      OR NOT EXISTS (
        SELECT 1 FROM decision d
        WHERE d.id = NEW.decision_id AND d.task_id = NEW.task_id
          AND d.proposal_id = NEW.proposal_id
          AND d.outcome = CASE NEW.state WHEN 'approved' THEN 'approve' ELSE 'reject' END))
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_ACTION_NOT_AUTHORIZED');
END;

-- Starting the provider effect re-checks the admission, exclusion and exact Gate.
CREATE TRIGGER pr_external_action_start_guard
BEFORE UPDATE OF state ON pr_external_action
WHEN NEW.state = 'running' AND OLD.state != 'running'
 AND (NOT EXISTS (
        SELECT 1 FROM remote_pr_admission a
        WHERE a.task_merge_operation_id = NEW.admission_id
          AND a.task_id = NEW.task_id AND a.state = 'open')
   OR EXISTS (SELECT 1 FROM task_integration_operation op
              WHERE op.task_id = NEW.task_id AND op.status = 'running'
                AND op.id != NEW.admission_id)
   OR (NEW.kind = 'push_update' AND NOT EXISTS (
        SELECT 1 FROM gate_evaluation e
        JOIN gate_evaluation_currentness currentness ON currentness.evaluation_id = e.id
        WHERE e.id = NEW.gate_evaluation_id AND e.task_id = NEW.task_id
          AND e.outcome = 'satisfied' AND currentness.inputs_current = 1)))
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_ACTION_NOT_AUTHORIZED');
END;

CREATE TRIGGER pr_external_action_immutable_delete
BEFORE DELETE ON pr_external_action
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g
                 ON g.project_id = t.project_id WHERE t.id = OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'PR external action history is preserved outside Project teardown');
END;
