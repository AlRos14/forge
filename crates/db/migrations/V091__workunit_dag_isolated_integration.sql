-- Plan PR5 adds executable WorkUnit scope, a same-Task dependency DAG,
-- WorkUnit-scoped workspaces/leases, and durable explicit integration.
-- Workspace is rebuilt to remove its historical task_id UNIQUE constraint.
PRAGMA foreign_keys = OFF;

CREATE TABLE work_unit (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    parent_work_unit_id     TEXT,
    title                   TEXT NOT NULL CHECK (length(trim(title)) > 0),
    scope                   TEXT NOT NULL CHECK (length(trim(scope)) > 0),
    status                  TEXT NOT NULL DEFAULT 'open'
                                CHECK (status IN ('open', 'completed', 'cancelled')),
    role                    TEXT NOT NULL CHECK (length(trim(role)) > 0),
    assigned_actor_kind     TEXT CHECK (assigned_actor_kind IN ('human', 'agent')),
    assigned_actor_id       TEXT CHECK (assigned_actor_id IS NULL OR length(trim(assigned_actor_id)) > 0),
    requires_integration    INTEGER NOT NULL DEFAULT 1 CHECK (requires_integration IN (0, 1)),
    provenance_kind         TEXT,
    provenance_id           TEXT,
    created_by_kind         TEXT NOT NULL CHECK (created_by_kind IN ('human', 'agent')),
    created_by_id           TEXT NOT NULL CHECK (length(trim(created_by_id)) > 0),
    version                 INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    UNIQUE(id, task_id),
    FOREIGN KEY (task_id, role) REFERENCES task_role(task_id, role) ON DELETE RESTRICT,
    FOREIGN KEY (parent_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    CHECK ((assigned_actor_kind IS NULL) = (assigned_actor_id IS NULL)),
    CHECK ((provenance_kind IS NULL) = (provenance_id IS NULL)),
    CHECK (provenance_kind IS NULL OR provenance_kind IN ('actor', 'work_unit', 'artifact', 'external'))
);
CREATE INDEX idx_work_unit_task_created ON work_unit(task_id, created_at, id);
CREATE INDEX idx_work_unit_task_status ON work_unit(task_id, status, id);
CREATE INDEX idx_work_unit_allocation ON work_unit(task_id, role, assigned_actor_kind, assigned_actor_id);

CREATE TRIGGER work_unit_actor_guard_insert
BEFORE INSERT ON work_unit
WHEN (NEW.created_by_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user u WHERE u.id = NEW.created_by_id
      ))
  OR (NEW.created_by_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.created_by_id
      ))
  OR (NEW.assigned_actor_kind = 'human' AND NOT EXISTS (
          SELECT 1
          FROM task_role tr JOIN role_membership rm ON rm.task_role_id = tr.id
          WHERE tr.task_id = NEW.task_id AND tr.role = NEW.role
            AND rm.actor_kind = NEW.assigned_actor_kind AND rm.actor_id = NEW.assigned_actor_id
            AND rm.status = 'active'
      ))
  OR (NEW.assigned_actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1
          FROM task_role tr JOIN role_membership rm ON rm.task_role_id = tr.id
          WHERE tr.task_id = NEW.task_id AND tr.role = NEW.role
            AND rm.actor_kind = NEW.assigned_actor_kind AND rm.actor_id = NEW.assigned_actor_id
            AND rm.status = 'active'
      ))
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit creator or allocation is not an eligible TaskRole Actor');
END;

CREATE TRIGGER work_unit_actor_guard_update
BEFORE UPDATE OF assigned_actor_kind, assigned_actor_id, role ON work_unit
WHEN (NEW.assigned_actor_kind IS NOT NULL AND NOT EXISTS (
          SELECT 1
          FROM task_role tr JOIN role_membership rm ON rm.task_role_id = tr.id
          WHERE tr.task_id = NEW.task_id AND tr.role = NEW.role
            AND rm.actor_kind = NEW.assigned_actor_kind AND rm.actor_id = NEW.assigned_actor_id
            AND rm.status = 'active'
      ))
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit allocation is not an active TaskRole membership');
END;

CREATE TRIGGER work_unit_terminal_guard_update
BEFORE UPDATE ON work_unit
WHEN OLD.status IN ('completed', 'cancelled') AND NEW.status != OLD.status
BEGIN
    SELECT RAISE(ABORT, 'terminal WorkUnit lifecycle is immutable');
END;

CREATE TRIGGER work_unit_delete_guard
BEFORE DELETE ON work_unit
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnits are removed only during guarded Project teardown');
END;

CREATE TABLE work_unit_dependency (
    task_id                     TEXT NOT NULL,
    work_unit_id                TEXT NOT NULL,
    depends_on_work_unit_id     TEXT NOT NULL,
    created_by_kind             TEXT NOT NULL CHECK (created_by_kind IN ('human', 'agent')),
    created_by_id               TEXT NOT NULL CHECK (length(trim(created_by_id)) > 0),
    created_at                  TEXT NOT NULL,
    PRIMARY KEY (work_unit_id, depends_on_work_unit_id),
    CHECK (work_unit_id != depends_on_work_unit_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (depends_on_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT
);
CREATE INDEX idx_work_unit_dependency_prerequisite
    ON work_unit_dependency(task_id, depends_on_work_unit_id, work_unit_id);

CREATE TRIGGER work_unit_dependency_cycle_guard_insert
BEFORE INSERT ON work_unit_dependency
WHEN EXISTS (
    WITH RECURSIVE reachable(id) AS (
        SELECT NEW.depends_on_work_unit_id
        UNION
        SELECT d.depends_on_work_unit_id
        FROM work_unit_dependency d JOIN reachable r ON d.work_unit_id = r.id
        WHERE d.task_id = NEW.task_id
    )
    SELECT 1 FROM reachable WHERE id = NEW.work_unit_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit dependency would create a cycle');
END;

CREATE TRIGGER work_unit_dependency_creator_guard_insert
BEFORE INSERT ON work_unit_dependency
WHEN (NEW.created_by_kind = 'human' AND NOT EXISTS (SELECT 1 FROM user u WHERE u.id = NEW.created_by_id))
  OR (NEW.created_by_kind = 'agent' AND NOT EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.created_by_id))
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit dependency creator is invalid');
END;

-- Keep every historical workspace row exactly as it was, but classify it as
-- the Task integration workspace. No WorkUnit is inferred for old data.
DROP INDEX IF EXISTS idx_workspace_task;
-- SQLite reparses every trigger during table rename. The PR1 trigger used a
-- target-table qualifier that SQLite rejects during that reparse; preserve
-- its invariant using the trigger's stable NEW row reference.
DROP TRIGGER task_role_coordination_mode_guard_update;
DROP TRIGGER proposal_actor_target_guard_insert;
CREATE TABLE workspace_new (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    repo_id         TEXT NOT NULL REFERENCES repo(id) ON DELETE CASCADE,
    worktree_path   TEXT NOT NULL,
    branch          TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('creating', 'ready', 'error', 'cleaning', 'cleaned')),
    before_sha      TEXT,
    error           TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    cleanup_after   TEXT,
    UNIQUE(id, task_id),
    UNIQUE(worktree_path)
);
INSERT INTO workspace_new (
    id, task_id, repo_id, worktree_path, branch, status, before_sha, error,
    created_at, updated_at, cleanup_after
)
SELECT id, task_id, repo_id, worktree_path, branch, status, before_sha, error,
       created_at, updated_at, cleanup_after
FROM workspace;
DROP TABLE workspace;
ALTER TABLE workspace_new RENAME TO workspace;
CREATE TRIGGER task_role_coordination_mode_guard_update
BEFORE UPDATE OF coordination_mode ON task_role
WHEN NEW.coordination_mode IS NULL
 AND EXISTS (
     SELECT 1
     FROM role_membership
     WHERE role_membership.task_role_id = NEW.id
       AND role_membership.status IN ('active', 'suspended')
     GROUP BY role_membership.task_role_id
     HAVING COUNT(*) > 1
 )
BEGIN
    SELECT RAISE(ABORT, 'TaskRole coordination_mode is required for multiple current members');
END;
CREATE INDEX idx_workspace_task ON workspace(task_id);
CREATE INDEX idx_workspace_repo_branch ON workspace(repo_id, branch);

CREATE TABLE workspace_scope (
    workspace_id    TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL,
    scope_kind      TEXT NOT NULL CHECK (scope_kind IN ('integration', 'work_unit')),
    work_unit_id    TEXT,
    created_at      TEXT NOT NULL,
    UNIQUE(workspace_id, task_id),
    FOREIGN KEY (workspace_id, task_id)
        REFERENCES workspace(id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    CHECK ((scope_kind = 'integration' AND work_unit_id IS NULL)
        OR (scope_kind = 'work_unit' AND work_unit_id IS NOT NULL))
);
INSERT INTO workspace_scope (workspace_id, task_id, scope_kind, work_unit_id, created_at)
SELECT id, task_id, 'integration', NULL, created_at FROM workspace;
CREATE UNIQUE INDEX idx_workspace_scope_integration
    ON workspace_scope(task_id) WHERE scope_kind = 'integration';
CREATE UNIQUE INDEX idx_workspace_scope_work_unit
    ON workspace_scope(work_unit_id) WHERE scope_kind = 'work_unit';
CREATE INDEX idx_workspace_scope_task ON workspace_scope(task_id, scope_kind, workspace_id);

CREATE TRIGGER workspace_scope_insert_guard
BEFORE INSERT ON workspace_scope
WHEN (NEW.scope_kind = 'integration' AND EXISTS (
          SELECT 1 FROM workspace_scope s
          WHERE s.task_id = NEW.task_id AND s.scope_kind = 'integration'
      ))
  OR (NEW.scope_kind = 'work_unit' AND NOT EXISTS (
          SELECT 1 FROM work_unit w
          WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
            AND w.requires_integration = 1
      ))
  OR NOT EXISTS (
      SELECT 1
      FROM workspace ws
      JOIN task t ON t.id = ws.task_id
      JOIN repo r ON r.id = ws.repo_id
      WHERE ws.id = NEW.workspace_id
        AND ws.task_id = NEW.task_id
        AND ws.repo_id = t.repo_id
        AND r.project_id = t.project_id
  )
BEGIN
    SELECT RAISE(ABORT, 'Workspace scope is duplicate, cross-Task, or cross-repository');
END;
CREATE TRIGGER workspace_scope_immutable_update
BEFORE UPDATE ON workspace_scope
BEGIN
    SELECT RAISE(ABORT, 'Workspace scope identity is immutable');
END;
ALTER TABLE execution ADD COLUMN work_unit_id TEXT REFERENCES work_unit(id) ON DELETE RESTRICT;
ALTER TABLE execution ADD COLUMN work_unit_version INTEGER CHECK (work_unit_version IS NULL OR work_unit_version >= 1);
CREATE INDEX idx_execution_work_unit ON execution(work_unit_id, created_at, id)
    WHERE work_unit_id IS NOT NULL;

-- A Task may enter the WorkUnit model only after all Task-scoped repository
-- authority has stopped. Once a WorkUnit exists, legacy executions cannot
-- acquire the Task integration workspace as a mutable worker workspace.
CREATE TRIGGER work_unit_create_legacy_authority_guard
BEFORE INSERT ON work_unit
WHEN EXISTS (
        SELECT 1 FROM execution e
        WHERE e.task_id = NEW.task_id AND e.status = 'running'
          AND e.work_unit_id IS NULL
    )
  OR EXISTS (
        SELECT 1 FROM workspace_lease l
        WHERE l.task_id = NEW.task_id AND l.status = 'active'
          AND l.work_unit_id IS NULL
    )
  OR EXISTS (
        SELECT 1 FROM task_terminal_session s
        JOIN workspace_scope ws ON ws.workspace_id = s.workspace_id
        WHERE s.task_id = NEW.task_id AND s.status IN ('starting', 'running')
          AND ws.scope_kind = 'integration'
    )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit creation blocked by active legacy Task workspace authority');
END;

CREATE TRIGGER execution_legacy_integration_workspace_guard_insert
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NULL AND NEW.workspace_id IS NOT NULL
 AND EXISTS (SELECT 1 FROM work_unit w WHERE w.task_id = NEW.task_id)
 AND EXISTS (
     SELECT 1 FROM workspace_scope ws
     WHERE ws.workspace_id = NEW.workspace_id AND ws.task_id = NEW.task_id
       AND ws.scope_kind = 'integration'
 )
BEGIN
    SELECT RAISE(ABORT, 'Task integration workspace is not a worker workspace for WorkUnit Tasks');
END;

CREATE TRIGGER execution_legacy_integration_workspace_guard_update
BEFORE UPDATE OF task_id, work_unit_id, workspace_id ON execution
WHEN NEW.work_unit_id IS NULL AND NEW.workspace_id IS NOT NULL
 AND EXISTS (SELECT 1 FROM work_unit w WHERE w.task_id = NEW.task_id)
 AND EXISTS (
     SELECT 1 FROM workspace_scope ws
     WHERE ws.workspace_id = NEW.workspace_id AND ws.task_id = NEW.task_id
       AND ws.scope_kind = 'integration'
 )
BEGIN
    SELECT RAISE(ABORT, 'Task integration workspace is not a worker workspace for WorkUnit Tasks');
END;

CREATE TRIGGER work_unit_allocation_running_guard
BEFORE UPDATE OF role, assigned_actor_kind, assigned_actor_id ON work_unit
WHEN EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = OLD.id AND e.status = 'running')
 AND (NEW.role IS NOT OLD.role OR NEW.assigned_actor_kind IS NOT OLD.assigned_actor_kind
      OR NEW.assigned_actor_id IS NOT OLD.assigned_actor_id)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit allocation cannot change while an Execution is running');
END;

CREATE TRIGGER work_unit_scope_running_guard
BEFORE UPDATE OF title, scope, parent_work_unit_id, requires_integration ON work_unit
WHEN EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = OLD.id AND e.status = 'running')
 AND (NEW.title IS NOT OLD.title OR NEW.scope IS NOT OLD.scope
      OR NEW.parent_work_unit_id IS NOT OLD.parent_work_unit_id
      OR NEW.requires_integration IS NOT OLD.requires_integration)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit executable scope cannot change while an Execution is running');
END;

CREATE TRIGGER work_unit_repository_mode_pinned_guard
BEFORE UPDATE OF requires_integration ON work_unit
WHEN NEW.requires_integration IS NOT OLD.requires_integration
 AND (EXISTS (
          SELECT 1 FROM workspace_scope s
          WHERE s.work_unit_id = OLD.id AND s.scope_kind = 'work_unit'
      ) OR EXISTS (
          SELECT 1 FROM execution e WHERE e.work_unit_id = OLD.id
      ))
 AND NOT EXISTS (
     SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
     WHERE t.id = OLD.task_id
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository mode is immutable after Workspace or Execution creation');
END;

CREATE TRIGGER work_unit_dependency_running_guard_insert
BEFORE INSERT ON work_unit_dependency
WHEN EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = NEW.work_unit_id AND e.status = 'running')
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit dependencies cannot change while an Execution is running');
END;

CREATE TRIGGER work_unit_dependency_running_guard_delete
BEFORE DELETE ON work_unit_dependency
WHEN EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = OLD.work_unit_id AND e.status = 'running')
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit dependencies cannot change while an Execution is running');
END;

-- A repository Workspace pins the integration HEAD used as this WorkUnit's
-- execution base. Keep its dependency set fixed after that snapshot exists.
CREATE TRIGGER work_unit_dependency_workspace_guard_insert
BEFORE INSERT ON work_unit_dependency
WHEN EXISTS (
    SELECT 1 FROM workspace_scope s
    WHERE s.work_unit_id = NEW.work_unit_id AND s.scope_kind = 'work_unit'
)
AND NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = NEW.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit dependency set is immutable after Workspace preparation');
END;

CREATE TRIGGER work_unit_dependency_workspace_guard_delete
BEFORE DELETE ON work_unit_dependency
WHEN EXISTS (
    SELECT 1 FROM workspace_scope s
    WHERE s.work_unit_id = OLD.work_unit_id AND s.scope_kind = 'work_unit'
)
AND NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit dependency set is immutable after Workspace preparation');
END;

CREATE TRIGGER work_unit_completion_running_guard
BEFORE UPDATE OF status ON work_unit
WHEN NEW.status IN ('completed', 'cancelled')
 AND EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = OLD.id AND e.status = 'running')
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit cannot become terminal while an Execution is running');
END;

CREATE TRIGGER work_unit_completion_readiness_guard
BEFORE UPDATE OF status ON work_unit
WHEN NEW.status = 'completed'
 AND (
     EXISTS (
         SELECT 1 FROM work_unit_dependency d
         JOIN work_unit prerequisite ON prerequisite.id = d.depends_on_work_unit_id
                                    AND prerequisite.task_id = d.task_id
         WHERE d.work_unit_id = OLD.id
           AND (prerequisite.status != 'completed'
                OR (prerequisite.requires_integration = 1 AND NOT EXISTS (
                    SELECT 1 FROM work_unit_integration i
                    WHERE i.work_unit_id = prerequisite.id
                      AND i.task_id = prerequisite.task_id
                      AND i.outcome = 'success'
                )))
     )
     OR (NEW.requires_integration = 1 AND NOT EXISTS (
         SELECT 1 FROM execution e
         JOIN workspace_scope s
           ON s.workspace_id = e.workspace_id AND s.task_id = e.task_id
         WHERE e.work_unit_id = OLD.id AND e.task_id = OLD.task_id
           AND e.work_unit_version = OLD.version
           AND e.status = 'completed' AND e.after_sha IS NOT NULL
           AND s.scope_kind = 'work_unit' AND s.work_unit_id = e.work_unit_id
     ))
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit completion requires satisfied dependencies and a completed result SHA');
END;

CREATE TABLE work_unit_integration (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    work_unit_id            TEXT NOT NULL,
    execution_id            TEXT NOT NULL,
    source_workspace_id     TEXT NOT NULL,
    source_branch           TEXT NOT NULL CHECK (length(trim(source_branch)) > 0),
    source_sha              TEXT NOT NULL CHECK (length(trim(source_sha)) > 0),
    target_workspace_id     TEXT NOT NULL,
    target_branch           TEXT NOT NULL CHECK (length(trim(target_branch)) > 0),
    target_before_sha       TEXT NOT NULL CHECK (length(trim(target_before_sha)) > 0),
    target_after_sha        TEXT,
    operation_idempotency_key TEXT NOT NULL CHECK (length(trim(operation_idempotency_key)) > 0),
    outcome                 TEXT NOT NULL CHECK (outcome IN ('running', 'success', 'conflict', 'failed', 'rejected')),
    conflict_metadata_json  TEXT CHECK (conflict_metadata_json IS NULL OR json_valid(conflict_metadata_json)),
    version                 INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    started_at              TEXT NOT NULL,
    finished_at             TEXT,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    UNIQUE(id, task_id),
    UNIQUE(task_id, operation_idempotency_key),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_workspace_id, task_id)
        REFERENCES workspace(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_workspace_id, task_id)
        REFERENCES workspace(id, task_id) ON DELETE RESTRICT,
    CHECK ((outcome = 'running' AND finished_at IS NULL AND target_after_sha IS NULL)
        OR (outcome != 'running' AND finished_at IS NOT NULL)),
    CHECK ((outcome = 'success' AND target_after_sha IS NOT NULL)
        OR (outcome != 'success' AND target_after_sha IS NULL))
);
CREATE UNIQUE INDEX idx_work_unit_integration_active_task
    ON work_unit_integration(task_id) WHERE outcome = 'running';
CREATE INDEX idx_work_unit_integration_work_unit
    ON work_unit_integration(work_unit_id, started_at DESC, id DESC);

CREATE TRIGGER work_unit_execution_admission_guard
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
 AND (
    NEW.status != 'running'
    OR
    NEW.work_unit_version IS NULL
    OR NEW.actor_kind IS NULL OR NEW.actor_id IS NULL
    OR NEW.purpose IS NULL
    OR (NEW.actor_kind = 'agent' AND NOT EXISTS (
        SELECT 1 FROM agent_current a
        WHERE a.id = NEW.actor_id AND NEW.agent_id = a.id
          AND (
              SELECT COUNT(*) FROM execution e
              WHERE e.agent_id = a.id AND e.status = 'running'
          ) + (
              SELECT COUNT(*) FROM agent_chat_turn_job j
              WHERE j.responder_identity_id = a.id
                AND j.status IN ('leased', 'running')
          ) < a.max_concurrent_tasks
    ))
    OR NOT EXISTS (
        SELECT 1 FROM work_unit w
        WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
          AND w.version = NEW.work_unit_version AND w.status = 'open'
          AND w.role = NEW.role
          AND ((w.requires_integration = 0 AND NEW.workspace_id IS NULL)
               OR (w.requires_integration = 1 AND NEW.status = 'running'
                   AND NEW.actor_kind = 'agent' AND NEW.agent_id = NEW.actor_id))
          AND (w.assigned_actor_id IS NULL
               OR (w.assigned_actor_kind = NEW.actor_kind AND w.assigned_actor_id = NEW.actor_id))
          AND ((w.requires_integration = 1 AND EXISTS (
              SELECT 1 FROM workspace_scope ws
              JOIN workspace bound_workspace ON bound_workspace.id = ws.workspace_id
              WHERE ws.workspace_id = NEW.workspace_id AND ws.task_id = NEW.task_id
                AND ws.scope_kind = 'work_unit' AND ws.work_unit_id = w.id
                AND bound_workspace.status = 'ready'
                AND bound_workspace.repo_id = (SELECT t.repo_id FROM task t WHERE t.id = w.task_id)
          )) OR (w.requires_integration = 0 AND NEW.workspace_id IS NULL))
          AND EXISTS (
              SELECT 1 FROM task_role tr JOIN role_membership rm ON rm.task_role_id = tr.id
              WHERE tr.task_id = w.task_id AND tr.role = w.role
                AND rm.actor_kind = NEW.actor_kind AND rm.actor_id = NEW.actor_id
                AND rm.status = 'active'
          )
          AND NOT EXISTS (
              SELECT 1 FROM execution active
              WHERE active.work_unit_id = w.id AND active.status = 'running'
          )
          AND NOT EXISTS (
              SELECT 1 FROM work_unit_dependency d
              JOIN work_unit p ON p.id = d.depends_on_work_unit_id AND p.task_id = d.task_id
              WHERE d.work_unit_id = w.id AND d.task_id = w.task_id
                AND (p.status != 'completed' OR
                     (p.requires_integration = 1 AND NOT EXISTS (
                         SELECT 1 FROM work_unit_integration i
                         WHERE i.work_unit_id = p.id AND i.task_id = p.task_id
                           AND i.outcome = 'success'
                     )))
          )
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Execution admission is not authorized, runnable, or Agent reached concurrent execution capacity');
END;

CREATE TRIGGER work_unit_execution_binding_immutable
BEFORE UPDATE OF task_id, actor_kind, actor_id, role, purpose, work_unit_id,
                 work_unit_version, workspace_id ON execution
WHEN OLD.work_unit_id IS NOT NULL AND (
    NEW.task_id IS NOT OLD.task_id OR NEW.actor_kind IS NOT OLD.actor_kind
    OR NEW.actor_id IS NOT OLD.actor_id OR NEW.role IS NOT OLD.role
    OR NEW.purpose IS NOT OLD.purpose OR NEW.work_unit_id IS NOT OLD.work_unit_id
    OR NEW.work_unit_version IS NOT OLD.work_unit_version
    OR NEW.workspace_id IS NOT OLD.workspace_id
)
AND NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Execution historical binding is immutable');
END;

CREATE TRIGGER work_unit_integration_admission_guard
BEFORE INSERT ON work_unit_integration
WHEN NEW.outcome != 'running'
  OR NOT EXISTS (
      SELECT 1 FROM work_unit w
      JOIN execution e ON e.work_unit_id = w.id AND e.task_id = w.task_id
      JOIN workspace_scope src ON src.workspace_id = NEW.source_workspace_id
                                AND src.task_id = NEW.task_id
      JOIN workspace_scope dst ON dst.workspace_id = NEW.target_workspace_id
                                AND dst.task_id = NEW.task_id
      JOIN workspace sw ON sw.id = src.workspace_id
      JOIN workspace tw ON tw.id = dst.workspace_id
      WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
        AND w.status = 'completed' AND w.requires_integration = 1
        AND e.id = NEW.execution_id AND e.status = 'completed'
        AND e.work_unit_version = w.version - 1
        AND e.workspace_id = NEW.source_workspace_id
        AND e.after_sha = NEW.source_sha
        AND src.scope_kind = 'work_unit' AND src.work_unit_id = w.id
        AND dst.scope_kind = 'integration' AND dst.work_unit_id IS NULL
        AND sw.branch = NEW.source_branch AND tw.branch = NEW.target_branch
  )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit integration source or target binding is invalid');
END;

CREATE TRIGGER work_unit_integration_binding_immutable
BEFORE UPDATE ON work_unit_integration
WHEN NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.work_unit_id IS NOT OLD.work_unit_id OR NEW.execution_id IS NOT OLD.execution_id
  OR NEW.source_workspace_id IS NOT OLD.source_workspace_id
  OR NEW.source_branch IS NOT OLD.source_branch OR NEW.source_sha IS NOT OLD.source_sha
  OR NEW.target_workspace_id IS NOT OLD.target_workspace_id
  OR NEW.target_branch IS NOT OLD.target_branch
  OR NEW.target_before_sha IS NOT OLD.target_before_sha
  OR NEW.operation_idempotency_key IS NOT OLD.operation_idempotency_key
  OR NEW.started_at IS NOT OLD.started_at OR NEW.created_at IS NOT OLD.created_at
  OR OLD.outcome != 'running'
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit integration source and terminal records are immutable');
END;

CREATE TRIGGER work_unit_integration_delete_guard
BEFORE DELETE ON work_unit_integration
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit integration records are immutable outside Project teardown');
END;

ALTER TABLE workspace_lease ADD COLUMN work_unit_id TEXT REFERENCES work_unit(id) ON DELETE RESTRICT;
ALTER TABLE workspace_lease ADD COLUMN workspace_id TEXT REFERENCES workspace(id) ON DELETE RESTRICT;
DROP INDEX IF EXISTS idx_workspace_lease_active_task;
CREATE UNIQUE INDEX idx_workspace_lease_active_legacy_task
    ON workspace_lease(task_id) WHERE status = 'active' AND work_unit_id IS NULL;
CREATE UNIQUE INDEX idx_workspace_lease_active_work_unit
    ON workspace_lease(work_unit_id) WHERE status = 'active' AND work_unit_id IS NOT NULL;

CREATE TRIGGER workspace_lease_work_unit_binding_guard
BEFORE INSERT ON workspace_lease
WHEN NEW.work_unit_id IS NOT NULL
 AND (NEW.workspace_id IS NULL OR NOT EXISTS (
      SELECT 1 FROM work_unit w
      JOIN workspace_scope ws ON ws.work_unit_id = w.id AND ws.task_id = w.task_id
      JOIN execution e ON e.work_unit_id = w.id AND e.task_id = w.task_id
      JOIN workspace bound_workspace ON bound_workspace.id = ws.workspace_id
      JOIN task t ON t.id = w.task_id
      WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
        AND ws.workspace_id = NEW.workspace_id AND ws.scope_kind = 'work_unit'
        AND e.id = NEW.execution_id AND e.workspace_id = NEW.workspace_id
        AND e.status = 'running' AND e.actor_kind = NEW.assigned_principal_type
        AND e.actor_id = NEW.assigned_principal_id AND e.role = w.role
        AND NEW.role = CASE WHEN lower(trim(w.role)) = 'reviewer' THEN 'reviewer' ELSE 'worker' END
        AND NEW.project_id = t.project_id AND NEW.repository_binding_id = t.repo_id
        AND bound_workspace.repo_id = t.repo_id
        AND NEW.base_ref = COALESCE(bound_workspace.before_sha,
            (SELECT default_branch FROM repo WHERE id = t.repo_id))
        AND NEW.status = 'active'
        AND NEW.issuing_principal_type = 'system'
        AND NEW.issuing_principal_id = 'task-service-scheduler'
        AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
  ))
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit WorkspaceLease binding is invalid');
END;

CREATE TRIGGER workspace_lease_work_unit_active_renewal_guard
BEFORE UPDATE ON workspace_lease
WHEN OLD.work_unit_id IS NOT NULL AND OLD.status = 'active' AND NEW.status = 'active'
 AND NOT EXISTS (
      SELECT 1 FROM work_unit w
      JOIN workspace_scope ws ON ws.work_unit_id = w.id AND ws.task_id = w.task_id
      JOIN workspace bound_workspace ON bound_workspace.id = ws.workspace_id
      JOIN execution e ON e.work_unit_id = w.id AND e.task_id = w.task_id
      JOIN task t ON t.id = w.task_id
      WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
        AND ws.workspace_id = NEW.workspace_id AND ws.scope_kind = 'work_unit'
        AND bound_workspace.repo_id = t.repo_id AND bound_workspace.status = 'ready'
        AND e.id = NEW.execution_id AND e.status = 'running'
        AND e.workspace_id = NEW.workspace_id AND e.actor_kind = NEW.assigned_principal_type
        AND e.actor_id = NEW.assigned_principal_id AND e.role = w.role
        AND NEW.project_id = t.project_id AND NEW.repository_binding_id = t.repo_id
        AND NEW.role = CASE WHEN lower(trim(w.role)) = 'reviewer' THEN 'reviewer' ELSE 'worker' END
        AND EXISTS (
            SELECT 1 FROM task_role tr JOIN role_membership rm ON rm.task_role_id = tr.id
            WHERE tr.task_id = w.task_id AND tr.role = w.role
              AND rm.actor_kind = e.actor_kind AND rm.actor_id = e.actor_id
              AND rm.status = 'active'
        )
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit WorkspaceLease renewal authority is stale');
END;

CREATE TRIGGER workspace_lease_scope_pair_guard
BEFORE INSERT ON workspace_lease
WHEN (NEW.work_unit_id IS NULL) != (NEW.workspace_id IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'WorkspaceLease WorkUnit and Workspace bindings must be paired');
END;

CREATE TRIGGER workspace_lease_legacy_scope_guard
BEFORE INSERT ON workspace_lease
WHEN NEW.status = 'active' AND NEW.work_unit_id IS NULL
 AND EXISTS (SELECT 1 FROM work_unit w WHERE w.task_id = NEW.task_id)
BEGIN
    SELECT RAISE(ABORT, 'Task-scoped WorkspaceLease is not valid for a WorkUnit Task');
END;

CREATE TRIGGER workspace_lease_work_unit_binding_immutable
BEFORE UPDATE OF task_id, execution_id, work_unit_id, workspace_id,
                 repository_binding_id, base_ref, role, assigned_principal_type,
                 assigned_principal_id ON workspace_lease
WHEN OLD.work_unit_id IS NOT NULL AND (
    NEW.task_id IS NOT OLD.task_id OR NEW.execution_id IS NOT OLD.execution_id
    OR NEW.work_unit_id IS NOT OLD.work_unit_id OR NEW.workspace_id IS NOT OLD.workspace_id
    OR NEW.repository_binding_id IS NOT OLD.repository_binding_id
    OR NEW.base_ref IS NOT OLD.base_ref OR NEW.role IS NOT OLD.role
    OR NEW.assigned_principal_type IS NOT OLD.assigned_principal_type
    OR NEW.assigned_principal_id IS NOT OLD.assigned_principal_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit WorkspaceLease authority binding is immutable');
END;

CREATE TABLE message_work_unit (
    message_id      TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL,
    work_unit_id    TEXT NOT NULL,
    FOREIGN KEY (message_id, task_id) REFERENCES message(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (work_unit_id, task_id) REFERENCES work_unit(id, task_id) ON DELETE RESTRICT
);
CREATE TABLE handoff_work_unit (
    handoff_id      TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL,
    work_unit_id    TEXT NOT NULL,
    FOREIGN KEY (handoff_id, task_id) REFERENCES handoff(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (work_unit_id, task_id) REFERENCES work_unit(id, task_id) ON DELETE RESTRICT
);

CREATE TRIGGER message_work_unit_immutable_update
BEFORE UPDATE ON message_work_unit
BEGIN SELECT RAISE(ABORT, 'Message WorkUnit context is immutable'); END;
CREATE TRIGGER handoff_work_unit_immutable_update
BEFORE UPDATE ON handoff_work_unit
BEGIN SELECT RAISE(ABORT, 'Handoff WorkUnit context is immutable'); END;
CREATE TRIGGER message_work_unit_delete_guard
BEFORE DELETE ON message_work_unit
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'Message WorkUnit context is immutable outside Project teardown'); END;
CREATE TRIGGER handoff_work_unit_delete_guard
BEFORE DELETE ON handoff_work_unit
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'Handoff WorkUnit context is immutable outside Project teardown'); END;

CREATE TRIGGER proposal_actor_target_guard_insert
BEFORE INSERT ON proposal
WHEN NEW.target_kind NOT IN ('task', 'execution', 'workspace', 'work_unit')
  OR (NEW.proposer_actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user u WHERE u.id = NEW.proposer_actor_id
      ))
  OR (NEW.proposer_actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.proposer_actor_id
      ))
  OR (NEW.target_kind = 'task' AND NEW.target_id != NEW.task_id)
  OR (NEW.target_kind = 'execution' AND NOT EXISTS (
          SELECT 1 FROM execution e WHERE e.id = NEW.target_id AND e.task_id = NEW.task_id
      ))
  OR (NEW.target_kind = 'workspace' AND NOT EXISTS (
          SELECT 1 FROM workspace w WHERE w.id = NEW.target_id AND w.task_id = NEW.task_id
      ))
  OR (NEW.target_kind = 'work_unit' AND NOT EXISTS (
          SELECT 1 FROM work_unit w WHERE w.id = NEW.target_id AND w.task_id = NEW.task_id
      ))
  OR (NEW.supersedes_proposal_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM proposal p
          WHERE p.id = NEW.supersedes_proposal_id
            AND p.task_id = NEW.task_id AND p.status = 'superseded'
      ))
BEGIN
    SELECT RAISE(ABORT, 'Proposal ActorRef, target, or supersedes scope is invalid');
END;

PRAGMA foreign_keys = ON;
