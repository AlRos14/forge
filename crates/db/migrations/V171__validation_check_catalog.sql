-- Versioned Project/Repo validation check catalog, queued check requests and
-- exact base-vs-candidate comparison (plan: coordinator-verification-and-pr-loop,
-- Congelado 2, A1). Additive only: no existing table is rebuilt.
--
-- Decision C2 (lead): the disposable baseline workspace is a runner-owned
-- detached git worktree with NO `workspace` row and NO `workspace_scope` row.
-- A baseline therefore has no `validation_run` row (that table requires a
-- `workspace` FK). The comparison row records the base commit, its snapshot
-- digest, the baseline result facts and runner-local provenance directly.

CREATE TABLE validation_check_definition (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id           TEXT REFERENCES repo(id) ON DELETE CASCADE,
    check_key         TEXT NOT NULL CHECK (length(check_key) BETWEEN 1 AND 64
                          AND check_key NOT GLOB '*[^a-z0-9_-]*'),
    version           INTEGER NOT NULL CHECK (version >= 1),
    command           TEXT NOT NULL CHECK (length(trim(command)) BETWEEN 1 AND 8192),
    timeout_seconds   INTEGER NOT NULL CHECK (timeout_seconds BETWEEN 1 AND 3600),
    env_policy        TEXT NOT NULL CHECK (env_policy IN ('captured_default')),
    definition_digest TEXT NOT NULL CHECK (length(definition_digest) = 64),
    state             TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'retired')),
    created_by        TEXT NOT NULL REFERENCES user(id) ON DELETE RESTRICT,
    created_at        TEXT NOT NULL,
    retired_at        TEXT,
    UNIQUE(id, definition_digest),
    UNIQUE(id, project_id),
    CHECK ((state = 'active') = (retired_at IS NULL))
);
CREATE UNIQUE INDEX idx_vcd_version ON validation_check_definition
    (project_id, COALESCE(repo_id, ''), check_key, version);
CREATE UNIQUE INDEX idx_vcd_one_active ON validation_check_definition
    (project_id, COALESCE(repo_id, ''), check_key) WHERE state = 'active';

CREATE TRIGGER validation_check_definition_insert_guard
BEFORE INSERT ON validation_check_definition
BEGIN
    SELECT CASE
        WHEN NEW.state != 'active' THEN
            RAISE(ABORT, 'ValidationCheckDefinition must be created active')
        WHEN NEW.repo_id IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM repo r WHERE r.id = NEW.repo_id AND r.project_id = NEW.project_id)
            THEN RAISE(ABORT, 'ValidationCheckDefinition repo must belong to its Project')
        WHEN NOT EXISTS (
            SELECT 1 FROM project_member m
            WHERE m.project_id = NEW.project_id AND m.user_id = NEW.created_by
              AND m.role IN ('owner', 'admin'))
            THEN RAISE(ABORT, 'ValidationCheckDefinition author must be a Project owner or admin')
        WHEN NEW.version != COALESCE((
            SELECT MAX(d.version) FROM validation_check_definition d
            WHERE d.project_id = NEW.project_id
              AND COALESCE(d.repo_id, '') = COALESCE(NEW.repo_id, '')
              AND d.check_key = NEW.check_key), 0) + 1
            THEN RAISE(ABORT, 'ValidationCheckDefinition version must be the next exact version')
    END;
END;
CREATE TRIGGER validation_check_definition_immutable
BEFORE UPDATE ON validation_check_definition
WHEN NOT (OLD.state = 'active' AND NEW.state = 'retired' AND NEW.retired_at IS NOT NULL
    AND NEW.id IS OLD.id AND NEW.project_id IS OLD.project_id AND NEW.repo_id IS OLD.repo_id
    AND NEW.check_key IS OLD.check_key AND NEW.version IS OLD.version
    AND NEW.command IS OLD.command AND NEW.timeout_seconds IS OLD.timeout_seconds
    AND NEW.env_policy IS OLD.env_policy AND NEW.definition_digest IS OLD.definition_digest
    AND NEW.created_by IS OLD.created_by AND NEW.created_at IS OLD.created_at)
BEGIN
    SELECT RAISE(ABORT, 'ValidationCheckDefinition is immutable except active to retired');
END;
CREATE TRIGGER validation_check_definition_immutable_delete
BEFORE DELETE ON validation_check_definition
WHEN NOT EXISTS (SELECT 1 FROM project_deletion_guard g WHERE g.project_id = OLD.project_id)
BEGIN
    SELECT RAISE(ABORT, 'ValidationCheckDefinition history is preserved outside Project teardown');
END;

CREATE TABLE validation_check_request (
    id                        TEXT PRIMARY KEY,
    task_id                   TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    authorization_version     INTEGER NOT NULL CHECK (authorization_version >= 1),
    check_definition_id       TEXT NOT NULL,
    definition_digest         TEXT NOT NULL CHECK (length(definition_digest) = 64),
    trigger_kind              TEXT NOT NULL CHECK (trigger_kind IN ('coordinator_request', 'after_integration')),
    subject_kind              TEXT NOT NULL CHECK (subject_kind IN ('work_unit_result', 'task_integration')),
    subject_id                TEXT NOT NULL CHECK (length(trim(subject_id)) > 0),
    subject_version           INTEGER NOT NULL CHECK (subject_version >= 1),
    workspace_id              TEXT NOT NULL,
    commit_sha                TEXT NOT NULL CHECK (length(trim(commit_sha)) BETWEEN 7 AND 128),
    workspace_snapshot_digest TEXT NOT NULL CHECK (length(workspace_snapshot_digest) = 64),
    baseline                  INTEGER NOT NULL CHECK (baseline IN (0, 1)),
    -- Resolved exact base commit of the baseline comparison (runner worktree is
    -- not a workspace row); set once while the request is still open.
    baseline_commit_sha       TEXT CHECK (baseline_commit_sha IS NULL
                                  OR length(trim(baseline_commit_sha)) BETWEEN 7 AND 128),
    requested_by_request_id   TEXT REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    requested_by_execution_id TEXT,
    reason                    TEXT CHECK (reason IS NULL OR length(reason) <= 1024),
    idempotency_key           TEXT NOT NULL UNIQUE
                                  CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    state                     TEXT NOT NULL DEFAULT 'queued' CHECK (state IN
                                  ('queued', 'running', 'completed', 'cancelled', 'failed_to_start')),
    candidate_run_id          TEXT,
    failure_code              TEXT CHECK (failure_code IS NULL OR length(trim(failure_code)) BETWEEN 1 AND 128),
    version                   INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL,
    completed_at              TEXT,
    UNIQUE(id, task_id),
    FOREIGN KEY (check_definition_id, definition_digest)
        REFERENCES validation_check_definition(id, definition_digest) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, task_id) REFERENCES workspace(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (requested_by_execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (candidate_run_id, task_id)
        REFERENCES validation_run(id, task_id) ON DELETE RESTRICT,
    CHECK ((trigger_kind = 'coordinator_request') = (requested_by_request_id IS NOT NULL)),
    CHECK (baseline = 1 OR baseline_commit_sha IS NULL),
    CHECK ((state IN ('queued', 'running')) = (completed_at IS NULL)),
    CHECK (state != 'completed' OR candidate_run_id IS NOT NULL),
    CHECK ((state = 'failed_to_start') = (failure_code IS NOT NULL))
);
CREATE INDEX idx_validation_check_request_task
    ON validation_check_request(task_id, created_at DESC, id DESC);
CREATE INDEX idx_validation_check_request_queue
    ON validation_check_request(state, created_at, id) WHERE state IN ('queued', 'running');

CREATE TRIGGER validation_check_request_insert_guard
BEFORE INSERT ON validation_check_request
BEGIN
    SELECT CASE
        WHEN NEW.state != 'queued' OR NEW.version != 1 OR NEW.candidate_run_id IS NOT NULL
            THEN RAISE(ABORT, 'ValidationCheckRequest must be created queued')
        WHEN NOT EXISTS (
            SELECT 1 FROM validation_check_definition d JOIN task t ON t.id = NEW.task_id
            WHERE d.id = NEW.check_definition_id AND d.definition_digest = NEW.definition_digest
              AND d.state = 'active' AND d.project_id = t.project_id
              AND (d.repo_id IS NULL OR d.repo_id IS t.repo_id))
            THEN RAISE(ABORT, 'FORGE_VALIDATION_CHECK_STALE_PIN')
        WHEN NOT EXISTS (
            SELECT 1 FROM task_coordination c
            JOIN validation_check_definition d ON d.id = NEW.check_definition_id
            WHERE c.task_id = NEW.task_id AND c.authorization_version = NEW.authorization_version
              AND c.revoked_at IS NULL AND c.mode != 'manual' AND c.team_stale = 0
              AND EXISTS (
                  SELECT 1 FROM json_each(c.envelope_json, '$.validation_checks') pin
                  WHERE json_extract(pin.value, '$.check_key') = d.check_key
                    AND json_extract(pin.value, '$.definition_digest') = NEW.definition_digest
                    AND json_extract(pin.value, '$.trigger') = CASE NEW.trigger_kind
                        WHEN 'coordinator_request' THEN 'on_request' ELSE 'after_integration' END))
            THEN RAISE(ABORT, 'FORGE_VALIDATION_CHECK_NOT_AUTHORIZED')
        WHEN EXISTS (
            SELECT 1 FROM workspace_lease l
            WHERE l.workspace_id = NEW.workspace_id AND l.status = 'active' AND l.role = 'worker')
            THEN RAISE(ABORT, 'FORGE_VALIDATION_WORKSPACE_LEASED')
    END;
END;
CREATE TRIGGER validation_check_request_update_guard
BEFORE UPDATE ON validation_check_request
WHEN NOT (
    OLD.state IN ('queued', 'running')
    AND NEW.version = OLD.version + 1
    AND NEW.id IS OLD.id AND NEW.task_id IS OLD.task_id
    AND NEW.authorization_version IS OLD.authorization_version
    AND NEW.check_definition_id IS OLD.check_definition_id
    AND NEW.definition_digest IS OLD.definition_digest
    AND NEW.trigger_kind IS OLD.trigger_kind AND NEW.subject_kind IS OLD.subject_kind
    AND NEW.subject_id IS OLD.subject_id AND NEW.subject_version IS OLD.subject_version
    AND NEW.workspace_id IS OLD.workspace_id AND NEW.commit_sha IS OLD.commit_sha
    AND NEW.workspace_snapshot_digest IS OLD.workspace_snapshot_digest
    AND NEW.baseline IS OLD.baseline
    AND NEW.requested_by_request_id IS OLD.requested_by_request_id
    AND NEW.requested_by_execution_id IS OLD.requested_by_execution_id
    AND NEW.reason IS OLD.reason AND NEW.idempotency_key IS OLD.idempotency_key
    AND NEW.created_at IS OLD.created_at
    AND (OLD.baseline_commit_sha IS NULL OR NEW.baseline_commit_sha IS OLD.baseline_commit_sha)
    AND (OLD.candidate_run_id IS NULL OR NEW.candidate_run_id IS OLD.candidate_run_id)
    AND ((OLD.state = 'queued' AND NEW.state IN ('queued', 'running', 'cancelled', 'failed_to_start'))
      OR (OLD.state = 'running' AND NEW.state IN ('running', 'completed', 'cancelled', 'failed_to_start')))
)
BEGIN
    SELECT RAISE(ABORT, 'ValidationCheckRequest identity is immutable and transitions are forward-only');
END;
CREATE TRIGGER validation_check_request_binding_guard
BEFORE UPDATE ON validation_check_request
WHEN (NEW.candidate_run_id IS NOT NULL AND NEW.candidate_run_id IS NOT OLD.candidate_run_id
      AND NOT EXISTS (SELECT 1 FROM validation_run r
          WHERE r.id = NEW.candidate_run_id AND r.task_id = NEW.task_id
            AND r.check_request_id = NEW.id))
  OR (NEW.state = 'completed' AND NOT EXISTS (
          SELECT 1 FROM validation_run r
          WHERE r.id = NEW.candidate_run_id AND r.task_id = NEW.task_id AND r.status != 'running'))
  OR (NEW.state = 'completed' AND NEW.baseline = 1 AND NOT EXISTS (
          SELECT 1 FROM validation_comparison c WHERE c.check_request_id = NEW.id))
BEGIN
    SELECT RAISE(ABORT, 'ValidationCheckRequest result must bind its exact terminal run and comparison');
END;
CREATE TRIGGER validation_check_request_immutable_delete
BEFORE DELETE ON validation_check_request
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'ValidationCheckRequest history is preserved outside Project teardown');
END;

-- validation_run: additive provenance. Historical rows stay 'legacy_command'.
-- 'baseline' is reserved by the frozen enum but never inserted: baseline results
-- live in validation_comparison (decision C2).
ALTER TABLE validation_run ADD COLUMN catalog_check_id TEXT;
ALTER TABLE validation_run ADD COLUMN definition_digest TEXT
    CHECK (definition_digest IS NULL OR length(definition_digest) = 64);
ALTER TABLE validation_run ADD COLUMN run_origin TEXT NOT NULL DEFAULT 'legacy_command'
    CHECK (run_origin IN ('legacy_command', 'coordinator_request', 'after_integration', 'baseline'));
ALTER TABLE validation_run ADD COLUMN check_request_id TEXT;

-- One catalog run per request; an idempotent replay of an already stored run key
-- bypasses the guard so ON CONFLICT(idempotency_key) DO NOTHING keeps working.
CREATE INDEX idx_validation_run_check_request
    ON validation_run(check_request_id) WHERE check_request_id IS NOT NULL;
CREATE TRIGGER validation_run_catalog_origin_guard
BEFORE INSERT ON validation_run
WHEN NOT EXISTS (SELECT 1 FROM validation_run existing WHERE existing.idempotency_key = NEW.idempotency_key)
BEGIN
    SELECT CASE
        WHEN NEW.run_origin = 'baseline'
            THEN RAISE(ABORT, 'Baseline results are recorded in ValidationComparison, not ValidationRun')
        WHEN (NEW.run_origin = 'legacy_command') != (NEW.catalog_check_id IS NULL
            AND NEW.check_request_id IS NULL AND NEW.definition_digest IS NULL)
            THEN RAISE(ABORT, 'ValidationRun catalog provenance must be all-or-nothing')
        WHEN NEW.run_origin != 'legacy_command' AND NOT EXISTS (
            SELECT 1 FROM validation_check_request q
            WHERE q.id = NEW.check_request_id AND q.task_id = NEW.task_id
              AND q.state = 'running'
              AND q.check_definition_id = NEW.catalog_check_id
              AND q.definition_digest = NEW.definition_digest
              AND q.workspace_id = NEW.workspace_id AND q.commit_sha = NEW.commit_sha
              AND q.workspace_snapshot_digest = NEW.workspace_snapshot_digest
              AND q.trigger_kind = NEW.run_origin
              AND q.candidate_run_id IS NULL)
            THEN RAISE(ABORT, 'ValidationRun must match its exact running ValidationCheckRequest')
        WHEN NEW.run_origin != 'legacy_command' AND EXISTS (
            SELECT 1 FROM validation_run r WHERE r.check_request_id = NEW.check_request_id)
            THEN RAISE(ABORT, 'ValidationCheckRequest already has its catalog ValidationRun')
    END;
END;
CREATE TRIGGER validation_run_catalog_provenance_immutable
BEFORE UPDATE OF catalog_check_id, definition_digest, run_origin, check_request_id ON validation_run
BEGIN
    SELECT RAISE(ABORT, 'ValidationRun catalog provenance is immutable');
END;

CREATE TABLE validation_comparison (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    check_request_id        TEXT NOT NULL UNIQUE,
    candidate_run_id        TEXT NOT NULL,
    candidate_status        TEXT NOT NULL CHECK (candidate_status IN
                                ('passed', 'failed', 'error', 'cancelled', 'stale')),
    candidate_output_digest TEXT CHECK (candidate_output_digest IS NULL
                                OR length(candidate_output_digest) = 64),
    -- Runner-owned detached worktree: no workspace / workspace_scope row.
    baseline_commit_sha     TEXT NOT NULL CHECK (length(trim(baseline_commit_sha)) BETWEEN 7 AND 128),
    baseline_snapshot_digest TEXT NOT NULL CHECK (length(baseline_snapshot_digest) = 64),
    baseline_provenance_json TEXT NOT NULL CHECK (json_valid(baseline_provenance_json)
                                AND json_type(baseline_provenance_json) = 'object'
                                AND length(baseline_provenance_json) <= 4096),
    baseline_status         TEXT NOT NULL CHECK (baseline_status IN
                                ('passed', 'failed', 'error', 'cancelled', 'stale')),
    baseline_exit_code      INTEGER,
    baseline_output_digest  TEXT CHECK (baseline_output_digest IS NULL
                                OR length(baseline_output_digest) = 64),
    classification          TEXT NOT NULL CHECK (classification IN
                                ('regression', 'pre_existing_failure', 'fixed', 'both_pass', 'inconclusive')),
    same_output_digest      INTEGER NOT NULL CHECK (same_output_digest IN (0, 1)),
    created_at              TEXT NOT NULL,
    UNIQUE(id, task_id),
    UNIQUE(candidate_run_id),
    FOREIGN KEY (check_request_id, task_id)
        REFERENCES validation_check_request(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (candidate_run_id, task_id)
        REFERENCES validation_run(id, task_id) ON DELETE RESTRICT,
    CHECK ((baseline_status = 'passed' AND baseline_exit_code = 0)
        OR (baseline_status = 'failed' AND baseline_exit_code IS NOT NULL AND baseline_exit_code != 0)
        OR baseline_status IN ('error', 'cancelled', 'stale')),
    CHECK (classification = CASE
        WHEN candidate_status = 'passed' AND baseline_status = 'passed' THEN 'both_pass'
        WHEN candidate_status = 'failed' AND baseline_status = 'passed' THEN 'regression'
        WHEN candidate_status = 'failed' AND baseline_status = 'failed' THEN 'pre_existing_failure'
        WHEN candidate_status = 'passed' AND baseline_status = 'failed' THEN 'fixed'
        ELSE 'inconclusive' END),
    CHECK (same_output_digest = CASE
        WHEN candidate_output_digest IS NOT NULL AND baseline_output_digest IS NOT NULL
         AND candidate_output_digest = baseline_output_digest THEN 1 ELSE 0 END)
);
CREATE INDEX idx_validation_comparison_task ON validation_comparison(task_id, created_at DESC, id DESC);

CREATE TRIGGER validation_comparison_insert_guard
BEFORE INSERT ON validation_comparison
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1 FROM validation_check_request q
            JOIN validation_run r ON r.id = NEW.candidate_run_id AND r.task_id = NEW.task_id
            WHERE q.id = NEW.check_request_id AND q.task_id = NEW.task_id
              AND q.baseline = 1 AND q.state = 'running'
              AND q.baseline_commit_sha IS NEW.baseline_commit_sha
              AND r.check_request_id = q.id AND r.run_origin IN ('coordinator_request', 'after_integration')
              AND r.definition_digest = q.definition_digest
              AND r.status != 'running' AND r.status = NEW.candidate_status)
            THEN RAISE(ABORT, 'ValidationComparison must bind a baseline request and its exact terminal candidate run')
    END;
END;
CREATE TRIGGER validation_comparison_immutable
BEFORE UPDATE ON validation_comparison
BEGIN
    SELECT RAISE(ABORT, 'ValidationComparison is immutable evidence');
END;
CREATE TRIGGER validation_comparison_immutable_delete
BEFORE DELETE ON validation_comparison
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'ValidationComparison is preserved outside Project teardown');
END;
