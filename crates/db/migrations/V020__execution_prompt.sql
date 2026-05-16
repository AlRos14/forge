ALTER TABLE execution ADD COLUMN prompt TEXT;

UPDATE execution
SET prompt = summary
WHERE prompt IS NULL
  AND summary IS NOT NULL
  AND status = 'running';
