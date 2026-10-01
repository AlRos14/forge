-- Plan PR8 adds independent, durable Review Execution outputs and deterministic
-- ValidationRuns. V006/V008/V011/V084 remain immutable historical storage.

CREATE TABLE validation_run (
    id                       TEXT PRIMARY KEY,
    task_id                  TEXT NOT NULL,
    work_unit_id             TEXT,
    caused_by_execution_id   TEXT,
    check_identity           TEXT NOT NULL CHECK (length(trim(check_identity)) BETWEEN 1 AND 256),
    command                  TEXT NOT NULL CHECK (length(trim(command)) BETWEEN 1 AND 8192),
    config_summary_json      TEXT NOT NULL CHECK (
                                 json_valid(config_summary_json)
                                 AND json_type(config_summary_json) = 'object'
                                 AND length(config_summary_json) <= 8192
                             ),
    config_digest            TEXT NOT NULL CHECK (length(config_digest) = 64),
    workspace_id             TEXT NOT NULL,
    commit_sha                TEXT NOT NULL CHECK (length(trim(commit_sha)) BETWEEN 7 AND 128),
    workspace_snapshot_digest TEXT NOT NULL CHECK (length(workspace_snapshot_digest) = 64),
    idempotency_key           TEXT NOT NULL UNIQUE CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    status                    TEXT NOT NULL CHECK (status IN (
                                  'running', 'passed', 'failed', 'error', 'cancelled', 'stale'
                              )),
    exit_code                 INTEGER,
    started_at                TEXT NOT NULL,
    finished_at               TEXT,
    logs_ref                  TEXT,
    claim_owner               TEXT,
    claim_until               TEXT,
    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL,
    UNIQUE(id, task_id),
    FOREIGN KEY (task_id) REFERENCES task(id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, task_id)
        REFERENCES workspace(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (caused_by_execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT,
    CHECK (
        (status = 'running' AND finished_at IS NULL AND exit_code IS NULL AND logs_ref IS NULL)
        OR (status = 'passed' AND finished_at IS NOT NULL AND exit_code = 0
            AND logs_ref IS NOT NULL AND length(trim(logs_ref)) > 0)
        OR (status = 'failed' AND finished_at IS NOT NULL AND exit_code IS NOT NULL
            AND exit_code != 0 AND logs_ref IS NOT NULL AND length(trim(logs_ref)) > 0)
        OR (status IN ('error', 'cancelled', 'stale') AND finished_at IS NOT NULL
            AND logs_ref IS NOT NULL AND length(trim(logs_ref)) > 0)
    ),
    CHECK ((claim_owner IS NULL AND claim_until IS NULL)
        OR (status = 'running' AND claim_owner IS NOT NULL AND claim_until IS NOT NULL))
);
CREATE INDEX idx_validation_run_task_subject
    ON validation_run(task_id, workspace_id, commit_sha, check_identity, created_at, id);
CREATE INDEX idx_validation_run_cause
    ON validation_run(caused_by_execution_id, task_id)
    WHERE caused_by_execution_id IS NOT NULL;

CREATE TRIGGER validation_run_insert_running_only
BEFORE INSERT ON validation_run
WHEN NEW.status != 'running'
BEGIN
    SELECT RAISE(ABORT, 'ValidationRun must be created in running state');
END;

CREATE TABLE evidence (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    kind            TEXT NOT NULL CHECK (length(trim(kind)) BETWEEN 1 AND 128),
    content_json    TEXT NOT NULL CHECK (
                        json_valid(content_json)
                        AND json_type(content_json) = 'object'
                        AND length(content_json) <= 32768
                    ),
    digest          TEXT NOT NULL CHECK (length(digest) = 64),
    created_at      TEXT NOT NULL,
    UNIQUE(id, task_id)
);
CREATE INDEX idx_evidence_task_created ON evidence(task_id, created_at DESC, id DESC);

CREATE TABLE evidence_validation_run_producer (
    evidence_id       TEXT NOT NULL,
    validation_run_id TEXT NOT NULL,
    task_id           TEXT NOT NULL,
    evidence_key      TEXT NOT NULL CHECK (length(trim(evidence_key)) BETWEEN 1 AND 128),
    PRIMARY KEY (evidence_id),
    UNIQUE(validation_run_id, evidence_key),
    FOREIGN KEY (evidence_id, task_id)
        REFERENCES evidence(id, task_id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (validation_run_id, task_id)
        REFERENCES validation_run(id, task_id) ON DELETE RESTRICT
);
CREATE INDEX idx_evidence_validation_run
    ON evidence_validation_run_producer(validation_run_id, evidence_key);

CREATE TABLE execution_evidence_input (
    execution_id TEXT NOT NULL,
    evidence_id  TEXT NOT NULL,
    task_id      TEXT NOT NULL,
    digest       TEXT NOT NULL CHECK (length(digest) = 64),
    created_at   TEXT NOT NULL,
    PRIMARY KEY (execution_id, evidence_id),
    FOREIGN KEY (execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (evidence_id, task_id)
        REFERENCES evidence(id, task_id) ON DELETE RESTRICT
);
CREATE INDEX idx_execution_evidence_input_evidence
    ON execution_evidence_input(evidence_id, execution_id);

CREATE TABLE artifact_validation_run_producer (
    artifact_id       TEXT NOT NULL,
    validation_run_id TEXT NOT NULL,
    task_id           TEXT NOT NULL,
    PRIMARY KEY (artifact_id),
    UNIQUE(artifact_id, validation_run_id, task_id),
    FOREIGN KEY (artifact_id, task_id)
        REFERENCES artifact(id, task_id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (validation_run_id, task_id)
        REFERENCES validation_run(id, task_id) ON DELETE RESTRICT
);
CREATE INDEX idx_artifact_validation_run_producer_run
    ON artifact_validation_run_producer(validation_run_id);

CREATE TABLE validation_run_artifact_output (
    validation_run_id TEXT NOT NULL,
    artifact_id       TEXT NOT NULL UNIQUE,
    task_id           TEXT NOT NULL,
    kind              TEXT NOT NULL CHECK (kind = 'validation_report'),
    digest            TEXT,
    created_at        TEXT NOT NULL,
    PRIMARY KEY (validation_run_id, kind),
    FOREIGN KEY (validation_run_id, task_id)
        REFERENCES validation_run(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (artifact_id, task_id)
        REFERENCES artifact(id, task_id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (artifact_id, validation_run_id, task_id)
        REFERENCES artifact_validation_run_producer(artifact_id, validation_run_id, task_id)
        ON DELETE RESTRICT
);
CREATE INDEX idx_validation_run_artifact_output_artifact
    ON validation_run_artifact_output(artifact_id);

-- A reviewer Execution pins the precise Evidence supplied to it before launch.
CREATE TRIGGER execution_evidence_input_guard_insert
BEFORE INSERT ON execution_evidence_input
WHEN NOT EXISTS (
    SELECT 1 FROM execution e
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
      AND e.status = 'running' AND e.logs_path IS NULL
) OR NOT EXISTS (
    SELECT 1 FROM evidence ev
    WHERE ev.id = NEW.evidence_id AND ev.task_id = NEW.task_id
      AND ev.digest = NEW.digest
)
BEGIN
    SELECT RAISE(ABORT, 'Execution Evidence input digest, Task, or lifecycle is invalid');
END;
CREATE TRIGGER execution_evidence_input_immutable_update
BEFORE UPDATE ON execution_evidence_input BEGIN
    SELECT RAISE(ABORT, 'Execution Evidence inputs are immutable');
END;
CREATE TRIGGER execution_evidence_input_immutable_delete
BEFORE DELETE ON execution_evidence_input
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Execution Evidence inputs are immutable outside Project teardown');
END;

CREATE TRIGGER validation_run_identity_immutable
BEFORE UPDATE OF task_id, work_unit_id, caused_by_execution_id, check_identity, command,
                 config_summary_json, config_digest, workspace_id, commit_sha,
                 workspace_snapshot_digest,
                 idempotency_key, started_at, created_at ON validation_run
BEGIN
    SELECT RAISE(ABORT, 'ValidationRun identity is immutable');
END;
CREATE TRIGGER validation_run_terminal_transition_guard
BEFORE UPDATE OF status ON validation_run
WHEN OLD.status != NEW.status AND OLD.status != 'running'
BEGIN
    SELECT RAISE(ABORT, 'terminal ValidationRun status is immutable');
END;
CREATE TRIGGER validation_run_terminal_immutable
BEFORE UPDATE ON validation_run
WHEN OLD.status != 'running' AND (
    OLD.status IS NOT NEW.status OR OLD.exit_code IS NOT NEW.exit_code
    OR OLD.finished_at IS NOT NEW.finished_at OR OLD.logs_ref IS NOT NEW.logs_ref
)
BEGIN
    SELECT RAISE(ABORT, 'terminal ValidationRun result is immutable');
END;

CREATE TRIGGER evidence_validation_run_producer_guard_insert
BEFORE INSERT ON evidence_validation_run_producer
WHEN EXISTS (SELECT 1 FROM evidence e WHERE e.id = NEW.evidence_id)
  OR NOT EXISTS (
      SELECT 1 FROM validation_run v
      WHERE v.id = NEW.validation_run_id AND v.task_id = NEW.task_id
        AND v.status = 'running'
  )
BEGIN
    SELECT RAISE(ABORT, 'Evidence requires one running same-Task ValidationRun producer');
END;
CREATE TRIGGER evidence_producer_required_insert
BEFORE INSERT ON evidence
WHEN NOT EXISTS (
    SELECT 1 FROM evidence_validation_run_producer p
    JOIN validation_run v ON v.id = p.validation_run_id AND v.task_id = p.task_id
    WHERE p.evidence_id = NEW.id AND p.task_id = NEW.task_id AND v.status = 'running'
      AND json_extract(NEW.content_json, '$.validation_run_id') = v.id
      AND json_extract(NEW.content_json, '$.task_id') = v.task_id
      AND json_extract(NEW.content_json, '$.check_identity') = v.check_identity
      AND json_extract(NEW.content_json, '$.command') = v.command
      AND json_extract(NEW.content_json, '$.config_digest') = v.config_digest
      AND json_extract(NEW.content_json, '$.workspace_id') = v.workspace_id
      AND json_extract(NEW.content_json, '$.commit_sha') = v.commit_sha
      AND json_extract(NEW.content_json, '$.workspace_snapshot_digest') = v.workspace_snapshot_digest
)
BEGIN
    SELECT RAISE(ABORT, 'Evidence requires exact same-Task ValidationRun identity content');
END;

CREATE TRIGGER validation_run_completion_requires_evidence
BEFORE UPDATE OF status ON validation_run
WHEN OLD.status = 'running' AND NEW.status != 'running'
 AND NOT EXISTS (
    SELECT 1 FROM evidence_validation_run_producer p
    JOIN evidence ev ON ev.id = p.evidence_id AND ev.task_id = p.task_id
    WHERE p.validation_run_id = NEW.id AND p.task_id = NEW.task_id
      AND json_extract(ev.content_json, '$.validation_run_id') = NEW.id
      AND json_extract(ev.content_json, '$.task_id') = NEW.task_id
      AND json_extract(ev.content_json, '$.check_identity') = NEW.check_identity
      AND json_extract(ev.content_json, '$.command') = NEW.command
      AND json_extract(ev.content_json, '$.config_digest') = NEW.config_digest
      AND json_extract(ev.content_json, '$.workspace_id') = NEW.workspace_id
      AND json_extract(ev.content_json, '$.commit_sha') = NEW.commit_sha
      AND json_extract(ev.content_json, '$.workspace_snapshot_digest') = NEW.workspace_snapshot_digest
      AND json_extract(ev.content_json, '$.status') = NEW.status
      AND json_extract(ev.content_json, '$.exit_code') IS NEW.exit_code
      AND json_extract(ev.content_json, '$.started_at') = NEW.started_at
      AND json_extract(ev.content_json, '$.finished_at') = NEW.finished_at
      AND NEW.logs_ref = 'validation-evidence://' || p.evidence_id
 )
BEGIN
    SELECT RAISE(ABORT, 'Terminal ValidationRun requires Evidence with exact identity and result');
END;
CREATE TRIGGER evidence_immutable_update
BEFORE UPDATE ON evidence BEGIN
    SELECT RAISE(ABORT, 'Evidence is immutable');
END;
CREATE TRIGGER evidence_immutable_delete
BEFORE DELETE ON evidence
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Evidence is immutable outside Project teardown');
END;
CREATE TRIGGER evidence_validation_run_producer_immutable_update
BEFORE UPDATE ON evidence_validation_run_producer BEGIN
    SELECT RAISE(ABORT, 'Evidence producers are immutable');
END;
CREATE TRIGGER evidence_validation_run_producer_immutable_delete
BEFORE DELETE ON evidence_validation_run_producer
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Evidence producers are immutable outside Project teardown');
END;

DROP TRIGGER artifact_producer_required_insert;
CREATE TRIGGER artifact_producer_required_insert
BEFORE INSERT ON artifact
WHEN (
    (EXISTS (SELECT 1 FROM artifact_execution_producer p WHERE p.artifact_id = NEW.id)
     AND EXISTS (SELECT 1 FROM artifact_validation_run_producer p WHERE p.artifact_id = NEW.id))
    OR (NOT EXISTS (SELECT 1 FROM artifact_execution_producer p WHERE p.artifact_id = NEW.id)
        AND NOT EXISTS (SELECT 1 FROM artifact_validation_run_producer p WHERE p.artifact_id = NEW.id))
    OR EXISTS (
        SELECT 1 FROM artifact_execution_producer p
        JOIN execution e ON e.id = p.execution_id AND e.task_id = p.task_id
        WHERE p.artifact_id = NEW.id AND p.task_id = NEW.task_id
          AND (e.actor_kind NOT IN ('human', 'agent') OR e.actor_id IS NULL
               OR (e.actor_kind = 'human' AND NOT EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
               OR (e.actor_kind = 'agent' AND NOT EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id)))
    )
    OR EXISTS (
        SELECT 1 FROM artifact_validation_run_producer p
        JOIN validation_run v ON v.id = p.validation_run_id AND v.task_id = p.task_id
        WHERE p.artifact_id = NEW.id AND p.task_id = NEW.task_id
          AND (NEW.kind != 'validation_report' OR v.status != 'running'
               OR v.task_id != NEW.task_id)
    )
)
BEGIN
    SELECT RAISE(ABORT, 'Artifact requires exactly one valid same-Task producer');
END;

CREATE TRIGGER artifact_validation_run_producer_guard_insert
BEFORE INSERT ON artifact_validation_run_producer
WHEN EXISTS (SELECT 1 FROM artifact a WHERE a.id = NEW.artifact_id)
  OR EXISTS (SELECT 1 FROM artifact_execution_producer p WHERE p.artifact_id = NEW.artifact_id)
  OR NOT EXISTS (
      SELECT 1 FROM validation_run v
      WHERE v.id = NEW.validation_run_id AND v.task_id = NEW.task_id
        AND v.status = 'running'
  )
BEGIN
    SELECT RAISE(ABORT, 'ValidationRun Artifact producer is invalid or already fixed');
END;
CREATE TRIGGER artifact_execution_producer_exactly_one_guard_insert
BEFORE INSERT ON artifact_execution_producer
WHEN EXISTS (
    SELECT 1 FROM artifact_validation_run_producer p WHERE p.artifact_id = NEW.artifact_id
)
BEGIN
    SELECT RAISE(ABORT, 'Artifact may have exactly one producer');
END;
CREATE TRIGGER artifact_validation_run_producer_immutable_update
BEFORE UPDATE ON artifact_validation_run_producer BEGIN
    SELECT RAISE(ABORT, 'Artifact producers are immutable');
END;
CREATE TRIGGER artifact_validation_run_producer_immutable_delete
BEFORE DELETE ON artifact_validation_run_producer
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Artifact producers are immutable outside Project teardown');
END;

CREATE TRIGGER execution_artifact_output_review_guard_insert
BEFORE INSERT ON execution_artifact_output
WHEN NEW.kind = 'review_report' AND NOT EXISTS (
    SELECT 1 FROM execution e
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
      AND e.role = 'reviewer' AND e.purpose = 'review'
      AND e.actor_kind IN ('human', 'agent') AND e.actor_id IS NOT NULL
      AND ((e.actor_kind = 'human' AND EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
        OR (e.actor_kind = 'agent' AND EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id)))
)
BEGIN
    SELECT RAISE(ABORT, 'ReviewReport output requires a real reviewer Review Execution');
END;
CREATE TRIGGER validation_run_artifact_output_guard_insert
BEFORE INSERT ON validation_run_artifact_output
WHEN NOT EXISTS (
    SELECT 1 FROM validation_run v
    WHERE v.id = NEW.validation_run_id AND v.task_id = NEW.task_id AND v.status = 'running'
) OR EXISTS (
    SELECT 1 FROM artifact a
    WHERE a.id = NEW.artifact_id
      AND (a.task_id != NEW.task_id OR a.kind != NEW.kind OR a.digest IS NOT NEW.digest)
)
BEGIN
    SELECT RAISE(ABORT, 'ValidationReport output producer, kind, digest, or Task is invalid');
END;
CREATE TRIGGER validation_run_artifact_output_match_artifact_insert
BEFORE INSERT ON artifact
WHEN EXISTS (
    SELECT 1 FROM validation_run_artifact_output o
    WHERE o.artifact_id = NEW.id
      AND (o.task_id != NEW.task_id OR o.kind != NEW.kind OR o.digest IS NOT NEW.digest)
)
BEGIN
    SELECT RAISE(ABORT, 'Artifact does not match its ValidationRun output binding');
END;
CREATE TRIGGER validation_report_artifact_required_insert
BEFORE INSERT ON artifact
WHEN NEW.kind = 'validation_report' AND NOT EXISTS (
    SELECT 1 FROM validation_run_artifact_output o
    JOIN artifact_validation_run_producer p
      ON p.artifact_id = o.artifact_id AND p.validation_run_id = o.validation_run_id
     AND p.task_id = o.task_id
    JOIN validation_run v ON v.id = p.validation_run_id AND v.task_id = p.task_id
    WHERE o.artifact_id = NEW.id AND o.task_id = NEW.task_id
      AND o.kind = NEW.kind AND o.digest IS NEW.digest AND v.status = 'running'
)
BEGIN
    SELECT RAISE(ABORT, 'ValidationReport requires its exact ValidationRun output binding');
END;
CREATE TRIGGER review_report_artifact_required_insert
BEFORE INSERT ON artifact
WHEN NEW.kind = 'review_report' AND NOT EXISTS (
    SELECT 1 FROM execution_artifact_output o
    JOIN artifact_execution_producer p
      ON p.artifact_id = o.artifact_id AND p.execution_id = o.execution_id
     AND p.task_id = o.task_id
    JOIN execution e ON e.id = p.execution_id AND e.task_id = p.task_id
    WHERE o.artifact_id = NEW.id AND o.task_id = NEW.task_id
      AND o.kind = NEW.kind AND o.digest IS NEW.digest
      AND NEW.storage_kind = 'inline'
      AND json_valid(NEW.content) AND json_type(NEW.content) = 'object'
      AND json_extract(NEW.content, '$.kind') = 'review_report'
      AND json_extract(NEW.content, '$.verdict') IN ('pass', 'request_changes', 'questions')
      AND length(trim(COALESCE(json_extract(NEW.content, '$.summary'), ''))) > 0
      AND json_type(NEW.content, '$.criteria') = 'array'
      AND json_type(NEW.content, '$.findings') = 'array'
      AND json_type(NEW.content, '$.questions') = 'array'
      AND json_type(NEW.content, '$.evidence_considered') = 'array'
      AND json_extract(NEW.content, '$.subject.task_id') = e.task_id
      AND json_extract(NEW.content, '$.subject.review_execution_id') = e.id
      AND json_extract(NEW.content, '$.subject.workspace_id') IS e.workspace_id
      AND json_extract(NEW.content, '$.subject.base_commit_sha') IS e.before_sha
      AND json_extract(NEW.content, '$.subject.head_commit_sha') IS e.after_sha
      AND e.role = 'reviewer' AND e.purpose = 'review'
      AND e.actor_kind IN ('human', 'agent') AND e.actor_id IS NOT NULL
      AND ((e.actor_kind = 'human' AND EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
        OR (e.actor_kind = 'agent' AND EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id)))
      AND NOT EXISTS (
          SELECT 1 FROM json_each(NEW.content, '$.evidence_considered') ref
          WHERE (
              (json_type(ref.value, '$.artifact_id') IS NOT NULL
               AND json_type(ref.value, '$.evidence_id') IS NOT NULL)
              OR (json_type(ref.value, '$.artifact_id') IS NULL
                  AND json_type(ref.value, '$.evidence_id') IS NULL)
              OR (json_type(ref.value, '$.artifact_id') IS NOT NULL AND NOT EXISTS (
                  SELECT 1 FROM execution_artifact_input i
                  JOIN artifact input ON input.id = i.artifact_id AND input.task_id = i.task_id
                  WHERE i.execution_id = e.id AND i.task_id = e.task_id
                    AND i.artifact_id = json_extract(ref.value, '$.artifact_id')
                    AND i.digest IS json_extract(ref.value, '$.digest')
                    AND input.kind = json_extract(ref.value, '$.kind')
              ))
              OR (json_type(ref.value, '$.evidence_id') IS NOT NULL AND NOT EXISTS (
                  SELECT 1 FROM execution_evidence_input i
                  JOIN evidence input ON input.id = i.evidence_id AND input.task_id = i.task_id
                  JOIN evidence_validation_run_producer p
                    ON p.evidence_id = input.id AND p.task_id = input.task_id
                  WHERE i.execution_id = e.id AND i.task_id = e.task_id
                    AND i.evidence_id = json_extract(ref.value, '$.evidence_id')
                    AND i.digest = json_extract(ref.value, '$.digest')
                    AND input.kind = json_extract(ref.value, '$.kind')
                    AND p.validation_run_id = json_extract(ref.value, '$.validation_run_id')
              ))
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'ReviewReport requires its exact attributed reviewer output binding');
END;
CREATE TRIGGER validation_run_terminal_report_matches_subject
BEFORE UPDATE OF status ON validation_run
WHEN OLD.status = 'running' AND NEW.status != 'running'
 AND EXISTS (
    SELECT 1 FROM validation_run_artifact_output o
    JOIN artifact a ON a.id = o.artifact_id AND a.task_id = o.task_id
    WHERE o.validation_run_id = NEW.id AND o.task_id = NEW.task_id
      AND o.kind = 'validation_report'
      AND (
          json_extract(a.content, '$.kind') != 'validation_report'
          OR json_extract(a.content, '$.validation_run_id') != NEW.id
          OR json_extract(a.content, '$.task_id') != NEW.task_id
          OR json_extract(a.content, '$.check_identity') != NEW.check_identity
          OR json_extract(a.content, '$.workspace_id') != NEW.workspace_id
          OR json_extract(a.content, '$.commit_sha') != NEW.commit_sha
          OR json_extract(a.content, '$.workspace_snapshot_digest') != NEW.workspace_snapshot_digest
          OR json_extract(a.content, '$.status') != NEW.status
          OR json_extract(a.content, '$.exit_code') IS NOT NEW.exit_code
          OR json_type(a.content, '$.evidence_ids') != 'array'
          OR (SELECT COUNT(*) FROM json_each(a.content, '$.evidence_ids')) != (
              SELECT COUNT(*) FROM evidence_validation_run_producer p
              WHERE p.validation_run_id = NEW.id AND p.task_id = NEW.task_id
          )
          OR EXISTS (
              SELECT 1 FROM json_each(a.content, '$.evidence_ids') item
              WHERE NOT EXISTS (
                  SELECT 1 FROM evidence_validation_run_producer p
                  WHERE p.validation_run_id = NEW.id AND p.task_id = NEW.task_id
                    AND p.evidence_id = item.value
              )
          )
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'ValidationReport must match its exact terminal ValidationRun and Evidence');
END;
CREATE TRIGGER validation_run_artifact_output_immutable_update
BEFORE UPDATE ON validation_run_artifact_output BEGIN
    SELECT RAISE(ABORT, 'ValidationRun output bindings are immutable');
END;
CREATE TRIGGER validation_run_artifact_output_immutable_delete
BEFORE DELETE ON validation_run_artifact_output
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'ValidationRun output bindings are immutable outside Project teardown');
END;

CREATE TRIGGER reviewer_execution_purpose_guard_insert
BEFORE INSERT ON execution
WHEN (NEW.role = 'reviewer' AND NEW.purpose IS NOT 'review')
  OR (NEW.purpose = 'review' AND NEW.role IS NOT 'reviewer')
BEGIN
    SELECT RAISE(ABORT, 'Review Execution must use role reviewer and purpose review');
END;
CREATE TRIGGER reviewer_execution_purpose_guard_update
BEFORE UPDATE OF role, purpose ON execution
WHEN (NEW.role = 'reviewer' AND NEW.purpose IS NOT 'review')
  OR (NEW.purpose = 'review' AND NEW.role IS NOT 'reviewer')
BEGIN
    SELECT RAISE(ABORT, 'Review Execution must use role reviewer and purpose review');
END;
CREATE TRIGGER review_report_subject_commit_immutable
BEFORE UPDATE OF workspace_id, before_sha, after_sha ON execution
WHEN EXISTS (
    SELECT 1 FROM execution_artifact_output o
    WHERE o.execution_id = OLD.id AND o.task_id = OLD.task_id AND o.kind = 'review_report'
) AND (NEW.workspace_id IS NOT OLD.workspace_id
    OR NEW.before_sha IS NOT OLD.before_sha
    OR NEW.after_sha IS NOT OLD.after_sha)
BEGIN
    SELECT RAISE(ABORT, 'ReviewReport subject Workspace and commits are immutable');
END;
CREATE TRIGGER completed_review_execution_requires_report
BEFORE UPDATE OF status ON execution
WHEN OLD.status = 'running' AND NEW.status = 'completed'
 AND NEW.role = 'reviewer' AND NEW.purpose = 'review'
 AND NOT EXISTS (
    SELECT 1 FROM execution_artifact_output o
    JOIN artifact_execution_producer p
      ON p.artifact_id = o.artifact_id AND p.execution_id = o.execution_id
     AND p.task_id = o.task_id
    JOIN artifact a ON a.id = p.artifact_id AND a.task_id = p.task_id
    WHERE o.execution_id = NEW.id AND o.task_id = NEW.task_id
      AND o.kind = 'review_report' AND a.kind = 'review_report'
 )
BEGIN
    SELECT RAISE(ABORT, 'Completed Review Execution requires its exact ReviewReport Artifact');
END;

CREATE TABLE legacy_review_artifact_migration (
    review_id         TEXT PRIMARY KEY REFERENCES review(id) ON DELETE RESTRICT,
    artifact_id       TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    migration_status  TEXT NOT NULL CHECK (migration_status IN (
        'migrated', 'mapped_duplicate', 'source_execution_missing',
        'source_execution_unresolved', 'wrong_task', 'actor_missing',
        'content_unrecoverable', 'ambiguous', 'existing_output_conflict'
    )),
    details_json      TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(details_json))
);
CREATE INDEX idx_legacy_review_artifact_migration_artifact
    ON legacy_review_artifact_migration(artifact_id);

CREATE TABLE legacy_ci_validation_migration (
    review_id         TEXT NOT NULL REFERENCES review(id) ON DELETE RESTRICT,
    step_index        INTEGER NOT NULL,
    validation_run_id TEXT REFERENCES validation_run(id) ON DELETE RESTRICT,
    migration_status  TEXT NOT NULL CHECK (migration_status IN (
        'migrated', 'mapped_duplicate', 'no_steps', 'source_execution_missing',
        'source_execution_unresolved', 'wrong_task', 'check_identity_missing',
        'status_missing', 'timestamps_missing', 'workspace_missing',
        'commit_identity_missing', 'output_missing', 'ambiguous',
        'insufficient_provenance'
    )),
    details_json      TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(details_json)),
    PRIMARY KEY (review_id, step_index)
);
CREATE INDEX idx_legacy_ci_validation_migration_run
    ON legacy_ci_validation_migration(validation_run_id);

-- Reconstruct only complete structured reviewer results with an exact same-Task,
-- real ActorRef, role=reviewer, purpose=review producer. Other history stays put.
CREATE TEMP TABLE pr8_legacy_review_candidate AS
SELECT r.id AS review_id, r.task_id, r.execution_id, r.created_at,
       json_extract(r.step_results_json, '$.structured_result') AS result_json,
       e.workspace_id, e.before_sha, e.after_sha,
       CASE
         WHEN e.id IS NULL THEN 'source_execution_missing'
         WHEN e.task_id != r.task_id THEN 'wrong_task'
         WHEN e.actor_kind NOT IN ('human', 'agent') OR e.actor_id IS NULL
           OR (e.actor_kind = 'human' AND NOT EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
           OR (e.actor_kind = 'agent' AND NOT EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id))
           THEN 'actor_missing'
         WHEN e.role != 'reviewer' OR e.purpose != 'review' OR e.status != 'completed'
           THEN 'source_execution_unresolved'
         WHEN r.status NOT IN ('passed', 'failed') THEN 'content_unrecoverable'
         WHEN json_type(r.step_results_json, '$.structured_result') != 'object'
           OR json_extract(r.step_results_json, '$.structured_result.schema_version') != 1
           OR json_extract(r.step_results_json, '$.structured_result.kind') != 'review'
           OR json_extract(r.step_results_json, '$.structured_result.verdict') NOT IN ('pass', 'fail', 'needs_human')
           OR json_type(r.step_results_json, '$.structured_result.summary') != 'text'
           OR length(trim(COALESCE(json_extract(r.step_results_json, '$.structured_result.summary'), ''))) = 0
           OR (json_type(r.step_results_json, '$.structured_result.criteria') IS NOT NULL
               AND json_type(r.step_results_json, '$.structured_result.criteria') != 'array')
           OR json_type(r.step_results_json, '$.structured_result.findings') != 'array'
           OR json_type(r.step_results_json, '$.structured_result.questions') != 'array'
           OR EXISTS (
               SELECT 1 FROM json_each(r.step_results_json, '$.structured_result.criteria') item
               WHERE item.type != 'text'
           )
           OR EXISTS (
               SELECT 1 FROM json_each(r.step_results_json, '$.structured_result.findings') item
               WHERE item.type != 'text'
           )
           OR EXISTS (
               SELECT 1 FROM json_each(r.step_results_json, '$.structured_result.questions') item
               WHERE item.type != 'text'
           )
           OR (json_type(r.step_results_json, '$.structured_result.evidence_considered') IS NOT NULL
               AND (json_type(r.step_results_json, '$.structured_result.evidence_considered') != 'array'
                    OR json_array_length(r.step_results_json, '$.structured_result.evidence_considered') != 0))
           THEN 'content_unrecoverable'
         ELSE 'eligible'
       END AS candidate_status
FROM review r
LEFT JOIN execution e ON e.id = r.execution_id;

CREATE TEMP TABLE pr8_legacy_review_primary AS
WITH eligible_base AS (
    SELECT c.*,
           ROW_NUMBER() OVER (PARTITION BY execution_id ORDER BY created_at ASC, review_id ASC) AS rn
    FROM pr8_legacy_review_candidate c
    WHERE candidate_status = 'eligible'
), eligible AS (
    SELECT b.*, g.distinct_results
    FROM eligible_base b
    JOIN (
        SELECT execution_id, COUNT(DISTINCT result_json) AS distinct_results
        FROM pr8_legacy_review_candidate
        WHERE candidate_status = 'eligible'
        GROUP BY execution_id
    ) g ON g.execution_id = b.execution_id
)
SELECT review_id, task_id, execution_id, result_json, workspace_id, before_sha, after_sha,
       lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
       lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
       substr('89ab', 1 + (abs(random()) % 4), 1) ||
       lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))) AS artifact_id,
       CASE
         WHEN distinct_results > 1 THEN 'ambiguous'
         WHEN EXISTS (
             SELECT 1 FROM execution_artifact_output o
             JOIN artifact a ON a.id = o.artifact_id AND a.task_id = o.task_id
             WHERE o.execution_id = eligible.execution_id AND o.task_id = eligible.task_id
               AND o.kind = 'review_report'
               AND a.kind = 'review_report'
               AND a.content = json_set(result_json, '$.kind', 'review_report',
                   '$.verdict', CASE json_extract(result_json, '$.verdict')
                       WHEN 'fail' THEN 'request_changes'
                       WHEN 'needs_human' THEN 'questions'
                       ELSE 'pass' END,
                   '$.criteria', json(COALESCE(json_extract(result_json, '$.criteria'), '[]')),
                   '$.evidence_considered', json('[]'),
                   '$.subject', json(json_object(
                       'task_id', eligible.task_id,
                       'review_execution_id', eligible.execution_id,
                       'workspace_id', eligible.workspace_id,
                       'base_commit_sha', eligible.before_sha,
                       'head_commit_sha', eligible.after_sha)))
         ) THEN 'mapped_duplicate'
         WHEN EXISTS (
             SELECT 1 FROM execution_artifact_output o
             WHERE o.execution_id = eligible.execution_id AND o.kind = 'review_report'
         ) THEN 'existing_output_conflict'
         WHEN rn = 1 THEN 'migrated'
         ELSE 'mapped_duplicate'
       END AS migration_status
FROM eligible;

INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
SELECT p.artifact_id, p.execution_id, p.task_id
FROM pr8_legacy_review_primary p
WHERE p.migration_status = 'migrated';

INSERT INTO execution_artifact_output (execution_id, artifact_id, task_id, kind, digest, created_at)
SELECT p.execution_id, p.artifact_id, p.task_id, 'review_report', NULL, r.created_at
FROM pr8_legacy_review_primary p JOIN review r ON r.id = p.review_id
WHERE p.migration_status = 'migrated';

INSERT INTO artifact (id, task_id, kind, storage_kind, content, content_ref, metadata_json, digest, created_at)
SELECT p.artifact_id, p.task_id, 'review_report', 'inline',
       json_set(p.result_json, '$.kind', 'review_report',
           '$.verdict', CASE json_extract(p.result_json, '$.verdict')
               WHEN 'fail' THEN 'request_changes'
               WHEN 'needs_human' THEN 'questions'
               ELSE 'pass' END,
           '$.criteria', json(COALESCE(json_extract(p.result_json, '$.criteria'), '[]')),
           '$.evidence_considered', json('[]'),
           '$.subject', json(json_object(
               'task_id', p.task_id,
               'review_execution_id', p.execution_id,
               'workspace_id', p.workspace_id,
               'base_commit_sha', p.before_sha,
               'head_commit_sha', p.after_sha))),
       NULL, json_object('migration_source', 'legacy_review', 'review_id', p.review_id), NULL, r.created_at
FROM pr8_legacy_review_primary p JOIN review r ON r.id = p.review_id
WHERE p.migration_status = 'migrated';

INSERT INTO legacy_review_artifact_migration (review_id, artifact_id, migration_status, details_json)
SELECT c.review_id,
       CASE
           WHEN p.migration_status = 'migrated' THEN p.artifact_id
           WHEN p.migration_status = 'mapped_duplicate' THEN (
               SELECT o.artifact_id FROM execution_artifact_output o
               WHERE o.execution_id = c.execution_id AND o.kind = 'review_report'
               UNION ALL
               SELECT p2.artifact_id FROM pr8_legacy_review_primary p2
               WHERE p2.execution_id = c.execution_id AND p2.migration_status = 'migrated'
               LIMIT 1
           )
           ELSE NULL
       END,
       COALESCE(p.migration_status, c.candidate_status),
       json_object('source_execution_id', c.execution_id)
FROM pr8_legacy_review_candidate c
LEFT JOIN pr8_legacy_review_primary p ON p.review_id = c.review_id;

INSERT INTO legacy_review_artifact_migration (review_id, artifact_id, migration_status, details_json)
SELECT r.id, NULL, 'content_unrecoverable',
       json_object('reason', 'historical review has no reconstructible terminal structured result')
FROM review r
WHERE r.status IN ('passed', 'failed')
  AND NOT EXISTS (SELECT 1 FROM legacy_review_artifact_migration m WHERE m.review_id = r.id);

DROP TABLE pr8_legacy_review_primary;
DROP TABLE pr8_legacy_review_candidate;

-- V084 and step_results_json are inspected conservatively. A row is backfilled
-- only when a concrete command, result, per-step timestamps, same-Task
-- workspace/commit, and exact durable output reference are all present.
CREATE TEMP TABLE pr8_legacy_ci_candidate AS
WITH step_rows AS (
    SELECT r.id AS review_id, r.task_id, r.execution_id, b.id AS bundle_id,
           b.task_id AS bundle_task_id, b.head_sha, e.task_id AS execution_task_id,
           e.workspace_id, CAST(s.key AS INTEGER) AS step_index, s.value AS step_json
    FROM review r
    LEFT JOIN review_evidence_bundle b ON b.review_id = r.id
    LEFT JOIN execution e ON e.id = b.reviewer_execution_id
    JOIN json_each(CASE
        WHEN json_type(b.ci_results_json) = 'array' THEN b.ci_results_json
        WHEN json_type(r.step_results_json, '$.ci_steps') = 'array'
            THEN json_extract(r.step_results_json, '$.ci_steps')
        WHEN json_type(r.step_results_json) = 'array' THEN r.step_results_json
        ELSE '[]' END) s
)
SELECT s.*,
       COALESCE(json_extract(s.step_json, '$.started_at'),
           (SELECT json_extract(x.value, '$.started_at') FROM review r2
            JOIN json_each(CASE WHEN json_type(r2.step_results_json, '$.ci_steps') = 'array'
                                THEN json_extract(r2.step_results_json, '$.ci_steps')
                                WHEN json_type(r2.step_results_json) = 'array'
                                THEN r2.step_results_json ELSE '[]' END) x
              ON CAST(x.key AS INTEGER) = s.step_index
            WHERE r2.id = s.review_id LIMIT 1)) AS step_started_at,
       COALESCE(json_extract(s.step_json, '$.finished_at'),
           (SELECT json_extract(x.value, '$.finished_at') FROM review r2
            JOIN json_each(CASE WHEN json_type(r2.step_results_json, '$.ci_steps') = 'array'
                                THEN json_extract(r2.step_results_json, '$.ci_steps')
                                WHEN json_type(r2.step_results_json) = 'array'
                                THEN r2.step_results_json ELSE '[]' END) x
              ON CAST(x.key AS INTEGER) = s.step_index
            WHERE r2.id = s.review_id LIMIT 1)) AS step_finished_at
FROM step_rows s;

INSERT INTO legacy_ci_validation_migration (review_id, step_index, validation_run_id, migration_status, details_json)
SELECT c.review_id, c.step_index, NULL,
       CASE
         WHEN c.bundle_id IS NULL THEN 'insufficient_provenance'
         WHEN c.bundle_task_id != c.task_id OR c.execution_task_id != c.task_id THEN 'wrong_task'
         WHEN c.workspace_id IS NULL THEN 'workspace_missing'
         WHEN length(trim(COALESCE(c.head_sha, ''))) = 0 THEN 'commit_identity_missing'
         WHEN length(trim(COALESCE(json_extract(c.step_json, '$.command'), ''))) = 0 THEN 'check_identity_missing'
         WHEN json_type(c.step_json, '$.exit_code') != 'integer' THEN 'status_missing'
         WHEN length(trim(COALESCE(c.step_started_at, ''))) = 0
           OR length(trim(COALESCE(c.step_finished_at, ''))) = 0 THEN 'timestamps_missing'
         WHEN length(trim(COALESCE(json_extract(c.step_json, '$.output_tail'),
                                   json_extract(c.step_json, '$.stderr_tail'), ''))) = 0
           THEN 'output_missing'
         ELSE 'insufficient_provenance'
       END,
       json_object('legacy_bundle_id', c.bundle_id, 'step_index', c.step_index)
FROM pr8_legacy_ci_candidate c;

INSERT INTO legacy_ci_validation_migration (review_id, step_index, validation_run_id, migration_status, details_json)
SELECT r.id, -1, NULL, 'no_steps', json_object('reason', 'no structured deterministic step result')
FROM review r
WHERE NOT EXISTS (SELECT 1 FROM legacy_ci_validation_migration m WHERE m.review_id = r.id);

DROP TABLE pr8_legacy_ci_candidate;
