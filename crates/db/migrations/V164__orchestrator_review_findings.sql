CREATE TABLE task_review_triage (
    review_report_artifact_id TEXT PRIMARY KEY REFERENCES artifact(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    report_digest TEXT NOT NULL,
    execution_id TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    authorization_version INTEGER NOT NULL,
    disposition TEXT NOT NULL CHECK(disposition IN ('correct','recheck','human')),
    rationale TEXT NOT NULL,
    instructions TEXT NOT NULL,
    decision_digest TEXT NOT NULL,
    review_work_unit_id TEXT REFERENCES work_unit(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL
);
CREATE TRIGGER task_review_triage_immutable
BEFORE UPDATE ON task_review_triage
BEGIN
    SELECT RAISE(ABORT, 'Review triage is immutable evidence, not a review verdict');
END;
CREATE TRIGGER task_review_triage_immutable_delete
BEFORE DELETE ON task_review_triage
WHEN NOT EXISTS(SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id=t.project_id WHERE t.id=OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'Review triage is preserved outside Project teardown');
END;
