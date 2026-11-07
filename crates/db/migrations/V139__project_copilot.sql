CREATE TABLE project_copilot (
    project_id  TEXT PRIMARY KEY REFERENCES project(id) ON DELETE CASCADE,
    agent_id    TEXT REFERENCES agent_identity(id) ON DELETE SET NULL,
    version     INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

INSERT INTO project_copilot (project_id, agent_id, version, created_at, updated_at)
SELECT id, NULL, 1, created_at, updated_at FROM project;

CREATE TRIGGER project_copilot_create_binding
AFTER INSERT ON project
BEGIN
    INSERT INTO project_copilot (project_id, agent_id, version, created_at, updated_at)
    VALUES (NEW.id, NULL, 1, NEW.created_at, NEW.updated_at);
END;

CREATE TRIGGER project_copilot_agent_delete
BEFORE DELETE ON agent_identity
BEGIN
    UPDATE project_copilot
    SET agent_id = NULL,
        version = version + 1,
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE agent_id = OLD.id;
END;

CREATE TRIGGER project_copilot_agent_archive
AFTER UPDATE OF archived_at ON agent_identity
WHEN OLD.archived_at IS NULL AND NEW.archived_at IS NOT NULL
BEGIN
    UPDATE project_copilot SET agent_id = NULL, version = version + 1,
        updated_at = NEW.updated_at WHERE agent_id = NEW.id;
END;

CREATE TRIGGER project_copilot_agent_owner_change
AFTER UPDATE OF owner_id ON agent_identity
WHEN OLD.owner_id IS NOT NEW.owner_id
BEGIN
    UPDATE project_copilot SET agent_id = NULL, version = version + 1,
        updated_at = NEW.updated_at
    WHERE agent_id = NEW.id AND EXISTS (
        SELECT 1 FROM project WHERE project.id = project_copilot.project_id
            AND project.owner_id IS NOT NEW.owner_id
    );
END;

CREATE TRIGGER project_copilot_project_owner_change
AFTER UPDATE OF owner_id ON project
WHEN OLD.owner_id IS NOT NEW.owner_id
BEGIN
    UPDATE project_copilot SET agent_id = NULL, version = version + 1,
        updated_at = NEW.updated_at
    WHERE project_id = NEW.id AND agent_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM agent_current WHERE agent_current.id = project_copilot.agent_id
            AND agent_current.owner_id = NEW.owner_id
    );
END;

CREATE TABLE project_copilot_run (
    id                       TEXT PRIMARY KEY,
    project_id               TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    agent_id                 TEXT NOT NULL,
    user_id                  TEXT NOT NULL,
    binding_version          INTEGER NOT NULL CHECK (binding_version >= 1),
    agent_version            INTEGER NOT NULL CHECK (agent_version >= 1),
    agent_snapshot_json      TEXT NOT NULL,
    -- Frozen scope: deleting the Task must not broaden this run to Project scope.
    context_task_id          TEXT,
    status                   TEXT NOT NULL CHECK (status IN (
                                 'queued', 'running', 'succeeded', 'failed',
                                 'cancelled', 'interrupted'
                             )),
    idempotency_key          TEXT NOT NULL CHECK (length(trim(idempotency_key)) > 0),
    request_digest           TEXT NOT NULL CHECK (length(request_digest) = 64),
    mcp_token_hash           TEXT,
    mcp_token_expires_at     TEXT,
    failure_code             TEXT,
    failure_message          TEXT,
    created_at               TEXT NOT NULL,
    started_at               TEXT,
    completed_at             TEXT,
    updated_at               TEXT NOT NULL,
    UNIQUE(project_id, idempotency_key),
    CHECK ((mcp_token_hash IS NULL) = (mcp_token_expires_at IS NULL))
);

CREATE UNIQUE INDEX idx_project_copilot_one_active_run
    ON project_copilot_run(project_id)
    WHERE status IN ('queued', 'running');
CREATE INDEX idx_project_copilot_run_project_created
    ON project_copilot_run(project_id, created_at, id);
CREATE UNIQUE INDEX idx_project_copilot_run_mcp_token_hash
    ON project_copilot_run(mcp_token_hash)
    WHERE mcp_token_hash IS NOT NULL;

CREATE TABLE project_copilot_message (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    run_id      TEXT NOT NULL REFERENCES project_copilot_run(id) ON DELETE CASCADE,
    role        TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    actor_type  TEXT NOT NULL CHECK (actor_type IN ('human', 'agent')),
    actor_id    TEXT NOT NULL,
    content     TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    UNIQUE(run_id, role),
    CHECK ((role = 'user' AND actor_type = 'human') OR
           (role = 'assistant' AND actor_type = 'agent'))
);

CREATE INDEX idx_project_copilot_message_project_created
    ON project_copilot_message(project_id, created_at, id);

CREATE TRIGGER project_copilot_message_identity_guard
BEFORE INSERT ON project_copilot_message
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM project_copilot_run AS run
        WHERE run.id = NEW.run_id
          AND run.project_id = NEW.project_id
          AND (
              (NEW.role = 'user' AND NEW.actor_type = 'human'
               AND NEW.actor_id = run.user_id)
              OR
              (NEW.role = 'assistant' AND NEW.actor_type = 'agent'
               AND NEW.actor_id = run.agent_id AND run.status = 'running')
          )
    ) THEN RAISE(ABORT, 'project copilot message identity mismatch') END;
END;

-- Every Project Copilot MCP mutation records an exact run correlation and the
-- Agent actor. This guard shares the caller's write transaction, so any failed
-- authority check rolls back the associated domain mutation as well.
CREATE TRIGGER project_copilot_domain_event_guard
BEFORE INSERT ON domain_event
WHEN EXISTS (
    SELECT 1 FROM project_copilot_run
    WHERE id = NEW.correlation_id
)
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM project_copilot_run AS run
        JOIN project_copilot AS binding
          ON binding.project_id = run.project_id
        JOIN project AS p
          ON p.id = run.project_id
        JOIN agent_current AS agent
          ON agent.id = run.agent_id
        JOIN daemon AS host ON host.id = agent.daemon_id
        JOIN user AS agent_owner ON agent_owner.id = agent.owner_id
        JOIN user AS current_user
          ON current_user.id = run.user_id
        JOIN project_member AS member
          ON member.project_id = run.project_id
         AND member.user_id = run.user_id
        JOIN project_member AS owner_membership
          ON owner_membership.project_id = run.project_id
         AND owner_membership.user_id = p.owner_id
        WHERE run.id = NEW.correlation_id
          AND run.status = 'running'
          AND run.mcp_token_hash IS NOT NULL
          AND run.mcp_token_expires_at IS NOT NULL
          AND julianday(run.mcp_token_expires_at) > julianday('now')
          AND binding.agent_id = run.agent_id
          AND binding.version = run.binding_version
          AND agent.version = run.agent_version
          AND agent.paused = 0
          AND host.status = 'online'
          AND host.visibility IN ('account', 'global')
          AND host.machine_id LIKE 'embedded:%'
          AND agent_owner.is_admin = 1
          AND (host.owner_id IS NULL OR host.owner_id = agent.owner_id)
          AND json_valid(run.agent_snapshot_json)
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.agent_owner_id') IS agent.owner_id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.profile_id') IS agent.profile_id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.executor_type') IS agent.executor_type
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.agent_daemon_id') IS host.id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.daemon_id') IS host.id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.machine_id') IS host.machine_id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.daemon_owner_id') IS host.owner_id
          AND json_extract(run.agent_snapshot_json, '$.daemon_authority.daemon_visibility') IS host.visibility
          AND EXISTS (
              SELECT 1 FROM json_each(CASE WHEN json_valid(host.detected_clis_json)
                  THEN host.detected_clis_json ELSE '[]' END) cli
              WHERE json_extract(cli.value, '$.kind') = agent.executor_type
                AND json_extract(cli.value, '$.availability') = 'authenticated'
          )
          AND p.owner_id IS NOT NULL
          AND p.owner_id = agent.owner_id
          AND owner_membership.role = 'owner'
          AND member.role IN ('owner', 'admin', 'member')
          AND (run.context_task_id IS NULL OR EXISTS (
              SELECT 1 FROM task context_task WHERE context_task.id = run.context_task_id
                AND context_task.project_id = run.project_id AND context_task.deleted_at IS NULL
          ))
          AND NEW.actor_type = 'agent'
          AND NEW.actor_id = run.agent_id
          AND (
              (NEW.scope_type = 'project' AND NEW.scope_id = run.project_id)
              OR (
                  NEW.scope_type = 'task'
                  AND (run.context_task_id IS NULL OR run.context_task_id = NEW.scope_id)
                  AND EXISTS (
                      SELECT 1 FROM task
                      WHERE task.id = NEW.scope_id
                        AND task.project_id = run.project_id
                        AND task.deleted_at IS NULL
                  )
              )
          )
    ) THEN RAISE(ABORT, 'project copilot run authority is stale') END;
END;
