CREATE TABLE project_task_role_defaults (
    project_id  TEXT PRIMARY KEY REFERENCES project(id) ON DELETE CASCADE,
    version     INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    updated_at  TEXT NOT NULL
);

INSERT INTO project_task_role_defaults (project_id, version, updated_at)
SELECT id, 1, updated_at FROM project;

CREATE TABLE project_task_role_default_role (
    project_id          TEXT NOT NULL REFERENCES project_task_role_defaults(project_id) ON DELETE CASCADE,
    role                TEXT NOT NULL CHECK (length(trim(role)) > 0),
    coordination_mode   TEXT NOT NULL CHECK (
        coordination_mode IN ('partitioned', 'collaborative', 'independent')
    ),
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    PRIMARY KEY (project_id, role)
);

CREATE TABLE project_task_role_default_member (
    project_id  TEXT NOT NULL,
    role        TEXT NOT NULL,
    actor_kind  TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id    TEXT NOT NULL CHECK (length(trim(actor_id)) > 0),
    PRIMARY KEY (project_id, role, actor_kind, actor_id),
    FOREIGN KEY (project_id, role)
        REFERENCES project_task_role_default_role(project_id, role) ON DELETE CASCADE
);

CREATE INDEX idx_project_task_role_default_actor
    ON project_task_role_default_member(actor_kind, actor_id, project_id);

CREATE TRIGGER project_task_role_default_human_member_revocation
AFTER DELETE ON project_member
BEGIN
    DELETE FROM project_task_role_default_member
    WHERE project_id = OLD.project_id
      AND actor_kind = 'human'
      AND actor_id = OLD.user_id;
END;

CREATE TRIGGER project_task_role_default_agent_delete
AFTER DELETE ON agent_identity
BEGIN
    DELETE FROM project_task_role_default_member
    WHERE actor_kind = 'agent'
      AND actor_id = OLD.id;
END;

CREATE TRIGGER project_task_role_default_agent_archive
AFTER UPDATE OF archived_at ON agent_identity
WHEN OLD.archived_at IS NULL AND NEW.archived_at IS NOT NULL
BEGIN
    DELETE FROM project_task_role_default_member
    WHERE actor_kind = 'agent'
      AND actor_id = NEW.id;
END;

CREATE TRIGGER project_task_role_default_agent_owner_change
AFTER UPDATE OF owner_id ON agent_identity
WHEN OLD.owner_id IS NOT NEW.owner_id
BEGIN
    DELETE FROM project_task_role_default_member
    WHERE actor_kind = 'agent'
      AND actor_id = NEW.id
      AND EXISTS (
          SELECT 1 FROM project
          WHERE project.id = project_task_role_default_member.project_id
            AND project.owner_id IS NOT NEW.owner_id
      );
END;

CREATE TRIGGER project_task_role_default_owner_change
AFTER UPDATE OF owner_id ON project
WHEN OLD.owner_id IS NOT NEW.owner_id
BEGIN
    DELETE FROM project_task_role_default_member
    WHERE project_id = NEW.id
      AND actor_kind = 'agent'
      AND NOT EXISTS (
          SELECT 1 FROM agent_current
          WHERE agent_current.id = project_task_role_default_member.actor_id
            AND agent_current.owner_id = NEW.owner_id
      );
END;
