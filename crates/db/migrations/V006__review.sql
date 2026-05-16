DROP INDEX IF EXISTS idx_review_task_attempt;
DROP TABLE IF EXISTS review;

CREATE TABLE review (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    execution_id        TEXT NOT NULL REFERENCES execution(id) ON DELETE CASCADE,
    attempt_number      INTEGER NOT NULL,
    status              TEXT NOT NULL CHECK (status IN ('running', 'passed', 'failed', 'cancelled')),
    step_results_json   TEXT NOT NULL DEFAULT '[]',
    started_at          TEXT NOT NULL,
    finished_at         TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    UNIQUE(task_id, attempt_number)
);
CREATE INDEX idx_review_task_attempt ON review(task_id, attempt_number);

ALTER TABLE workspace ADD COLUMN cleanup_after TEXT;
