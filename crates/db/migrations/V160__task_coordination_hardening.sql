-- A coordination authorization binds one exact team snapshot. The originals
-- were only compared during service preflight, leaving a window where a
-- concurrent TaskRole or membership mutation could become effective after the
-- caller read the team but before and during the protected write. Mark the
-- authorization stale inside every team write so materialization, request
-- admission, and Provider Start can all fail closed under their own lock.

ALTER TABLE task_coordination
    ADD COLUMN team_stale INTEGER NOT NULL DEFAULT 0
        CHECK (team_stale IN (0, 1));
ALTER TABLE task_coordination
    ADD COLUMN team_generation INTEGER NOT NULL DEFAULT 0
        CHECK (team_generation >= 0);

CREATE TRIGGER task_coordination_team_stale_task_role_insert
AFTER INSERT ON task_role
BEGIN
    UPDATE task_coordination
    SET team_stale = 1,
        team_generation = team_generation + 1,
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_id = NEW.task_id;
END;

CREATE TRIGGER task_coordination_team_stale_task_role_update
AFTER UPDATE ON task_role
BEGIN
    UPDATE task_coordination
    SET team_stale = 1,
        team_generation = team_generation + 1,
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_id = NEW.task_id;
END;

CREATE TRIGGER task_coordination_team_stale_task_role_delete
AFTER DELETE ON task_role
BEGIN
    UPDATE task_coordination
    SET team_stale = 1,
        team_generation = team_generation + 1,
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_id = OLD.task_id;
END;

CREATE TRIGGER task_coordination_team_stale_capability_insert
AFTER INSERT ON task_role_capability_configuration
BEGIN
    UPDATE task_coordination
    SET team_stale = 1,
        team_generation = team_generation + 1,
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_id = (
        SELECT task_id FROM task_role WHERE id = NEW.task_role_id
    );
END;

CREATE TRIGGER task_coordination_team_stale_capability_update
AFTER UPDATE ON task_role_capability_configuration
BEGIN
    UPDATE task_coordination
    SET team_stale = 1,
        team_generation = team_generation + 1,
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_id = (
        SELECT task_id FROM task_role WHERE id = NEW.task_role_id
    );
END;

CREATE TRIGGER task_coordination_team_stale_capability_delete
AFTER DELETE ON task_role_capability_configuration
BEGIN
    UPDATE task_coordination
    SET team_stale = 1,
        team_generation = team_generation + 1,
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_id = (
        SELECT task_id FROM task_role WHERE id = OLD.task_role_id
    );
END;

CREATE TRIGGER task_coordination_team_stale_membership_insert
AFTER INSERT ON role_membership
BEGIN
    UPDATE task_coordination
    SET team_stale = 1,
        team_generation = team_generation + 1,
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_id = (
        SELECT task_id FROM task_role WHERE id = NEW.task_role_id
    );
END;

CREATE TRIGGER task_coordination_team_stale_membership_update
AFTER UPDATE ON role_membership
BEGIN
    UPDATE task_coordination
    SET team_stale = 1,
        team_generation = team_generation + 1,
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_id = (
        SELECT task_id FROM task_role WHERE id = NEW.task_role_id
    );
END;

CREATE TRIGGER task_coordination_team_stale_membership_delete
AFTER DELETE ON role_membership
BEGIN
    UPDATE task_coordination
    SET team_stale = 1,
        team_generation = team_generation + 1,
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_id = (
        SELECT task_id FROM task_role WHERE id = OLD.task_role_id
    );
END;

-- Quality requests need an exact link to the coordination turn that answers a
-- plan-revision route, and a durable reference to the evidence that resolved
-- the request. Without these columns the request could only remain at
-- `notified` with no inspectable response.
ALTER TABLE task_quality_request
    ADD COLUMN coordination_request_id TEXT
        REFERENCES task_coordination_request(id) ON DELETE RESTRICT;
ALTER TABLE task_quality_request
    ADD COLUMN resolution_kind TEXT
        CHECK (resolution_kind IS NULL
               OR resolution_kind IN ('execution', 'review_report', 'plan',
                                      'human', 'work_unit'));
ALTER TABLE task_quality_request
    ADD COLUMN resolution_ref TEXT
        CHECK (resolution_ref IS NULL OR length(trim(resolution_ref)) > 0);
ALTER TABLE task_quality_request
    ADD COLUMN resolution_digest TEXT
        CHECK (resolution_digest IS NULL OR length(resolution_digest) = 64);
