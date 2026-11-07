-- Dynamic review/correction work keeps exact provenance and a bounded rework
-- budget. These relations are not a workflow DSL: they record which result a
-- WorkUnit consumes and which exact result a review WorkUnit must inspect.

CREATE TABLE task_rework_receipt (
    id                         TEXT PRIMARY KEY,
    task_id                    TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    idempotency_key            TEXT NOT NULL CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    request_digest             TEXT NOT NULL CHECK (length(request_digest) = 64),
    review_report_artifact_id  TEXT NOT NULL REFERENCES artifact(id) ON DELETE RESTRICT,
    reviewed_work_unit_id      TEXT NOT NULL,
    reviewed_work_unit_version INTEGER NOT NULL CHECK (reviewed_work_unit_version >= 1),
    correction_work_unit_id    TEXT NOT NULL REFERENCES work_unit(id) ON DELETE RESTRICT,
    review_work_unit_id        TEXT NOT NULL REFERENCES work_unit(id) ON DELETE RESTRICT,
    round                      INTEGER NOT NULL CHECK (round >= 1),
    created_at                 TEXT NOT NULL,
    UNIQUE(task_id, idempotency_key),
    UNIQUE(task_id, round)
);
CREATE INDEX idx_task_rework_receipt_task_created
    ON task_rework_receipt(task_id, created_at DESC, id DESC);

CREATE TABLE work_unit_result_requirement (
    work_unit_id             TEXT PRIMARY KEY REFERENCES work_unit(id) ON DELETE CASCADE,
    task_id                  TEXT NOT NULL,
    source_work_unit_id      TEXT NOT NULL,
    source_work_unit_version INTEGER NOT NULL CHECK (source_work_unit_version >= 1),
    result_execution_id      TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    result_artifact_id       TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    result_commit_sha        TEXT,
    result_workspace_id      TEXT REFERENCES workspace(id) ON DELETE RESTRICT,
    workspace_snapshot_digest TEXT,
    created_at               TEXT NOT NULL,
    CHECK (work_unit_id != source_work_unit_id)
);
CREATE INDEX idx_work_unit_result_requirement_source
    ON work_unit_result_requirement(task_id, source_work_unit_id);

CREATE TABLE work_unit_review_subject (
    review_work_unit_id       TEXT PRIMARY KEY REFERENCES work_unit(id) ON DELETE CASCADE,
    task_id                   TEXT NOT NULL,
    reviewed_work_unit_id     TEXT NOT NULL,
    reviewed_work_unit_version INTEGER NOT NULL CHECK (reviewed_work_unit_version >= 1),
    result_execution_id       TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    result_artifact_id        TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    result_commit_sha         TEXT,
    result_workspace_id       TEXT REFERENCES workspace(id) ON DELETE RESTRICT,
    workspace_snapshot_digest TEXT,
    created_at                TEXT NOT NULL,
    CHECK (review_work_unit_id != reviewed_work_unit_id)
);
CREATE INDEX idx_work_unit_review_subject_reviewed
    ON work_unit_review_subject(task_id, reviewed_work_unit_id, reviewed_work_unit_version);

CREATE TRIGGER task_rework_receipt_same_task_insert
BEFORE INSERT ON task_rework_receipt
WHEN NOT EXISTS (
    SELECT 1 FROM artifact a
    WHERE a.id = NEW.review_report_artifact_id
      AND a.task_id = NEW.task_id
      AND a.kind = 'review_report'
)
   OR NOT EXISTS (
    SELECT 1 FROM work_unit w
    WHERE w.id = NEW.reviewed_work_unit_id AND w.task_id = NEW.task_id
)
   OR NOT EXISTS (
    SELECT 1 FROM work_unit c
    JOIN work_unit r ON r.id = NEW.review_work_unit_id AND r.task_id = NEW.task_id
    WHERE c.id = NEW.correction_work_unit_id AND c.task_id = NEW.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'rework receipt must reference same-Task exact records');
END;

CREATE TRIGGER task_rework_receipt_immutable_update
BEFORE UPDATE ON task_rework_receipt
BEGIN
    SELECT RAISE(ABORT, 'rework receipts are immutable');
END;
CREATE TRIGGER task_rework_receipt_immutable_delete
BEFORE DELETE ON task_rework_receipt
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'rework receipts are immutable outside Project teardown');
END;

CREATE TRIGGER work_unit_result_requirement_same_task_insert
BEFORE INSERT ON work_unit_result_requirement
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit w
    WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
)
   OR NOT EXISTS (
    SELECT 1 FROM work_unit s
    WHERE s.id = NEW.source_work_unit_id AND s.task_id = NEW.task_id
)
   OR NOT EXISTS (
    SELECT 1 FROM execution e
    WHERE e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
      AND e.work_unit_id = NEW.source_work_unit_id
      AND e.work_unit_version = NEW.source_work_unit_version
      AND e.status = 'completed'
)
   OR (NEW.result_artifact_id IS NOT NULL AND NOT EXISTS (
       SELECT 1 FROM artifact a
       WHERE a.id = NEW.result_artifact_id AND a.task_id = NEW.task_id
   ))
   OR (NEW.result_workspace_id IS NOT NULL AND NOT EXISTS (
       SELECT 1 FROM workspace w
       WHERE w.id = NEW.result_workspace_id AND w.task_id = NEW.task_id
   ))
BEGIN
    SELECT RAISE(ABORT, 'result requirement must reference same-Task exact completed result');
END;

CREATE TRIGGER work_unit_result_requirement_immutable_update
BEFORE UPDATE ON work_unit_result_requirement
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit result requirements are immutable');
END;
CREATE TRIGGER work_unit_result_requirement_immutable_delete
BEFORE DELETE ON work_unit_result_requirement
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit result requirements are immutable outside Project teardown');
END;

CREATE TRIGGER work_unit_review_subject_same_task_insert
BEFORE INSERT ON work_unit_review_subject
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit w
    WHERE w.id = NEW.review_work_unit_id AND w.task_id = NEW.task_id
      AND w.role = 'reviewer'
)
   OR NOT EXISTS (
    SELECT 1 FROM work_unit w
    WHERE w.id = NEW.reviewed_work_unit_id AND w.task_id = NEW.task_id
      AND w.version = NEW.reviewed_work_unit_version
)
   OR (NEW.result_execution_id IS NOT NULL AND NOT EXISTS (
       SELECT 1 FROM execution e
       WHERE e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
         AND e.work_unit_id = NEW.reviewed_work_unit_id
         AND e.work_unit_version = NEW.reviewed_work_unit_version
         AND e.status = 'completed'
   ))
   OR (NEW.result_artifact_id IS NOT NULL AND NOT EXISTS (
       SELECT 1 FROM artifact a
       WHERE a.id = NEW.result_artifact_id AND a.task_id = NEW.task_id
   ))
   OR (NEW.result_workspace_id IS NOT NULL AND NOT EXISTS (
       SELECT 1 FROM workspace w
       WHERE w.id = NEW.result_workspace_id AND w.task_id = NEW.task_id
   ))
BEGIN
    SELECT RAISE(ABORT, 'review subject must reference same-Task exact WorkUnit and result');
END;

CREATE TRIGGER work_unit_review_subject_immutable_update
BEFORE UPDATE ON work_unit_review_subject
WHEN NEW.review_work_unit_id IS NOT OLD.review_work_unit_id
  OR NEW.task_id IS NOT OLD.task_id
  OR NEW.reviewed_work_unit_id IS NOT OLD.reviewed_work_unit_id
  OR NEW.reviewed_work_unit_version IS NOT OLD.reviewed_work_unit_version
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'review subject identity is immutable');
END;
CREATE TRIGGER work_unit_review_subject_immutable_delete
BEFORE DELETE ON work_unit_review_subject
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'review subjects are immutable outside Project teardown');
END;
