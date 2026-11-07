-- Exact review lineage and immutable Artifact bindings for package WorkUnits.

-- GateEvaluation inputs are immutable facts. Rebuild the input table to admit
-- a typed review_line fact while preserving every historical row verbatim.
DROP TRIGGER gate_evaluation_input_same_task_insert;
DROP TRIGGER gate_evaluation_input_immutable_update;
DROP TRIGGER gate_evaluation_input_immutable_delete;
DROP INDEX idx_gate_evaluation_input_ref;

CREATE TABLE gate_evaluation_input_v155 (
    evaluation_id   TEXT NOT NULL,
    task_id         TEXT NOT NULL,
    ordinal         INTEGER NOT NULL CHECK (ordinal >= 0),
    input_kind      TEXT NOT NULL CHECK (input_kind IN (
                        'review_report', 'validation_run', 'evidence', 'decision',
                        'work_unit', 'execution', 'work_unit_integration',
                        'task_role_snapshot', 'merge_operation', 'lifecycle_operation',
                        'review_line'
                    )),
    input_id        TEXT NOT NULL CHECK (length(trim(input_id)) > 0),
    input_version   INTEGER NOT NULL CHECK (input_version >= 1),
    input_digest    TEXT NOT NULL CHECK (length(input_digest) = 64),
    producer_ref    TEXT,
    subject_json    TEXT NOT NULL CHECK (json_valid(subject_json) AND json_type(subject_json) = 'object'),
    status          TEXT NOT NULL CHECK (length(trim(status)) BETWEEN 1 AND 64),
    PRIMARY KEY(evaluation_id, ordinal),
    UNIQUE(evaluation_id, input_kind, input_id),
    FOREIGN KEY (evaluation_id, task_id)
        REFERENCES gate_evaluation(id, task_id) ON DELETE CASCADE
);

INSERT INTO gate_evaluation_input_v155 (
    evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
    input_digest, producer_ref, subject_json, status
)
SELECT evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
       input_digest, producer_ref, subject_json, status
FROM gate_evaluation_input;

PRAGMA legacy_alter_table = ON;
ALTER TABLE gate_evaluation_input RENAME TO gate_evaluation_input_v100;
ALTER TABLE gate_evaluation_input_v155 RENAME TO gate_evaluation_input;
DROP TABLE gate_evaluation_input_v100;
PRAGMA legacy_alter_table = OFF;
CREATE INDEX idx_gate_evaluation_input_ref
    ON gate_evaluation_input(input_kind, input_id, task_id);

CREATE TRIGGER gate_evaluation_input_same_task_insert
BEFORE INSERT ON gate_evaluation_input
WHEN NOT EXISTS (
    SELECT 1 FROM gate_evaluation e
    WHERE e.id = NEW.evaluation_id AND e.task_id = NEW.task_id
)
  OR (NEW.input_kind = 'review_report' AND NOT EXISTS (
      SELECT 1 FROM artifact a
      JOIN artifact_execution_producer p ON p.artifact_id = a.id AND p.task_id = a.task_id
      JOIN execution x ON x.id = p.execution_id AND x.task_id = p.task_id
      WHERE a.id = NEW.input_id AND a.task_id = NEW.task_id AND a.kind = 'review_report'
        AND a.digest = NEW.input_digest AND p.execution_id = NEW.producer_ref
        AND EXISTS (
            SELECT 1 FROM execution_task_role_authority authority
            JOIN json_each(authority.capabilities_json) capability
              ON capability.type = 'text' AND capability.value = 'formal_review'
            WHERE authority.execution_id = x.id
        ) AND x.status = 'completed'
        AND json_extract(a.content, '$.subject.task_id') = NEW.task_id
        AND json_extract(a.content, '$.subject.review_execution_id') = x.id
        AND json_extract(a.content, '$.verdict') = NEW.status
  ))
  OR (NEW.input_kind = 'validation_run' AND NOT EXISTS (
      SELECT 1 FROM validation_run v
      WHERE v.id = NEW.input_id AND v.task_id = NEW.task_id
        AND v.config_digest = json_extract(NEW.subject_json, '$.config_digest')
        AND v.workspace_id = json_extract(NEW.subject_json, '$.workspace_id')
        AND v.commit_sha = json_extract(NEW.subject_json, '$.commit_sha')
        AND v.workspace_snapshot_digest = json_extract(NEW.subject_json, '$.workspace_snapshot_digest')
        AND v.status = NEW.status
  ))
  OR (NEW.input_kind = 'evidence' AND NOT EXISTS (
      SELECT 1 FROM evidence e
      JOIN evidence_validation_run_producer p ON p.evidence_id = e.id AND p.task_id = e.task_id
      WHERE e.id = NEW.input_id AND e.task_id = NEW.task_id
        AND e.digest = NEW.input_digest AND p.validation_run_id = NEW.producer_ref
  ))
  OR (NEW.input_kind = 'decision' AND NOT EXISTS (
      SELECT 1 FROM decision d WHERE d.id = NEW.input_id AND d.task_id = NEW.task_id
        AND d.proposal_id = json_extract(NEW.subject_json, '$.proposal_id')
        AND d.proposal_version = NEW.input_version AND d.outcome = NEW.status
        AND d.policy_ref IS json_extract(NEW.subject_json, '$.policy_ref')
        AND d.policy_version IS json_extract(NEW.subject_json, '$.policy_version')
        AND d.policy_digest IS json_extract(NEW.subject_json, '$.policy_digest')
  ))
  OR (NEW.input_kind = 'work_unit' AND NOT EXISTS (
      SELECT 1 FROM work_unit w WHERE w.id = NEW.input_id AND w.task_id = NEW.task_id
        AND w.version = NEW.input_version AND w.status = NEW.status
  ))
  OR (NEW.input_kind = 'execution' AND NOT EXISTS (
      SELECT 1 FROM execution x WHERE x.id = NEW.input_id AND x.task_id = NEW.task_id
        AND x.work_unit_id IS json_extract(NEW.subject_json, '$.work_unit_id')
        AND x.status = NEW.status
  ))
  OR (NEW.input_kind = 'work_unit_integration' AND NOT EXISTS (
      SELECT 1 FROM work_unit_integration i WHERE i.id = NEW.input_id AND i.task_id = NEW.task_id
        AND i.work_unit_id = json_extract(NEW.subject_json, '$.work_unit_id')
        AND i.execution_id = NEW.producer_ref
        AND i.version = NEW.input_version AND i.outcome = NEW.status
  ))
  OR (NEW.input_kind = 'task_role_snapshot' AND NOT EXISTS (
      SELECT 1 FROM task_role r WHERE r.id = NEW.input_id AND r.task_id = NEW.task_id
        AND r.version = NEW.input_version
  ))
  OR (NEW.input_kind = 'merge_operation' AND NOT EXISTS (
      SELECT 1 FROM task_integration_operation op
      WHERE op.id = NEW.input_id AND op.task_id = NEW.task_id
        AND op.kind = 'task_merge' AND op.version = NEW.input_version AND op.status = NEW.status
  ))
  OR (NEW.input_kind = 'lifecycle_operation' AND NOT EXISTS (
      SELECT 1 FROM task_lifecycle_transition tr
      WHERE tr.id = NEW.input_id AND tr.task_id = NEW.task_id
        AND tr.to_version = NEW.input_version
  ))
BEGIN
    SELECT RAISE(ABORT, 'Gate input is missing, stale, cross-Task, or has invalid provenance');
END;

CREATE TRIGGER gate_evaluation_input_immutable_update
BEFORE UPDATE ON gate_evaluation_input
BEGIN
    SELECT RAISE(ABORT, 'Gate evaluation inputs are immutable');
END;

CREATE TRIGGER gate_evaluation_input_immutable_delete
BEFORE DELETE ON gate_evaluation_input
WHEN NOT EXISTS (
    SELECT 1 FROM gate_evaluation e
    JOIN task t ON t.id = e.task_id
    JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE e.id = OLD.evaluation_id
)
BEGIN
    SELECT RAISE(ABORT, 'Gate evaluation inputs are immutable outside Project teardown');
END;

CREATE TABLE work_unit_artifact_input (
    work_unit_id            TEXT NOT NULL,
    task_id                 TEXT NOT NULL,
    artifact_id             TEXT NOT NULL,
    artifact_digest         TEXT NOT NULL CHECK (length(artifact_digest) > 0),
    producer_execution_id   TEXT NOT NULL,
    created_at              TEXT NOT NULL,
    PRIMARY KEY (work_unit_id, artifact_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (artifact_id, task_id)
        REFERENCES artifact(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (producer_execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT
);

CREATE INDEX idx_work_unit_artifact_input_artifact
    ON work_unit_artifact_input(task_id, artifact_id, work_unit_id);

CREATE TRIGGER work_unit_artifact_input_guard_insert
BEFORE INSERT ON work_unit_artifact_input
WHEN NOT EXISTS (
    SELECT 1 FROM artifact artifact
    JOIN artifact_execution_producer producer
      ON producer.artifact_id = artifact.id AND producer.task_id = artifact.task_id
    JOIN execution execution
      ON execution.id = producer.execution_id AND execution.task_id = producer.task_id
    WHERE artifact.id = NEW.artifact_id AND artifact.task_id = NEW.task_id
      AND artifact.digest = NEW.artifact_digest
      AND producer.execution_id = NEW.producer_execution_id
      AND execution.status = 'completed'
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Artifact input requires its exact completed same-Task producer and digest');
END;

CREATE TRIGGER work_unit_artifact_input_immutable_update
BEFORE UPDATE ON work_unit_artifact_input
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Artifact inputs are immutable');
END;

CREATE TRIGGER work_unit_artifact_input_immutable_delete
BEFORE DELETE ON work_unit_artifact_input
WHEN NOT EXISTS (
    SELECT 1 FROM task task
    JOIN project_deletion_guard guard ON guard.project_id = task.project_id
    WHERE task.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Artifact inputs are immutable outside Project teardown');
END;

CREATE TRIGGER execution_pins_work_unit_artifact_inputs
AFTER INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
BEGIN
    INSERT INTO execution_artifact_input (
        execution_id, artifact_id, task_id, digest, created_at
    )
    SELECT NEW.id, input.artifact_id, NEW.task_id, input.artifact_digest, NEW.created_at
    FROM work_unit_artifact_input input
    WHERE input.work_unit_id = NEW.work_unit_id AND input.task_id = NEW.task_id;
END;

CREATE TABLE work_unit_artifact_manifest (
    id                       TEXT PRIMARY KEY,
    task_id                  TEXT NOT NULL,
    work_unit_id             TEXT NOT NULL,
    work_unit_version        INTEGER NOT NULL CHECK (work_unit_version >= 1),
    execution_id             TEXT NOT NULL,
    delivery_artifact_id     TEXT NOT NULL,
    delivery_artifact_digest TEXT NOT NULL CHECK (length(delivery_artifact_digest) > 0),
    manifest_json            TEXT NOT NULL CHECK (
        json_valid(manifest_json) AND json_type(manifest_json) = 'object'
    ),
    manifest_digest          TEXT NOT NULL CHECK (length(manifest_digest) = 64),
    created_at               TEXT NOT NULL,
    UNIQUE (execution_id, delivery_artifact_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (delivery_artifact_id, task_id)
        REFERENCES artifact(id, task_id) ON DELETE RESTRICT
);

CREATE TRIGGER work_unit_artifact_manifest_guard_insert
BEFORE INSERT ON work_unit_artifact_manifest
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit unit
    JOIN execution execution
      ON execution.id = NEW.execution_id AND execution.task_id = unit.task_id
     AND execution.work_unit_id = unit.id
     AND execution.work_unit_version = NEW.work_unit_version
     AND execution.status = 'completed'
    JOIN artifact artifact
      ON artifact.id = NEW.delivery_artifact_id AND artifact.task_id = unit.task_id
    JOIN artifact_execution_producer producer
      ON producer.artifact_id = artifact.id AND producer.execution_id = execution.id
     AND producer.task_id = unit.task_id
    WHERE unit.id = NEW.work_unit_id AND unit.task_id = NEW.task_id
      AND artifact.digest = NEW.delivery_artifact_digest
)
 OR EXISTS (
    SELECT 1 FROM json_each(NEW.manifest_json, '$.source_artifacts') source
    WHERE source.type <> 'object'
       OR NOT EXISTS (
           SELECT 1 FROM work_unit_artifact_input binding
           WHERE binding.work_unit_id = NEW.work_unit_id
             AND binding.task_id = NEW.task_id
             AND binding.artifact_id = json_extract(source.value, '$.artifact_id')
             AND binding.artifact_digest = json_extract(source.value, '$.digest')
             AND binding.producer_execution_id = json_extract(source.value, '$.producer_execution_id')
       )
)
 OR (SELECT COUNT(*) FROM json_each(NEW.manifest_json, '$.source_artifacts')) != (
    SELECT COUNT(*) FROM work_unit_artifact_input input
    WHERE input.work_unit_id = NEW.work_unit_id AND input.task_id = NEW.task_id
)
 OR json_extract(NEW.manifest_json, '$.delivery_artifact.artifact_id') IS NOT NEW.delivery_artifact_id
 OR json_extract(NEW.manifest_json, '$.delivery_artifact.digest') IS NOT NEW.delivery_artifact_digest
 OR json_extract(NEW.manifest_json, '$.delivery_artifact.producer_execution_id') IS NOT NEW.execution_id
 OR json_extract(NEW.manifest_json, '$.work_unit_id') IS NOT NEW.work_unit_id
 OR json_extract(NEW.manifest_json, '$.work_unit_version') IS NOT NEW.work_unit_version
 OR json_extract(NEW.manifest_json, '$.result_execution_id') IS NOT NEW.execution_id
BEGIN
    SELECT RAISE(ABORT, 'Artifact manifest must pin the exact completed delivery and every WorkUnit source input');
END;

CREATE TRIGGER work_unit_artifact_manifest_immutable_update
BEFORE UPDATE ON work_unit_artifact_manifest
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Artifact manifests are immutable');
END;

CREATE TRIGGER work_unit_artifact_manifest_immutable_delete
BEFORE DELETE ON work_unit_artifact_manifest
WHEN NOT EXISTS (
    SELECT 1 FROM task task
    JOIN project_deletion_guard guard ON guard.project_id = task.project_id
    WHERE task.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Artifact manifests are immutable outside Project teardown');
END;

CREATE TABLE work_unit_review_line (
    id                          TEXT PRIMARY KEY,
    task_id                     TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    root_work_unit_id           TEXT NOT NULL REFERENCES work_unit(id) ON DELETE RESTRICT,
    current_work_unit_id        TEXT NOT NULL REFERENCES work_unit(id) ON DELETE RESTRICT,
    current_work_unit_version   INTEGER NOT NULL CHECK (current_work_unit_version >= 1),
    current_result_execution_id TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    current_result_artifact_id  TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    current_commit_sha          TEXT,
    current_workspace_id        TEXT REFERENCES workspace(id) ON DELETE RESTRICT,
    current_snapshot_digest     TEXT,
    current_manifest_id         TEXT REFERENCES work_unit_artifact_manifest(id) ON DELETE RESTRICT,
    latest_report_artifact_id   TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    latest_verdict              TEXT CHECK (latest_verdict IS NULL OR latest_verdict IN (
                                    'pass', 'request_changes', 'questions', 'blocked'
                                )),
    revision                    INTEGER NOT NULL CHECK (revision >= 1),
    created_at                  TEXT NOT NULL,
    updated_at                  TEXT NOT NULL,
    UNIQUE (task_id, root_work_unit_id),
    CHECK (current_result_execution_id IS NOT NULL OR (
        current_snapshot_digest IS NULL AND current_result_artifact_id IS NULL
        AND current_commit_sha IS NULL AND current_workspace_id IS NULL
        AND current_manifest_id IS NULL
    ))
);

CREATE TABLE work_unit_review_line_revision (
    line_id                     TEXT NOT NULL REFERENCES work_unit_review_line(id) ON DELETE RESTRICT,
    task_id                     TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    revision                    INTEGER NOT NULL CHECK (revision >= 1),
    work_unit_id                TEXT NOT NULL REFERENCES work_unit(id) ON DELETE RESTRICT,
    work_unit_version           INTEGER NOT NULL CHECK (work_unit_version >= 1),
    result_execution_id         TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    result_artifact_id          TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    result_commit_sha           TEXT,
    result_workspace_id         TEXT REFERENCES workspace(id) ON DELETE RESTRICT,
    workspace_snapshot_digest   TEXT,
    manifest_id                 TEXT REFERENCES work_unit_artifact_manifest(id) ON DELETE RESTRICT,
    review_report_artifact_id   TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    verdict                     TEXT CHECK (verdict IS NULL OR verdict IN (
                                    'pass', 'request_changes', 'questions', 'blocked'
                                )),
    created_at                  TEXT NOT NULL,
    PRIMARY KEY (line_id, revision)
);

CREATE INDEX idx_work_unit_review_line_current
    ON work_unit_review_line(task_id, current_work_unit_id, revision);

CREATE TRIGGER work_unit_review_line_revision_insert_guard
BEFORE INSERT ON work_unit_review_line_revision
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit_review_line line
    JOIN work_unit unit ON unit.id = NEW.work_unit_id AND unit.task_id = NEW.task_id
    WHERE line.id = NEW.line_id AND line.task_id = NEW.task_id
      AND line.revision = NEW.revision
      AND line.current_work_unit_id = NEW.work_unit_id
      AND line.current_work_unit_version = NEW.work_unit_version
      AND line.current_result_execution_id IS NEW.result_execution_id
      AND line.current_result_artifact_id IS NEW.result_artifact_id
      AND line.current_commit_sha IS NEW.result_commit_sha
      AND line.current_workspace_id IS NEW.result_workspace_id
      AND line.current_snapshot_digest IS NEW.workspace_snapshot_digest
      AND line.current_manifest_id IS NEW.manifest_id
      AND line.latest_report_artifact_id IS NEW.review_report_artifact_id
      AND line.latest_verdict IS NEW.verdict
)
BEGIN
    SELECT RAISE(ABORT, 'review line revision must snapshot its exact current state');
END;

CREATE TRIGGER work_unit_review_line_revision_immutable_update
BEFORE UPDATE ON work_unit_review_line_revision
BEGIN
    SELECT RAISE(ABORT, 'review line revisions are immutable');
END;

CREATE TRIGGER work_unit_review_line_revision_immutable_delete
BEFORE DELETE ON work_unit_review_line_revision
WHEN NOT EXISTS (
    SELECT 1 FROM task task
    JOIN project_deletion_guard guard ON guard.project_id = task.project_id
    WHERE task.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'review line revisions are immutable outside Project teardown');
END;

CREATE TRIGGER work_unit_review_line_identity_immutable
BEFORE UPDATE ON work_unit_review_line
WHEN NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.root_work_unit_id IS NOT OLD.root_work_unit_id
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.revision != OLD.revision + 1
BEGIN
    SELECT RAISE(ABORT, 'review line identity is immutable and revisions advance exactly once');
END;

CREATE TRIGGER work_unit_review_line_revision_snapshot_insert
AFTER INSERT ON work_unit_review_line
BEGIN
    INSERT INTO work_unit_review_line_revision (
        line_id, task_id, revision, work_unit_id, work_unit_version,
        result_execution_id, result_artifact_id, result_commit_sha,
        result_workspace_id, workspace_snapshot_digest, manifest_id,
        review_report_artifact_id, verdict, created_at
    ) VALUES (
        NEW.id, NEW.task_id, NEW.revision, NEW.current_work_unit_id,
        NEW.current_work_unit_version, NEW.current_result_execution_id,
        NEW.current_result_artifact_id, NEW.current_commit_sha,
        NEW.current_workspace_id, NEW.current_snapshot_digest,
        NEW.current_manifest_id, NEW.latest_report_artifact_id,
        NEW.latest_verdict, NEW.created_at
    );
END;

CREATE TRIGGER work_unit_review_line_revision_snapshot_update
AFTER UPDATE ON work_unit_review_line
BEGIN
    INSERT INTO work_unit_review_line_revision (
        line_id, task_id, revision, work_unit_id, work_unit_version,
        result_execution_id, result_artifact_id, result_commit_sha,
        result_workspace_id, workspace_snapshot_digest, manifest_id,
        review_report_artifact_id, verdict, created_at
    ) VALUES (
        NEW.id, NEW.task_id, NEW.revision, NEW.current_work_unit_id,
        NEW.current_work_unit_version, NEW.current_result_execution_id,
        NEW.current_result_artifact_id, NEW.current_commit_sha,
        NEW.current_workspace_id, NEW.current_snapshot_digest,
        NEW.current_manifest_id, NEW.latest_report_artifact_id,
        NEW.latest_verdict, NEW.updated_at
    );
END;

CREATE TRIGGER work_unit_review_line_from_exact_subject_insert
AFTER INSERT ON work_unit_review_subject
WHEN NEW.result_execution_id IS NOT NULL
BEGIN
    INSERT OR IGNORE INTO work_unit_review_line (
        id, task_id, root_work_unit_id, current_work_unit_id,
        current_work_unit_version, current_result_execution_id,
        current_result_artifact_id, current_commit_sha, current_workspace_id,
        current_snapshot_digest, current_manifest_id, latest_report_artifact_id,
        latest_verdict, revision, created_at, updated_at
    )
    SELECT lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4'
             || lower(substr(hex(randomblob(2)), 2, 3)) || '-'
             || substr('89ab', 1 + (abs(random()) % 4), 1)
             || lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
           NEW.task_id, NEW.reviewed_work_unit_id, NEW.reviewed_work_unit_id,
           NEW.reviewed_work_unit_version, NEW.result_execution_id,
           NEW.result_artifact_id, NEW.result_commit_sha, NEW.result_workspace_id,
           NEW.workspace_snapshot_digest,
           (SELECT manifest.id FROM work_unit_artifact_manifest manifest
            WHERE manifest.execution_id = NEW.result_execution_id
              AND manifest.delivery_artifact_id = NEW.result_artifact_id),
           NULL, NULL, 1, NEW.created_at, NEW.created_at
    WHERE NOT EXISTS (
        SELECT 1 FROM task_rework_receipt receipt
        WHERE receipt.review_work_unit_id = NEW.review_work_unit_id
    )
    ON CONFLICT(task_id, root_work_unit_id) DO NOTHING;
END;

CREATE TRIGGER work_unit_review_line_from_exact_subject_update
AFTER UPDATE OF result_execution_id, result_artifact_id, result_commit_sha,
    result_workspace_id, workspace_snapshot_digest ON work_unit_review_subject
WHEN NEW.result_execution_id IS NOT NULL
BEGIN
    INSERT OR IGNORE INTO work_unit_review_line (
        id, task_id, root_work_unit_id, current_work_unit_id,
        current_work_unit_version, current_result_execution_id,
        current_result_artifact_id, current_commit_sha, current_workspace_id,
        current_snapshot_digest, current_manifest_id, latest_report_artifact_id,
        latest_verdict, revision, created_at, updated_at
    )
    SELECT lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4'
             || lower(substr(hex(randomblob(2)), 2, 3)) || '-'
             || substr('89ab', 1 + (abs(random()) % 4), 1)
             || lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
           NEW.task_id, NEW.reviewed_work_unit_id, NEW.reviewed_work_unit_id,
           NEW.reviewed_work_unit_version, NEW.result_execution_id,
           NEW.result_artifact_id, NEW.result_commit_sha, NEW.result_workspace_id,
           NEW.workspace_snapshot_digest,
           (SELECT manifest.id FROM work_unit_artifact_manifest manifest
            WHERE manifest.execution_id = NEW.result_execution_id
              AND manifest.delivery_artifact_id = NEW.result_artifact_id),
           NULL, NULL, 1, NEW.created_at, NEW.created_at
    WHERE NOT EXISTS (
        SELECT 1 FROM task_rework_receipt receipt
        WHERE receipt.review_work_unit_id = NEW.review_work_unit_id
    )
    ON CONFLICT(task_id, root_work_unit_id) DO NOTHING;

    UPDATE work_unit_review_line
       SET current_work_unit_id = NEW.reviewed_work_unit_id,
           current_work_unit_version = NEW.reviewed_work_unit_version,
           current_result_execution_id = NEW.result_execution_id,
           current_result_artifact_id = NEW.result_artifact_id,
           current_commit_sha = NEW.result_commit_sha,
           current_workspace_id = NEW.result_workspace_id,
           current_snapshot_digest = NEW.workspace_snapshot_digest,
           current_manifest_id = (SELECT manifest.id FROM work_unit_artifact_manifest manifest
             WHERE manifest.execution_id = NEW.result_execution_id
               AND manifest.delivery_artifact_id = NEW.result_artifact_id),
           latest_report_artifact_id = NULL, latest_verdict = NULL,
           revision = revision + 1, updated_at = NEW.created_at
     WHERE EXISTS (
         SELECT 1 FROM task_rework_receipt receipt
         WHERE receipt.review_work_unit_id = NEW.review_work_unit_id
           AND receipt.task_id = NEW.task_id
     )
       AND id = (
           SELECT receipt_line.id FROM task_rework_receipt receipt_line
           JOIN work_unit_review_line_revision revision
             ON revision.review_report_artifact_id = receipt_line.review_report_artifact_id
           WHERE receipt_line.review_work_unit_id = NEW.review_work_unit_id
             AND receipt_line.task_id = NEW.task_id
           LIMIT 1
       );
END;

CREATE TRIGGER work_unit_review_line_advance_on_rework
AFTER INSERT ON task_rework_receipt
BEGIN
    UPDATE work_unit_review_line
       SET current_work_unit_id = NEW.correction_work_unit_id,
           current_work_unit_version = (SELECT version FROM work_unit WHERE id = NEW.correction_work_unit_id),
           current_result_execution_id = NULL,
           current_result_artifact_id = NULL,
           current_commit_sha = NULL,
           current_workspace_id = NULL,
           current_snapshot_digest = NULL,
           current_manifest_id = NULL,
           latest_report_artifact_id = NULL,
           latest_verdict = NULL,
           revision = revision + 1, updated_at = NEW.created_at
     WHERE task_id = NEW.task_id
       AND latest_report_artifact_id = NEW.review_report_artifact_id;
END;

CREATE TRIGGER work_unit_review_line_report_created
AFTER INSERT ON artifact
WHEN NEW.kind = 'review_report'
BEGIN
    UPDATE work_unit_review_line
       SET latest_report_artifact_id = NEW.id,
           latest_verdict = json_extract(NEW.content, '$.verdict'),
           revision = revision + 1, updated_at = NEW.created_at
     WHERE task_id = NEW.task_id
       AND EXISTS (
           SELECT 1 FROM artifact_execution_producer producer
           JOIN execution review_execution
             ON review_execution.id = producer.execution_id AND review_execution.task_id = producer.task_id
           JOIN work_unit_review_subject subject
             ON subject.review_work_unit_id = review_execution.work_unit_id
            AND subject.task_id = review_execution.task_id
           WHERE producer.artifact_id = NEW.id AND producer.task_id = NEW.task_id
             AND subject.reviewed_work_unit_id = work_unit_review_line.current_work_unit_id
             AND subject.reviewed_work_unit_version = work_unit_review_line.current_work_unit_version
             AND subject.result_execution_id = work_unit_review_line.current_result_execution_id
             AND json_extract(NEW.content, '$.subject.reviewed_work_unit_id') = subject.reviewed_work_unit_id
             AND json_extract(NEW.content, '$.subject.reviewed_work_unit_version') = subject.reviewed_work_unit_version
       );
END;

ALTER TABLE work_unit ADD COLUMN review_approval_gate_id TEXT REFERENCES gate(id) ON DELETE RESTRICT;
ALTER TABLE work_unit ADD COLUMN review_approval_line_id TEXT REFERENCES work_unit_review_line(id) ON DELETE RESTRICT;

CREATE TRIGGER work_unit_review_approval_same_task_insert
BEFORE INSERT ON work_unit
WHEN (NEW.review_approval_gate_id IS NULL) != (NEW.review_approval_line_id IS NULL)
  OR (NEW.review_approval_gate_id IS NOT NULL AND NOT EXISTS (
      SELECT 1 FROM gate gate
      JOIN work_unit_review_line line ON line.id = NEW.review_approval_line_id
      JOIN gate_policy_revision policy
        ON policy.gate_id = gate.id AND policy.revision = gate.active_policy_revision
      WHERE gate.id = NEW.review_approval_gate_id AND gate.task_id = NEW.task_id
        AND line.task_id = NEW.task_id
        AND json_extract(policy.policy_json, '$.review.review_line_id') = line.id
  ))
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit review approval must bind a same-Task Gate policy to an exact review line');
END;

CREATE TRIGGER work_unit_review_approval_immutable_update
BEFORE UPDATE OF review_approval_gate_id, review_approval_line_id ON work_unit
WHEN NEW.review_approval_gate_id IS NOT OLD.review_approval_gate_id
  OR NEW.review_approval_line_id IS NOT OLD.review_approval_line_id
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit review approval requirements are immutable');
END;

CREATE TRIGGER work_unit_review_approval_admission_guard
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL AND EXISTS (
    SELECT 1 FROM work_unit consumer
    WHERE consumer.id = NEW.work_unit_id AND consumer.task_id = NEW.task_id
      AND consumer.review_approval_gate_id IS NOT NULL
      AND NOT EXISTS (
          SELECT 1 FROM work_unit_review_line line
          JOIN gate gate ON gate.id = consumer.review_approval_gate_id AND gate.task_id = consumer.task_id
          JOIN gate_evaluation evaluation
            ON evaluation.gate_id = gate.id AND evaluation.task_id = gate.task_id
           AND evaluation.outcome = 'satisfied'
           AND evaluation.policy_revision = gate.active_policy_revision
          JOIN gate_evaluation_input input
            ON input.evaluation_id = evaluation.id AND input.task_id = evaluation.task_id
           AND input.input_kind = 'review_line' AND input.input_id = line.id
           AND input.input_version = line.revision AND input.status = line.latest_verdict
          WHERE line.id = consumer.review_approval_line_id AND line.task_id = consumer.task_id
            AND line.latest_verdict = 'pass'
            AND line.current_result_execution_id IS NOT NULL
            AND evaluation.id = (
                SELECT latest.id FROM gate_evaluation latest
                WHERE latest.gate_id = gate.id
                ORDER BY latest.evaluated_at DESC, latest.id DESC LIMIT 1
            )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit requires a current satisfied Gate evaluation over its exact latest passing review line');
END;

CREATE TRIGGER gate_evaluation_input_review_line_guard
BEFORE INSERT ON gate_evaluation_input
WHEN NEW.input_kind = 'review_line' AND NOT EXISTS (
    SELECT 1 FROM work_unit_review_line line
    WHERE line.id = NEW.input_id AND line.task_id = NEW.task_id
      AND line.revision = NEW.input_version
      AND COALESCE(line.latest_verdict, 'pending') IS NEW.status
      AND NEW.input_digest = json_extract(NEW.subject_json, '$.line_digest')
      AND json_extract(NEW.subject_json, '$.revision') = line.revision
      AND json_extract(NEW.subject_json, '$.current_work_unit_id') = line.current_work_unit_id
      AND json_extract(NEW.subject_json, '$.current_result_execution_id') IS line.current_result_execution_id
      AND json_extract(NEW.subject_json, '$.latest_report_artifact_id') IS line.latest_report_artifact_id
)
BEGIN
    SELECT RAISE(ABORT, 'Gate review-line input must pin its exact current revision and verdict');
END;

CREATE TRIGGER work_unit_review_line_immutable_delete
BEFORE DELETE ON work_unit_review_line
WHEN NOT EXISTS (
    SELECT 1 FROM task task
    JOIN project_deletion_guard guard ON guard.project_id = task.project_id
    WHERE task.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'review lines are immutable outside Project teardown');
END;
