CREATE TABLE task_decision_request (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    execution_id        TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    role                TEXT NOT NULL CHECK (role IN ('planner', 'reviewer')),
    authority_scope     TEXT NOT NULL CHECK (authority_scope IN ('task', 'project_scope', 'policy', 'risk')),
    questions_json      TEXT NOT NULL CHECK (json_valid(questions_json)),
    context             TEXT,
    status              TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'answered', 'cancelled')),
    created_at          TEXT NOT NULL,
    UNIQUE(execution_id)
);

CREATE TABLE task_decision_answer (
    id                  TEXT PRIMARY KEY,
    request_id          TEXT NOT NULL UNIQUE REFERENCES task_decision_request(id) ON DELETE RESTRICT,
    principal_type      TEXT NOT NULL CHECK (principal_type = 'user'),
    principal_id        TEXT NOT NULL,
    answers_json        TEXT NOT NULL CHECK (json_valid(answers_json)),
    answered_at         TEXT NOT NULL
);

CREATE TRIGGER task_decision_request_content_immutable
BEFORE UPDATE OF task_id, execution_id, role, authority_scope, questions_json, context, created_at
ON task_decision_request BEGIN
    SELECT RAISE(ABORT, 'Task decision request content is immutable');
END;

CREATE TRIGGER task_decision_answer_immutable
BEFORE UPDATE ON task_decision_answer BEGIN
    SELECT RAISE(ABORT, 'Task decision answers are immutable');
END;
