ALTER TABLE project ADD COLUMN owner_id TEXT;

ALTER TABLE agent ADD COLUMN owner_id TEXT;
ALTER TABLE agent ADD COLUMN visibility TEXT NOT NULL DEFAULT 'global' CHECK(visibility IN ('global', 'account'));
