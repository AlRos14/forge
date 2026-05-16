ALTER TABLE execution ADD COLUMN stop_reason TEXT;
ALTER TABLE execution ADD COLUMN stopped_by TEXT;
ALTER TABLE execution ADD COLUMN resume_policy TEXT;
ALTER TABLE execution ADD COLUMN stopped_at TEXT;

UPDATE execution
SET stop_reason = 'legacy_unknown',
    resume_policy = 'manual'
WHERE status IN ('cancelled', 'failed')
  AND stop_reason IS NULL;
