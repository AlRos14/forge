-- Structured human decision requests raised by the coordinator (or by exact
-- Gate/validation/feedback/replan sources) and the exact resume continuation
-- (plan: coordinator-verification-and-pr-loop, Congelado 2, A4). Additive only.

CREATE TABLE human_decision_request (
    id                       TEXT PRIMARY KEY,
    task_id                  TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    source_kind              TEXT NOT NULL CHECK (source_kind IN ('coordination_request',
                                 'quality_request', 'gate_evaluation', 'validation_run',
                                 'external_feedback', 'coordinator_replan')),
    source_id                TEXT NOT NULL CHECK (length(trim(source_id)) > 0),
    raised_by_execution_id   TEXT,
    authorization_version    INTEGER CHECK (authorization_version IS NULL OR authorization_version >= 1),
    question                 TEXT NOT NULL CHECK (length(question) BETWEEN 1 AND 1024),
    options_json             TEXT NOT NULL CHECK (json_valid(options_json)
                                 AND json_type(options_json) = 'array'
                                 AND json_array_length(options_json) BETWEEN 2 AND 6
                                 AND length(options_json) <= 8192),
    recommended_option_id    TEXT,
    recommendation_rationale TEXT CHECK (recommendation_rationale IS NULL
                                 OR length(recommendation_rationale) <= 2048),
    evidence_links_json      TEXT NOT NULL CHECK (json_valid(evidence_links_json)
                                 AND json_type(evidence_links_json) = 'array'
                                 AND json_array_length(evidence_links_json) <= 16),
    max_authority_effect     TEXT NOT NULL CHECK (max_authority_effect IN ('none', 'scope', 'authority')),
    state                    TEXT NOT NULL DEFAULT 'open'
                                 CHECK (state IN ('open', 'answered', 'superseded', 'cancelled')),
    answered_by_user_id      TEXT REFERENCES user(id) ON DELETE RESTRICT,
    answered_option_id       TEXT,
    answer_note              TEXT CHECK (answer_note IS NULL OR length(answer_note) <= 2048),
    answered_at              TEXT,
    resume_request_id        TEXT REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    resume_binding_json      TEXT NOT NULL CHECK (json_valid(resume_binding_json)
                                 AND json_type(resume_binding_json) = 'object'),
    version                  INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at               TEXT NOT NULL,
    updated_at               TEXT NOT NULL,
    UNIQUE(id, task_id),
    FOREIGN KEY (raised_by_execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT,
    CHECK ((state = 'answered') = (answered_at IS NOT NULL AND answered_option_id IS NOT NULL
           AND answered_by_user_id IS NOT NULL)),
    CHECK (state = 'answered' OR (answered_at IS NULL AND answered_option_id IS NULL
           AND answered_by_user_id IS NULL AND answer_note IS NULL AND resume_request_id IS NULL)),
    CHECK (recommendation_rationale IS NULL OR recommended_option_id IS NOT NULL)
);
-- One question per exact source and authorization.
CREATE UNIQUE INDEX idx_human_decision_exact_source
    ON human_decision_request(source_kind, source_id, COALESCE(authorization_version, 0));
CREATE INDEX idx_human_decision_task_state
    ON human_decision_request(task_id, state, created_at DESC, id DESC);

CREATE TRIGGER human_decision_request_insert_guard
BEFORE INSERT ON human_decision_request
BEGIN
    SELECT CASE
        WHEN NEW.state != 'open' OR NEW.version != 1 OR NEW.resume_request_id IS NOT NULL
            THEN RAISE(ABORT, 'HumanDecisionRequest must be created open')
        -- every option is a bounded object with a unique slug id
        WHEN EXISTS (
            SELECT 1 FROM json_each(NEW.options_json) o
            WHERE o.type != 'object'
               OR json_type(o.value, '$.id') IS NOT 'text'
               OR length(json_extract(o.value, '$.id')) NOT BETWEEN 1 AND 32
               OR json_extract(o.value, '$.id') GLOB '*[^a-z0-9_-]*'
               OR json_type(o.value, '$.label') IS NOT 'text'
               OR length(json_extract(o.value, '$.label')) NOT BETWEEN 1 AND 80
               OR json_type(o.value, '$.description') IS NOT 'text'
               OR length(json_extract(o.value, '$.description')) > 512
               OR json_type(o.value, '$.consequence') IS NOT 'text'
               OR length(json_extract(o.value, '$.consequence')) > 512
               OR json_extract(o.value, '$.authority_effect') IS NOT NULL
                  AND json_extract(o.value, '$.authority_effect') NOT IN ('none', 'scope', 'authority')
               OR json_extract(o.value, '$.authority_effect') IS NULL)
            THEN RAISE(ABORT, 'HumanDecisionRequest options are malformed')
        WHEN (SELECT COUNT(DISTINCT json_extract(o.value, '$.id')) FROM json_each(NEW.options_json) o)
             != json_array_length(NEW.options_json)
            THEN RAISE(ABORT, 'HumanDecisionRequest option ids must be unique')
        WHEN NEW.recommended_option_id IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM json_each(NEW.options_json) o
            WHERE json_extract(o.value, '$.id') = NEW.recommended_option_id)
            THEN RAISE(ABORT, 'HumanDecisionRequest recommendation must name an option')
        WHEN NEW.max_authority_effect != (
            SELECT CASE MAX(CASE json_extract(o.value, '$.authority_effect')
                                WHEN 'authority' THEN 2 WHEN 'scope' THEN 1 ELSE 0 END)
                        WHEN 2 THEN 'authority' WHEN 1 THEN 'scope' ELSE 'none' END
            FROM json_each(NEW.options_json) o)
            THEN RAISE(ABORT, 'HumanDecisionRequest max_authority_effect must equal the option maximum')
        WHEN EXISTS (
            SELECT 1 FROM json_each(NEW.evidence_links_json) e
            WHERE e.type != 'object'
               OR json_extract(e.value, '$.kind') IS NULL
               OR json_extract(e.value, '$.kind') NOT IN ('artifact', 'review_report', 'validation_run',
                    'validation_comparison', 'gate_evaluation', 'execution', 'external_feedback', 'work_unit')
               OR json_type(e.value, '$.id') IS NOT 'text'
               OR length(trim(json_extract(e.value, '$.id'))) = 0
               OR (json_type(e.value, '$.digest') IS NOT NULL AND json_type(e.value, '$.digest') != 'null'
                   AND (json_type(e.value, '$.digest') != 'text'
                        OR length(json_extract(e.value, '$.digest')) != 64)))
            THEN RAISE(ABORT, 'HumanDecisionRequest evidence links are malformed')
        WHEN json_type(NEW.resume_binding_json, '$.request_id') IS NOT 'text'
          OR json_type(NEW.resume_binding_json, '$.request_kind') IS NOT 'text'
          OR json_type(NEW.resume_binding_json, '$.authorization_version') IS NOT 'integer'
          OR json_type(NEW.resume_binding_json, '$.source_subject') IS NOT 'object'
            THEN RAISE(ABORT, 'HumanDecisionRequest resume binding is malformed')
        WHEN NOT EXISTS (
            SELECT 1 FROM task_coordination_request r
            WHERE r.id = json_extract(NEW.resume_binding_json, '$.request_id')
              AND r.task_id = NEW.task_id
              AND r.kind = json_extract(NEW.resume_binding_json, '$.request_kind')
              AND r.authorization_version = json_extract(NEW.resume_binding_json, '$.authorization_version'))
            THEN RAISE(ABORT, 'HumanDecisionRequest resume binding must name an exact request of the Task')
    END;
END;

-- open -> answered|superseded|cancelled (version + 1, nothing else changes);
-- an answered request may additionally record its exact resume request once.
CREATE TRIGGER human_decision_request_update_guard
BEFORE UPDATE ON human_decision_request
WHEN NOT (
    NEW.version = OLD.version + 1
    AND NEW.id IS OLD.id AND NEW.task_id IS OLD.task_id
    AND NEW.source_kind IS OLD.source_kind AND NEW.source_id IS OLD.source_id
    AND NEW.raised_by_execution_id IS OLD.raised_by_execution_id
    AND NEW.authorization_version IS OLD.authorization_version
    AND NEW.question IS OLD.question AND NEW.options_json IS OLD.options_json
    AND NEW.recommended_option_id IS OLD.recommended_option_id
    AND NEW.recommendation_rationale IS OLD.recommendation_rationale
    AND NEW.evidence_links_json IS OLD.evidence_links_json
    AND NEW.max_authority_effect IS OLD.max_authority_effect
    AND NEW.resume_binding_json IS OLD.resume_binding_json
    AND NEW.created_at IS OLD.created_at
    AND (
        (OLD.state = 'open' AND NEW.state IN ('answered', 'superseded', 'cancelled')
         AND NEW.resume_request_id IS NULL)
        OR (OLD.state = 'answered' AND NEW.state = 'answered'
         AND OLD.resume_request_id IS NULL AND NEW.resume_request_id IS NOT NULL
         AND NEW.answered_by_user_id IS OLD.answered_by_user_id
         AND NEW.answered_option_id IS OLD.answered_option_id
         AND NEW.answer_note IS OLD.answer_note AND NEW.answered_at IS OLD.answered_at)
    )
)
BEGIN
    SELECT RAISE(ABORT, 'HumanDecisionRequest identity is immutable and transitions are forward-only');
END;
CREATE TRIGGER human_decision_request_answer_guard
BEFORE UPDATE ON human_decision_request
WHEN NEW.state = 'answered' AND OLD.state = 'open'
 AND NOT EXISTS (
    SELECT 1 FROM json_each(OLD.options_json) o
    WHERE json_extract(o.value, '$.id') = NEW.answered_option_id)
BEGIN
    SELECT RAISE(ABORT, 'HumanDecisionRequest answer must name one of its options');
END;
CREATE TRIGGER human_decision_request_resume_binding_guard
BEFORE UPDATE OF resume_request_id ON human_decision_request
WHEN NEW.resume_request_id IS NOT NULL AND NEW.resume_request_id IS NOT OLD.resume_request_id
 AND NOT EXISTS (
    SELECT 1 FROM task_coordination_request r
    WHERE r.id = NEW.resume_request_id AND r.task_id = NEW.task_id
      AND r.kind = 'human_decision_resume' AND r.source_subject_kind = 'human_decision'
      AND r.source_subject_id = NEW.id
      AND r.resumes_request_id = json_extract(NEW.resume_binding_json, '$.request_id'))
BEGIN
    SELECT RAISE(ABORT, 'HumanDecisionRequest resume request must be its exact human_decision_resume request');
END;
CREATE TRIGGER human_decision_request_immutable_delete
BEFORE DELETE ON human_decision_request
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'HumanDecisionRequest history is preserved outside Project teardown');
END;

-- Source guard for human_decision_resume requests (the kind itself is admitted by
-- V172): the decision must be answered, unresumed, of the same Task and exact
-- authorization, and the resumed request must be the one it was raised against.
CREATE TRIGGER task_coordination_request_human_decision_source_guard
BEFORE INSERT ON task_coordination_request
WHEN NEW.kind = 'human_decision_resume'
 AND NOT EXISTS (
    SELECT 1 FROM human_decision_request d
    WHERE d.id = NEW.source_subject_id AND d.task_id = NEW.task_id
      AND d.state = 'answered' AND d.resume_request_id IS NULL
      AND (d.authorization_version IS NULL OR d.authorization_version = NEW.authorization_version)
      AND json_extract(d.resume_binding_json, '$.request_id') = NEW.resumes_request_id
      AND json_extract(d.resume_binding_json, '$.authorization_version') = NEW.authorization_version)
BEGIN
    SELECT RAISE(ABORT, 'FORGE_COORDINATION_HUMAN_DECISION_STALE');
END;
