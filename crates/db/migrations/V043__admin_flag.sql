ALTER TABLE user ADD COLUMN is_admin INTEGER NOT NULL DEFAULT 0;

-- On upgrade, promote the earliest-created user to admin when bootstrap is already complete.
-- This ensures existing installs always have at least one admin after migration.
UPDATE user SET is_admin = 1
WHERE id = (SELECT id FROM user ORDER BY created_at ASC LIMIT 1)
  AND EXISTS (SELECT 1 FROM system_setting WHERE key = 'bootstrap_completed');
