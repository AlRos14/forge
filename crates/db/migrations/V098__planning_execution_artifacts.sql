-- Plan PR7 moves plan output onto generic Artifacts and pins exact Artifact
-- inputs to Executions. V081 remains immutable historical evidence.

CREATE UNIQUE INDEX idx_artifact_execution_producer_exact
    ON artifact_execution_producer(artifact_id, execution_id, task_id);

CREATE TABLE execution_artifact_output (
    execution_id TEXT NOT NULL,
    artifact_id  TEXT NOT NULL UNIQUE,
    task_id      TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK (length(trim(kind)) > 0),
    digest       TEXT,
    created_at   TEXT NOT NULL,
    PRIMARY KEY (execution_id, kind),
    FOREIGN KEY (execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (artifact_id, task_id)
        REFERENCES artifact(id, task_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (artifact_id, execution_id, task_id)
        REFERENCES artifact_execution_producer(artifact_id, execution_id, task_id)
        ON DELETE RESTRICT
);
CREATE INDEX idx_execution_artifact_output_artifact
    ON execution_artifact_output(artifact_id);

CREATE TRIGGER execution_artifact_output_guard_insert
BEFORE INSERT ON execution_artifact_output
WHEN (NEW.kind = 'plan' AND NOT EXISTS (
    SELECT 1 FROM execution e
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id AND e.purpose = 'plan'
)) OR EXISTS (
    SELECT 1 FROM artifact a
    WHERE a.id = NEW.artifact_id
      AND (a.task_id != NEW.task_id OR a.kind != NEW.kind OR a.digest IS NOT NEW.digest)
)
BEGIN
    SELECT RAISE(ABORT, 'Execution output Artifact purpose, kind, digest, or Task is invalid');
END;

CREATE TRIGGER execution_artifact_output_match_artifact_insert
BEFORE INSERT ON artifact
WHEN EXISTS (
    SELECT 1 FROM execution_artifact_output o
    WHERE o.artifact_id = NEW.id
      AND (o.task_id != NEW.task_id OR o.kind != NEW.kind OR o.digest IS NOT NEW.digest)
)
BEGIN
    SELECT RAISE(ABORT, 'Artifact does not match its Execution output binding');
END;

CREATE TRIGGER plan_artifact_output_required_insert
BEFORE INSERT ON artifact
WHEN NEW.kind = 'plan' AND NOT EXISTS (
    SELECT 1 FROM execution_artifact_output o
    JOIN artifact_execution_producer p
      ON p.artifact_id = o.artifact_id
     AND p.execution_id = o.execution_id
     AND p.task_id = o.task_id
    JOIN execution e ON e.id = p.execution_id AND e.task_id = p.task_id
    WHERE o.artifact_id = NEW.id AND o.task_id = NEW.task_id
      AND o.kind = NEW.kind AND o.digest IS NEW.digest AND e.purpose = 'plan'
      AND e.actor_kind IN ('human', 'agent') AND e.actor_id IS NOT NULL
      AND ((e.actor_kind = 'human' AND EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
        OR (e.actor_kind = 'agent' AND EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id)))
)
BEGIN
    SELECT RAISE(ABORT, 'Plan Artifact requires its exact attributed Plan Execution output binding');
END;

CREATE TRIGGER execution_artifact_output_immutable_update
BEFORE UPDATE ON execution_artifact_output
BEGIN
    SELECT RAISE(ABORT, 'Execution output Artifact bindings are immutable');
END;

CREATE TRIGGER execution_artifact_output_immutable_delete
BEFORE DELETE ON execution_artifact_output
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Execution output Artifact bindings are immutable outside Project teardown');
END;

CREATE TABLE execution_artifact_input (
    execution_id TEXT NOT NULL,
    artifact_id  TEXT NOT NULL,
    task_id      TEXT NOT NULL,
    digest       TEXT,
    created_at   TEXT NOT NULL,
    PRIMARY KEY (execution_id, artifact_id),
    FOREIGN KEY (execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (artifact_id, task_id)
        REFERENCES artifact(id, task_id) ON DELETE RESTRICT,
    CHECK (digest IS NULL OR length(trim(digest)) > 0)
);
CREATE INDEX idx_execution_artifact_input_artifact
    ON execution_artifact_input(artifact_id, execution_id);

CREATE TRIGGER execution_artifact_input_guard_insert
BEFORE INSERT ON execution_artifact_input
WHEN NOT EXISTS (
    SELECT 1 FROM execution e
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
      AND e.status = 'running' AND e.logs_path IS NULL
) OR NOT EXISTS (
    SELECT 1 FROM artifact a
    WHERE a.id = NEW.artifact_id
      AND a.task_id = NEW.task_id
      AND a.digest IS NEW.digest
)
BEGIN
    SELECT RAISE(ABORT, 'Execution input Artifact digest or Task is invalid');
END;

CREATE TRIGGER execution_artifact_input_immutable_update
BEFORE UPDATE ON execution_artifact_input
BEGIN
    SELECT RAISE(ABORT, 'Execution input Artifact bindings are immutable');
END;

CREATE TRIGGER execution_artifact_input_immutable_delete
BEFORE DELETE ON execution_artifact_input
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Execution input Artifact bindings are immutable outside Project teardown');
END;

-- This table is migration audit only. A NULL Artifact id means that the
-- historical row is retained but had no verifiable producer, or conflicted
-- with another output from the same Execution.
CREATE TABLE legacy_task_plan_artifact_migration (
    plan_revision_id TEXT PRIMARY KEY
        REFERENCES task_plan_revision(id) ON DELETE RESTRICT,
    artifact_id      TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    migration_status TEXT NOT NULL CHECK (migration_status IN (
        'migrated', 'mapped_duplicate', 'source_execution_missing',
        'source_execution_unresolved', 'source_execution_output_ambiguous',
        'existing_output_conflict'
    )),
    details_json     TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(details_json))
);
CREATE INDEX idx_legacy_task_plan_artifact_migration_artifact
    ON legacy_task_plan_artifact_migration(artifact_id);

-- Existing generic plan Artifacts are durable already. Bind an output identity
-- only where one same-Execution plan Artifact makes the result unambiguous.
WITH single_outputs AS (
    SELECT p.execution_id, p.task_id, a.id AS artifact_id, a.digest, a.created_at
    FROM artifact_execution_producer p
    JOIN artifact a ON a.id = p.artifact_id AND a.task_id = p.task_id
    JOIN execution e ON e.id = p.execution_id AND e.task_id = p.task_id
    WHERE a.kind = 'plan'
      AND a.digest IS NOT NULL
      AND e.purpose = 'plan'
      AND 1 = (
          SELECT COUNT(*)
          FROM artifact_execution_producer p2
          JOIN artifact a2 ON a2.id = p2.artifact_id AND a2.task_id = p2.task_id
          JOIN execution e2 ON e2.id = p2.execution_id AND e2.task_id = p2.task_id
          WHERE p2.execution_id = p.execution_id AND a2.kind = 'plan'
            AND e2.purpose = 'plan'
      )
)
INSERT INTO execution_artifact_output (execution_id, artifact_id, task_id, kind, digest, created_at)
SELECT execution_id, artifact_id, task_id, 'plan', digest, created_at
FROM single_outputs;

-- Only rows with a same-Task, persisted ActorRef are migratable. V081's
-- planner_ready result is preferred when several checkpoints describe one
-- Execution output. Multiple digests for one Execution remain explicit issues.
CREATE TEMP TABLE pr7_legacy_plan_output AS
WITH verified AS (
    SELECT r.id, r.task_id, r.source_execution_id, r.markdown, r.content_digest, r.created_at,
           e.actor_kind, e.actor_id,
           ROW_NUMBER() OVER (
               PARTITION BY r.source_execution_id
               ORDER BY CASE r.checkpoint WHEN 'planner_ready' THEN 0 ELSE 1 END,
                        r.created_at ASC, r.id ASC
           ) AS row_number
    FROM task_plan_revision r
    JOIN execution e ON e.id = r.source_execution_id AND e.task_id = r.task_id
    WHERE r.source_execution_id IS NOT NULL
      AND e.actor_kind IN ('human', 'agent')
      AND e.purpose = 'plan'
      AND e.actor_id IS NOT NULL
      AND (SELECT COUNT(DISTINCT other.content_digest)
           FROM task_plan_revision other
           WHERE other.source_execution_id = r.source_execution_id) = 1
      AND ((e.actor_kind = 'human' AND EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
        OR (e.actor_kind = 'agent' AND EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id)))
)
SELECT id AS source_revision_id, task_id, source_execution_id, markdown, content_digest,
       created_at, actor_kind, actor_id
FROM verified
WHERE row_number = 1
  AND NOT EXISTS (
      SELECT 1 FROM execution_artifact_output o
      WHERE o.execution_id = verified.source_execution_id AND o.kind = 'plan'
        AND o.digest IS NOT verified.content_digest
  )
  AND NOT EXISTS (
      SELECT 1
      FROM artifact_execution_producer p
      JOIN artifact a ON a.id = p.artifact_id AND a.task_id = p.task_id
      WHERE p.execution_id = verified.source_execution_id AND a.kind = 'plan'
        AND NOT EXISTS (
            SELECT 1 FROM execution_artifact_output o
            WHERE o.execution_id = p.execution_id AND o.artifact_id = a.id
              AND o.kind = 'plan' AND o.digest IS a.digest
        )
  );

CREATE TEMP TABLE pr7_legacy_plan_artifact AS
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random() % 4)), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))) AS artifact_id,
    source_revision_id, task_id, source_execution_id, markdown, content_digest, created_at
FROM pr7_legacy_plan_output
WHERE NOT EXISTS (
    SELECT 1 FROM execution_artifact_output o
    WHERE o.execution_id = pr7_legacy_plan_output.source_execution_id AND o.kind = 'plan'
      AND o.digest IS pr7_legacy_plan_output.content_digest
);

INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
SELECT artifact_id, source_execution_id, task_id FROM pr7_legacy_plan_artifact;

INSERT INTO execution_artifact_output (execution_id, artifact_id, task_id, kind, digest, created_at)
SELECT source_execution_id, artifact_id, task_id, 'plan', content_digest, created_at
FROM pr7_legacy_plan_artifact;

INSERT INTO artifact (
    id, task_id, kind, storage_kind, content, content_ref, metadata_json, digest, created_at
)
SELECT artifact_id, task_id, 'plan', 'inline', markdown, NULL,
       json_object('migration', 'V098', 'source_revision_id', source_revision_id),
       content_digest, created_at
FROM pr7_legacy_plan_artifact;

INSERT INTO legacy_task_plan_artifact_migration (
    plan_revision_id, artifact_id, migration_status, details_json
)
SELECT
    r.id,
    CASE
        WHEN r.source_execution_id IS NOT NULL THEN (
            SELECT o.artifact_id FROM execution_artifact_output o
            WHERE o.execution_id = r.source_execution_id AND o.task_id = r.task_id
              AND o.kind = 'plan' AND o.digest IS r.content_digest
        )
        WHEN r.checkpoint = 'approved' THEN (
            SELECT o.artifact_id FROM execution_artifact_output o
            WHERE o.task_id = r.task_id AND o.kind = 'plan' AND o.digest IS r.content_digest
            ORDER BY o.created_at ASC, o.artifact_id ASC LIMIT 1
        )
        ELSE NULL
    END,
    CASE
        WHEN r.source_execution_id IS NOT NULL AND EXISTS (
            SELECT 1 FROM execution_artifact_output o
            WHERE o.execution_id = r.source_execution_id AND o.task_id = r.task_id
              AND o.kind = 'plan' AND o.digest IS r.content_digest
        ) THEN CASE
            WHEN r.checkpoint = 'approved' THEN 'mapped_duplicate'
            WHEN EXISTS (
                SELECT 1 FROM pr7_legacy_plan_artifact a WHERE a.source_revision_id = r.id
            ) THEN 'migrated'
            ELSE 'mapped_duplicate'
        END
        WHEN r.checkpoint = 'approved' AND EXISTS (
            SELECT 1 FROM execution_artifact_output o
            WHERE o.task_id = r.task_id AND o.kind = 'plan' AND o.digest IS r.content_digest
        ) THEN 'mapped_duplicate'
        WHEN r.source_execution_id IS NULL THEN 'source_execution_missing'
        WHEN NOT EXISTS (
            SELECT 1 FROM execution e WHERE e.id = r.source_execution_id AND e.task_id = r.task_id
              AND e.purpose = 'plan' AND e.actor_id IS NOT NULL
              AND ((e.actor_kind = 'human' AND EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
                OR (e.actor_kind = 'agent' AND EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id)))
        ) THEN 'source_execution_unresolved'
        WHEN (SELECT COUNT(DISTINCT r2.content_digest) FROM task_plan_revision r2
              WHERE r2.source_execution_id = r.source_execution_id) > 1
            THEN 'source_execution_output_ambiguous'
        ELSE 'existing_output_conflict'
    END,
    json_object(
        'checkpoint', r.checkpoint,
        'source_execution_id', r.source_execution_id,
        'content_digest', r.content_digest
    )
FROM task_plan_revision r;

DROP TABLE pr7_legacy_plan_artifact;
DROP TABLE pr7_legacy_plan_output;
