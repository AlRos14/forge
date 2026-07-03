-- Normalize legacy interruption kinds to the closed FailureKind vocabulary.
-- Data-preserving: only the classification field is rewritten; rows with
-- malformed JSON are left untouched (json_valid guards). After this
-- migration, runtime classification relies exclusively on the structured
-- kind — reason/message prose carries no classification weight.

-- error_annotation `type` aliases
UPDATE task
SET error_annotation = json_set(error_annotation, '$.type', 'retry_exhausted')
WHERE error_annotation IS NOT NULL
  AND json_valid(error_annotation)
  AND json_extract(error_annotation, '$.type') = 'retry_budget_exhausted';

UPDATE task
SET error_annotation = json_set(error_annotation, '$.type', 'executor_failed')
WHERE error_annotation IS NOT NULL
  AND json_valid(error_annotation)
  AND json_extract(error_annotation, '$.type') = 'crash';

UPDATE task
SET error_annotation = json_set(error_annotation, '$.type', 'before_work_hook_failed')
WHERE error_annotation IS NOT NULL
  AND json_valid(error_annotation)
  AND json_extract(error_annotation, '$.type') = 'hook_failed';

-- Annotations previously classified as budget-exhausted only by their
-- blocking_reason phrasing gain the structured kind.
UPDATE task
SET error_annotation = json_set(error_annotation, '$.type', 'retry_exhausted')
WHERE error_annotation IS NOT NULL
  AND json_valid(error_annotation)
  AND COALESCE(json_extract(error_annotation, '$.type'), '')
      NOT IN ('review_budget_exhausted', 'retry_exhausted', 'merge_fix_budget_exhausted')
  AND (instr(lower(COALESCE(json_extract(error_annotation, '$.blocking_reason'), '')), 'retry budget exhausted') > 0
    OR instr(lower(COALESCE(json_extract(error_annotation, '$.blocking_reason'), '')), 'rejection budget exhausted') > 0);

-- blocked_json `kind` aliases
UPDATE task
SET blocked_json = json_set(blocked_json, '$.kind', 'retry_exhausted')
WHERE blocked_json IS NOT NULL
  AND json_valid(blocked_json)
  AND json_extract(blocked_json, '$.kind') = 'retry_budget_exhausted';

UPDATE task
SET blocked_json = json_set(blocked_json, '$.kind', 'executor_failed')
WHERE blocked_json IS NOT NULL
  AND json_valid(blocked_json)
  AND json_extract(blocked_json, '$.kind') = 'crash';

-- Blocked rows previously classified as retry-exhausted only by their
-- reason phrasing gain the structured kind.
UPDATE task
SET blocked_json = json_set(blocked_json, '$.kind', 'retry_exhausted')
WHERE blocked_json IS NOT NULL
  AND json_valid(blocked_json)
  AND COALESCE(json_extract(blocked_json, '$.kind'), '')
      NOT IN ('review_gate_failed', 'retry_exhausted', 'merge_fix_budget_exhausted')
  AND (instr(COALESCE(json_extract(blocked_json, '$.reason'), ''), 'retry budget exhausted') > 0
    OR instr(COALESCE(json_extract(blocked_json, '$.reason'), ''), 'rejection budget exhausted') > 0);

-- failed_json `kind` aliases
UPDATE task
SET failed_json = json_set(failed_json, '$.kind', 'executor_failed')
WHERE failed_json IS NOT NULL
  AND json_valid(failed_json)
  AND json_extract(failed_json, '$.kind') = 'crash';
