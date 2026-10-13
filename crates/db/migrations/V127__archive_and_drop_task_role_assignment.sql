-- RoleMembership is the only current role authority. Preserve every remaining
-- singleton row byte-for-byte as historical provenance, then remove the
-- runtime compatibility table and its projection triggers.
CREATE TABLE task_role_assignment_history (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL,
    role_name       TEXT NOT NULL,
    assignee_type   TEXT,
    assignee_id     TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

INSERT INTO task_role_assignment_history (
    id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at
)
SELECT id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at
FROM task_role_assignment;

CREATE TRIGGER task_role_assignment_history_no_insert
BEFORE INSERT ON task_role_assignment_history
BEGIN
    SELECT RAISE(ABORT, 'TaskRole assignment provenance is migration-only');
END;
CREATE TRIGGER task_role_assignment_history_immutable_update
BEFORE UPDATE ON task_role_assignment_history
BEGIN
    SELECT RAISE(ABORT, 'TaskRole assignment provenance is immutable');
END;
CREATE TRIGGER task_role_assignment_history_immutable_delete
BEFORE DELETE ON task_role_assignment_history
BEGIN
    SELECT RAISE(ABORT, 'TaskRole assignment provenance is immutable');
END;

DROP TRIGGER role_membership_agent_delete_projection;
DROP TRIGGER role_membership_human_delete_projection;
DROP TRIGGER task_role_assignment_insert_requires_assignee_id;
DROP INDEX idx_task_role_assignee;
DROP INDEX idx_task_role_task;
DROP TABLE task_role_assignment;

-- Actor identity deletion closes current target memberships while keeping the
-- immutable membership rows and removing all writes to singleton projections.
CREATE TRIGGER role_membership_agent_delete_cleanup
AFTER DELETE ON agent_identity
BEGIN
    UPDATE role_membership
    SET status = 'ended',
        ended_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
        version = version + 1
    WHERE actor_kind = 'agent'
      AND actor_id = OLD.id
      AND status IN ('active', 'suspended');
END;

CREATE TRIGGER role_membership_human_delete_cleanup
AFTER DELETE ON user
BEGIN
    UPDATE role_membership
    SET status = 'ended',
        ended_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
        version = version + 1
    WHERE actor_kind = 'human'
      AND actor_id = OLD.id
      AND status IN ('active', 'suspended');
END;
