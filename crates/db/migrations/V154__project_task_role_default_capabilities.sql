-- Project role defaults carry configuration templates, but references between
-- roles remain names until Task creation resolves them to local TaskRole IDs.

ALTER TABLE project_task_role_default_role
    ADD COLUMN configuration_json TEXT NOT NULL DEFAULT '{}'
    CHECK (json_valid(configuration_json) AND json_type(configuration_json) = 'object');
