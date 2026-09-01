CREATE TABLE review_evidence_bundle (
    id                      TEXT PRIMARY KEY,
    review_id               TEXT NOT NULL UNIQUE REFERENCES review(id) ON DELETE CASCADE,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    reviewer_execution_id   TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    plan_revision_id        TEXT REFERENCES task_plan_revision(id) ON DELETE RESTRICT,
    plan_digest             TEXT,
    base_sha                TEXT NOT NULL,
    head_sha                TEXT NOT NULL,
    diff_text               TEXT NOT NULL,
    diff_digest             TEXT NOT NULL,
    ci_results_json         TEXT NOT NULL CHECK (json_valid(ci_results_json)),
    fresh_session           INTEGER NOT NULL CHECK (fresh_session IN (0, 1)),
    created_at              TEXT NOT NULL
);

CREATE TRIGGER review_evidence_bundle_immutable
BEFORE UPDATE ON review_evidence_bundle BEGIN
    SELECT RAISE(ABORT, 'Review evidence bundles are immutable');
END;
