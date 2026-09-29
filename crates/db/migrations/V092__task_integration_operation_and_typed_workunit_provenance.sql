-- PR5 review hardening: atomically serialize every exclusive operation that
-- reads or mutates a Task integration workspace, and retain typed ActorRef
-- provenance without rewriting ambiguous historical claims.

ALTER TABLE work_unit
    ADD COLUMN provenance_actor_kind TEXT
        CHECK (provenance_actor_kind IN ('human', 'agent') OR provenance_actor_kind IS NULL);

-- Existing V091 Actor provenance stored only an ID. Backfill the type only
-- when the identity is unambiguous; unresolved historical claims remain
-- explicitly distinguishable from newly validated ActorRef provenance.
UPDATE work_unit
SET provenance_actor_kind = 'human'
WHERE provenance_kind = 'actor'
  AND EXISTS (SELECT 1 FROM user u WHERE u.id = work_unit.provenance_id)
  AND NOT EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = work_unit.provenance_id);

UPDATE work_unit
SET provenance_actor_kind = 'agent'
WHERE provenance_kind = 'actor'
  AND EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = work_unit.provenance_id)
  AND NOT EXISTS (SELECT 1 FROM user u WHERE u.id = work_unit.provenance_id);

CREATE TRIGGER work_unit_provenance_guard_insert
BEFORE INSERT ON work_unit
WHEN (NEW.provenance_kind = 'actor' AND (
          NEW.provenance_actor_kind IS NULL
          OR (NEW.provenance_actor_kind = 'human' AND NOT EXISTS (
              SELECT 1 FROM user u WHERE u.id = NEW.provenance_id
          ))
          OR (NEW.provenance_actor_kind = 'agent' AND NOT EXISTS (
              SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.provenance_id
          ))
      ))
  OR (NEW.provenance_kind IS NOT 'actor' AND NEW.provenance_actor_kind IS NOT NULL)
  OR (NEW.provenance_kind = 'work_unit' AND NOT EXISTS (
      SELECT 1 FROM work_unit source
      WHERE source.id = NEW.provenance_id AND source.task_id = NEW.task_id
  ))
  OR (NEW.provenance_kind = 'artifact' AND NOT EXISTS (
      SELECT 1 FROM artifact source
      WHERE source.id = NEW.provenance_id AND source.task_id = NEW.task_id
  ))
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit provenance is invalid or outside its Task');
END;

CREATE TRIGGER work_unit_provenance_guard_update
BEFORE UPDATE OF provenance_kind, provenance_id, provenance_actor_kind ON work_unit
WHEN NEW.provenance_kind IS NOT OLD.provenance_kind
  OR NEW.provenance_id IS NOT OLD.provenance_id
  OR NEW.provenance_actor_kind IS NOT OLD.provenance_actor_kind
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit provenance is immutable');
END;

CREATE TABLE task_integration_operation (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    kind            TEXT NOT NULL CHECK (kind IN (
                        'work_unit_integration', 'task_merge', 'publish_pr',
                        'work_unit_create', 'work_unit_workspace_prepare',
                        'integration_workspace_cleanup'
                    )),
    owner_id        TEXT NOT NULL CHECK (length(trim(owner_id)) > 0),
    status          TEXT NOT NULL CHECK (status IN (
                        'running', 'succeeded', 'conflict', 'failed', 'abandoned'
                    )),
    version         INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    finished_at     TEXT,
    UNIQUE(id, task_id),
    CHECK ((status = 'running' AND finished_at IS NULL)
        OR (status != 'running' AND finished_at IS NOT NULL))
);

-- The database row is the atomic durable claim. The service also holds an OS
-- file lock for the claim lifetime so a later process can distinguish a live
-- owner from a crashed operation before abandoning the durable row.
CREATE UNIQUE INDEX idx_task_integration_operation_active_task
    ON task_integration_operation(task_id) WHERE status = 'running';
CREATE INDEX idx_task_integration_operation_task_created
    ON task_integration_operation(task_id, created_at, id);

CREATE TRIGGER task_integration_operation_terminal_guard_insert
BEFORE INSERT ON task_integration_operation
WHEN NEW.status = 'running' AND EXISTS (
    SELECT 1
    FROM task_terminal_session s
    JOIN workspace_scope ws
      ON ws.workspace_id = s.workspace_id AND ws.task_id = s.task_id
    WHERE s.task_id = NEW.task_id
      AND s.status IN ('starting', 'running')
      AND ws.scope_kind = 'integration'
)
BEGIN
    SELECT RAISE(ABORT, 'Task integration workspace has an active terminal session');
END;

CREATE TRIGGER task_integration_operation_binding_immutable
BEFORE UPDATE ON task_integration_operation
WHEN NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.kind IS NOT OLD.kind OR NEW.owner_id IS NOT OLD.owner_id
  OR NEW.created_at IS NOT OLD.created_at
  OR OLD.status != 'running'
  OR NEW.status NOT IN ('succeeded', 'conflict', 'failed', 'abandoned')
  OR NEW.version != OLD.version + 1
BEGIN
    SELECT RAISE(ABORT, 'Task integration operation identity or terminal result is immutable');
END;

CREATE TRIGGER task_integration_operation_delete_guard
BEFORE DELETE ON task_integration_operation
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task integration operation history is removed only during guarded Project teardown');
END;

-- Terminal start and integration operations use the same SQLite write
-- serialization boundary, so neither can begin after the other commits.
CREATE TRIGGER task_terminal_integration_operation_guard_insert
BEFORE INSERT ON task_terminal_session
WHEN NEW.status IN ('starting', 'running') AND EXISTS (
    SELECT 1 FROM workspace_scope ws
    JOIN task_integration_operation op ON op.task_id = ws.task_id
    WHERE ws.workspace_id = NEW.workspace_id AND ws.task_id = NEW.task_id
      AND ws.scope_kind = 'integration' AND op.status = 'running'
)
BEGIN
    SELECT RAISE(ABORT, 'Task integration workspace has an active exclusive operation');
END;

CREATE TRIGGER task_terminal_integration_operation_guard_update
BEFORE UPDATE OF status, workspace_id, task_id ON task_terminal_session
WHEN NEW.status IN ('starting', 'running') AND EXISTS (
    SELECT 1 FROM workspace_scope ws
    JOIN task_integration_operation op ON op.task_id = ws.task_id
    WHERE ws.workspace_id = NEW.workspace_id AND ws.task_id = NEW.task_id
      AND ws.scope_kind = 'integration' AND op.status = 'running'
)
BEGIN
    SELECT RAISE(ABORT, 'Task integration workspace has an active exclusive operation');
END;
