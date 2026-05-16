UPDATE agent
SET visibility = 'account'
WHERE owner_id IS NOT NULL
  AND visibility = 'global'
  AND EXISTS (
      SELECT 1
      FROM system_setting
      WHERE key = 'bootstrap_completed'
        AND value = 'true'
  );

UPDATE daemon
SET visibility = 'account'
WHERE owner_id IS NOT NULL
  AND visibility = 'global'
  AND EXISTS (
      SELECT 1
      FROM system_setting
      WHERE key = 'bootstrap_completed'
        AND value = 'true'
  );
