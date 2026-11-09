-- Persist a quality budget for each exact source chain. Review requests share
-- their review-line route across corrections and later review reports, so
-- changing category or target TaskRole cannot reset the route budget.
CREATE TABLE task_quality_route (
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    route_id        TEXT NOT NULL CHECK (length(trim(route_id)) BETWEEN 1 AND 1024),
    route_kind      TEXT NOT NULL CHECK (route_kind IN ('review_line', 'source')),
    route_ref       TEXT NOT NULL CHECK (length(trim(route_ref)) BETWEEN 1 AND 1024),
    request_count   INTEGER NOT NULL DEFAULT 0 CHECK (request_count >= 0),
    budget_used     INTEGER NOT NULL DEFAULT 0 CHECK (budget_used >= 0),
    max_requests    INTEGER NOT NULL DEFAULT 4 CHECK (max_requests > 0),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (task_id, route_id),
    CHECK (budget_used <= max_requests)
);

CREATE INDEX idx_task_quality_route_task_updated
    ON task_quality_route(task_id, updated_at DESC, route_id);

ALTER TABLE task_quality_request
    ADD COLUMN route_id TEXT;
ALTER TABLE task_quality_request
    ADD COLUMN route_round INTEGER NOT NULL DEFAULT 1 CHECK (route_round >= 1);
ALTER TABLE task_quality_request
    ADD COLUMN route_limit INTEGER NOT NULL DEFAULT 4 CHECK (route_limit > 0);
ALTER TABLE task_quality_request
    ADD COLUMN source_task_role_id TEXT;
ALTER TABLE task_quality_request
    ADD COLUMN requires_human_reason_code TEXT;
ALTER TABLE task_quality_request
    ADD COLUMN requires_human_reason TEXT;
ALTER TABLE task_quality_request
    ADD COLUMN cycle_request_ids_json TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(cycle_request_ids_json)
               AND json_type(cycle_request_ids_json) = 'array');
ALTER TABLE task_quality_request
    ADD COLUMN cycle_task_role_ids_json TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(cycle_task_role_ids_json)
               AND json_type(cycle_task_role_ids_json) = 'array');

-- Backfill route identity and order without changing the historical state or
-- replaying any request. A ReviewReport with a durable review line uses that
-- line; other sources keep their exact source identity and digest.
WITH report_lines AS (
    SELECT review_report_artifact_id, MIN(line_id) AS line_id
    FROM work_unit_review_line_revision
    WHERE review_report_artifact_id IS NOT NULL
    GROUP BY review_report_artifact_id
), identified AS (
    SELECT quality.id,
           quality.task_id,
           CASE WHEN report_lines.line_id IS NOT NULL
                THEN 'review_line:' || report_lines.line_id
                ELSE 'source:' || hex(quality.source_kind) || ':' ||
                     hex(quality.source_id) || ':' || quality.source_digest
           END AS route_id,
           CASE WHEN report_lines.line_id IS NOT NULL THEN 'review_line'
                ELSE 'source'
           END AS route_kind,
           CASE WHEN report_lines.line_id IS NOT NULL THEN report_lines.line_id
                ELSE quality.source_id
           END AS route_ref,
           quality.created_at,
           quality.id AS order_id
    FROM task_quality_request quality
    LEFT JOIN report_lines
      ON quality.source_kind = 'review_report'
     AND report_lines.review_report_artifact_id = quality.source_id
), ranked AS (
    SELECT id,
           route_id,
           ROW_NUMBER() OVER (
               PARTITION BY task_id, route_id ORDER BY created_at, order_id
           ) AS route_round
    FROM identified
)
UPDATE task_quality_request
SET route_id = (SELECT identified.route_id FROM identified
                WHERE identified.id = task_quality_request.id),
    route_round = (SELECT ranked.route_round FROM ranked
                   WHERE ranked.id = task_quality_request.id),
    route_limit = 4;

WITH report_lines AS (
    SELECT review_report_artifact_id, MIN(line_id) AS line_id
    FROM work_unit_review_line_revision
    WHERE review_report_artifact_id IS NOT NULL
    GROUP BY review_report_artifact_id
), identified AS (
    SELECT quality.task_id,
           CASE WHEN report_lines.line_id IS NOT NULL
                THEN 'review_line:' || report_lines.line_id
                ELSE 'source:' || hex(quality.source_kind) || ':' ||
                     hex(quality.source_id) || ':' || quality.source_digest
           END AS route_id,
           CASE WHEN report_lines.line_id IS NOT NULL THEN 'review_line'
                ELSE 'source'
           END AS route_kind,
           CASE WHEN report_lines.line_id IS NOT NULL THEN report_lines.line_id
                ELSE quality.source_id
           END AS route_ref,
           quality.created_at,
           quality.updated_at
    FROM task_quality_request quality
    LEFT JOIN report_lines
      ON quality.source_kind = 'review_report'
     AND report_lines.review_report_artifact_id = quality.source_id
)
INSERT INTO task_quality_route (
    task_id, route_id, route_kind, route_ref, request_count, budget_used,
    max_requests, created_at, updated_at
)
SELECT task_id,
       route_id,
       MAX(route_kind),
       MAX(route_ref),
       COUNT(*),
       CASE WHEN COUNT(*) > 4 THEN 4 ELSE COUNT(*) END,
       4,
       MIN(created_at),
       MAX(updated_at)
FROM identified
GROUP BY task_id, route_id;

UPDATE task_quality_request
SET source_task_role_id = (
    SELECT authority.task_role_id
    FROM artifact_execution_producer producer
    JOIN execution_task_role_authority authority
      ON authority.execution_id = producer.execution_id
    WHERE producer.artifact_id = task_quality_request.source_id
      AND task_quality_request.source_kind = 'review_report'
);

-- Older requires_human rows did not persist their escalation reason. Preserve
-- their state and expose that provenance honestly rather than inventing one.
UPDATE task_quality_request
SET requires_human_reason_code = 'coordination.legacy_reason_unavailable',
    requires_human_reason = 'This request was already escalated before V161; its original reason was not persisted.'
WHERE state = 'requires_human'
  AND requires_human_reason_code IS NULL
  AND requires_human_reason IS NULL;

CREATE INDEX idx_task_quality_request_route
    ON task_quality_request(task_id, route_id, route_round);

CREATE TRIGGER task_quality_request_route_insert_guard
BEFORE INSERT ON task_quality_request
WHEN NEW.route_id IS NULL OR NOT EXISTS (
    SELECT 1 FROM task_quality_route route
    WHERE route.task_id = NEW.task_id AND route.route_id = NEW.route_id
)
BEGIN
    SELECT RAISE(ABORT, 'quality request requires an exact same-Task quality route');
END;

CREATE TRIGGER task_quality_request_route_immutable
BEFORE UPDATE ON task_quality_request
WHEN NEW.route_id IS NOT OLD.route_id
  OR NEW.route_round IS NOT OLD.route_round
  OR NEW.route_limit IS NOT OLD.route_limit
  OR NEW.source_task_role_id IS NOT OLD.source_task_role_id
  OR NEW.cycle_request_ids_json IS NOT OLD.cycle_request_ids_json
  OR NEW.cycle_task_role_ids_json IS NOT OLD.cycle_task_role_ids_json
BEGIN
    SELECT RAISE(ABORT, 'quality route identity and cycle evidence are immutable');
END;
