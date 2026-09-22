-- Plan PR1: establish the additive TaskRole/RoleMembership authority.
--
-- The old task.assignee_* and task_role_assignment records remain intact.  The
-- inserts below are a data-preserving, idempotent initial projection into the
-- new model; later PR13 owns removal of the old persistence.

CREATE TABLE task_role (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    role                TEXT NOT NULL CHECK (length(trim(role)) > 0),
    coordination_mode   TEXT CHECK (
        coordination_mode IS NULL OR
        coordination_mode IN ('partitioned', 'collaborative', 'independent')
    ),
    policy_json         TEXT NOT NULL DEFAULT '{}'
                            CHECK (json_valid(policy_json) AND json_type(policy_json) = 'object'),
    version             INTEGER NOT NULL DEFAULT 1,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    UNIQUE(task_id, role)
);

CREATE TABLE role_membership (
    id                  TEXT PRIMARY KEY,
    task_role_id        TEXT NOT NULL REFERENCES task_role(id) ON DELETE CASCADE,
    actor_kind          TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id            TEXT NOT NULL CHECK (length(trim(actor_id)) > 0),
    status              TEXT NOT NULL CHECK (status IN ('active', 'suspended', 'ended')),
    version             INTEGER NOT NULL DEFAULT 1,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    ended_at            TEXT,
    CHECK (
        (status = 'ended' AND ended_at IS NOT NULL) OR
        (status IN ('active', 'suspended') AND ended_at IS NULL)
    )
);

CREATE UNIQUE INDEX idx_role_membership_current_actor
    ON role_membership(task_role_id, actor_kind, actor_id)
    WHERE status IN ('active', 'suspended');
CREATE INDEX idx_role_membership_role_status
    ON role_membership(task_role_id, status, created_at, id);
CREATE INDEX idx_role_membership_actor_status
    ON role_membership(actor_kind, actor_id, status, created_at, id);

-- A legacy backfill may leave coordination_mode dormant while a role has at
-- most one current member.  Once a second current member is attempted, the
-- caller must choose truthful coordination semantics first.  The trigger is
-- the race-safe database backstop for repository/service checks.
CREATE TRIGGER role_membership_coordination_guard_insert
BEFORE INSERT ON role_membership
WHEN NEW.status IN ('active', 'suspended')
 AND EXISTS (
     SELECT 1
     FROM task_role tr
     JOIN role_membership current_member
       ON current_member.task_role_id = tr.id
      AND current_member.status IN ('active', 'suspended')
     WHERE tr.id = NEW.task_role_id
       AND tr.coordination_mode IS NULL
 )
BEGIN
    SELECT RAISE(ABORT, 'TaskRole coordination_mode is required for multiple current members');
END;

CREATE TRIGGER role_membership_coordination_guard_update
BEFORE UPDATE OF status ON role_membership
WHEN OLD.status NOT IN ('active', 'suspended')
 AND NEW.status IN ('active', 'suspended')
 AND EXISTS (
     SELECT 1
     FROM task_role tr
     JOIN role_membership current_member
       ON current_member.task_role_id = tr.id
      AND current_member.status IN ('active', 'suspended')
     WHERE tr.id = NEW.task_role_id
       AND current_member.id != OLD.id
       AND tr.coordination_mode IS NULL
 )
BEGIN
    SELECT RAISE(ABORT, 'TaskRole coordination_mode is required for multiple current members');
END;

CREATE TRIGGER role_membership_ended_immutable
BEFORE UPDATE OF status ON role_membership
WHEN OLD.status = 'ended' AND NEW.status != 'ended'
BEGIN
    SELECT RAISE(ABORT, 'ended role memberships are immutable');
END;

CREATE TRIGGER role_membership_ended_row_immutable
BEFORE UPDATE ON role_membership
WHEN OLD.status = 'ended'
 AND (
     NEW.id IS NOT OLD.id
     OR NEW.task_role_id IS NOT OLD.task_role_id
     OR NEW.actor_kind IS NOT OLD.actor_kind
     OR NEW.actor_id IS NOT OLD.actor_id
     OR NEW.status IS NOT OLD.status
     OR NEW.version IS NOT OLD.version
     OR NEW.created_at IS NOT OLD.created_at
     OR NEW.updated_at IS NOT OLD.updated_at
     OR NEW.ended_at IS NOT OLD.ended_at
 )
BEGIN
    SELECT RAISE(ABORT, 'ended role memberships are immutable');
END;

CREATE TRIGGER task_role_coordination_mode_guard_update
BEFORE UPDATE OF coordination_mode ON task_role
WHEN NEW.coordination_mode IS NULL
 AND EXISTS (
     SELECT 1
     FROM role_membership
     WHERE role_membership.task_role_id = task_role.id
       AND role_membership.status IN ('active', 'suspended')
     GROUP BY role_membership.task_role_id
     HAVING COUNT(*) > 1
 )
BEGIN
    SELECT RAISE(ABORT, 'TaskRole coordination_mode is required for multiple current members');
END;

-- SQLite cannot express a foreign key to either user or agent_identity from a
-- tagged reference.  These triggers make the polymorphic invariant fail closed
-- at the persistence boundary as well as in the service layer.
CREATE TRIGGER role_membership_actor_exists_insert
BEFORE INSERT ON role_membership
WHEN (NEW.actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user WHERE id = NEW.actor_id
      ))
  OR (NEW.actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity WHERE id = NEW.actor_id
      ))
BEGIN
    SELECT RAISE(ABORT, 'role_membership actor does not exist');
END;

CREATE TRIGGER role_membership_actor_exists_update
BEFORE UPDATE OF actor_kind, actor_id ON role_membership
WHEN (NEW.actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user WHERE id = NEW.actor_id
      ))
  OR (NEW.actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity WHERE id = NEW.actor_id
      ))
BEGIN
    SELECT RAISE(ABORT, 'role_membership actor does not exist');
END;

-- The identity references are polymorphic, so SQLite cannot cascade them with
-- a normal foreign key.  If an identity is physically deleted, close its
-- current memberships first; ended records remain as historical audit rows.
CREATE TRIGGER role_membership_human_delete
BEFORE DELETE ON user
WHEN EXISTS (
    SELECT 1 FROM role_membership
    WHERE actor_kind = 'human' AND actor_id = OLD.id
      AND status IN ('active', 'suspended')
)
BEGIN
    UPDATE role_membership
    SET status = 'ended',
        ended_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
        version = version + 1
    WHERE actor_kind = 'human' AND actor_id = OLD.id
      AND status IN ('active', 'suspended');
END;

CREATE TRIGGER role_membership_agent_delete
BEFORE DELETE ON agent_identity
WHEN EXISTS (
    SELECT 1 FROM role_membership
    WHERE actor_kind = 'agent' AND actor_id = OLD.id
      AND status IN ('active', 'suspended')
)
BEGIN
    UPDATE role_membership
    SET status = 'ended',
        ended_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
        version = version + 1
    WHERE actor_kind = 'agent' AND actor_id = OLD.id
      AND status IN ('active', 'suspended');
END;

-- User deletion is currently exposed through the admin path, which deletes
-- the identity directly rather than going through TaskService.  Rebuild the
-- bounded singleton projections after the membership-closing trigger has
-- ended the deleted identity's memberships.  This keeps old display/public
-- rows derived from the surviving active membership set and never makes the
-- old rows authoritative again.
CREATE TRIGGER role_membership_human_delete_projection
AFTER DELETE ON user
BEGIN
    UPDATE task_role_assignment
    SET assignee_type = (
            SELECT CASE membership.actor_kind WHEN 'agent' THEN 'agent' ELSE 'user' END
            FROM task_role AS role
            JOIN role_membership AS membership
              ON membership.task_role_id = role.id
             AND membership.status = 'active'
            WHERE role.task_id = task_role_assignment.task_id
              AND role.role = CASE lower(trim(task_role_assignment.role_name))
                  WHEN 'coder' THEN 'implementer'
                  WHEN 'worker' THEN 'implementer'
                  WHEN 'assignee' THEN 'implementer'
                  WHEN 'executor' THEN 'implementer'
                  ELSE lower(trim(task_role_assignment.role_name))
              END
            ORDER BY CASE membership.actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                     membership.created_at, membership.id
            LIMIT 1
        ),
        assignee_id = (
            SELECT membership.actor_id
            FROM task_role AS role
            JOIN role_membership AS membership
              ON membership.task_role_id = role.id
             AND membership.status = 'active'
            WHERE role.task_id = task_role_assignment.task_id
              AND role.role = CASE lower(trim(task_role_assignment.role_name))
                  WHEN 'coder' THEN 'implementer'
                  WHEN 'worker' THEN 'implementer'
                  WHEN 'assignee' THEN 'implementer'
                  WHEN 'executor' THEN 'implementer'
                  ELSE lower(trim(task_role_assignment.role_name))
              END
            ORDER BY CASE membership.actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                     membership.created_at, membership.id
            LIMIT 1
        ),
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_role_assignment.assignee_type = 'user' AND task_role_assignment.assignee_id = OLD.id;

    UPDATE task
    SET assignee_type = (
            SELECT CASE membership.actor_kind WHEN 'agent' THEN 'agent' ELSE 'user' END
            FROM task_role AS role
            JOIN role_membership AS membership
              ON membership.task_role_id = role.id
             AND membership.status = 'active'
            WHERE role.task_id = task.id
              AND role.role = 'implementer'
            ORDER BY CASE membership.actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                     membership.created_at, membership.id
            LIMIT 1
        ),
        assignee_id = (
            SELECT membership.actor_id
            FROM task_role AS role
            JOIN role_membership AS membership
              ON membership.task_role_id = role.id
             AND membership.status = 'active'
            WHERE role.task_id = task.id
              AND role.role = 'implementer'
            ORDER BY CASE membership.actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                     membership.created_at, membership.id
            LIMIT 1
        ),
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task.assignee_type = 'user' AND task.assignee_id = OLD.id;
END;

CREATE TRIGGER role_membership_agent_delete_projection
AFTER DELETE ON agent_identity
BEGIN
    UPDATE task_role_assignment
    SET assignee_type = (
            SELECT CASE membership.actor_kind WHEN 'agent' THEN 'agent' ELSE 'user' END
            FROM task_role AS role
            JOIN role_membership AS membership
              ON membership.task_role_id = role.id
             AND membership.status = 'active'
            WHERE role.task_id = task_role_assignment.task_id
              AND role.role = CASE lower(trim(task_role_assignment.role_name))
                  WHEN 'coder' THEN 'implementer'
                  WHEN 'worker' THEN 'implementer'
                  WHEN 'assignee' THEN 'implementer'
                  WHEN 'executor' THEN 'implementer'
                  ELSE lower(trim(task_role_assignment.role_name))
              END
            ORDER BY CASE membership.actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                     membership.created_at, membership.id
            LIMIT 1
        ),
        assignee_id = (
            SELECT membership.actor_id
            FROM task_role AS role
            JOIN role_membership AS membership
              ON membership.task_role_id = role.id
             AND membership.status = 'active'
            WHERE role.task_id = task_role_assignment.task_id
              AND role.role = CASE lower(trim(task_role_assignment.role_name))
                  WHEN 'coder' THEN 'implementer'
                  WHEN 'worker' THEN 'implementer'
                  WHEN 'assignee' THEN 'implementer'
                  WHEN 'executor' THEN 'implementer'
                  ELSE lower(trim(task_role_assignment.role_name))
              END
            ORDER BY CASE membership.actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                     membership.created_at, membership.id
            LIMIT 1
        ),
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task_role_assignment.assignee_type = 'agent' AND task_role_assignment.assignee_id = OLD.id;

    UPDATE task
    SET assignee_type = (
            SELECT CASE membership.actor_kind WHEN 'agent' THEN 'agent' ELSE 'user' END
            FROM task_role AS role
            JOIN role_membership AS membership
              ON membership.task_role_id = role.id
             AND membership.status = 'active'
            WHERE role.task_id = task.id
              AND role.role = 'implementer'
            ORDER BY CASE membership.actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                     membership.created_at, membership.id
            LIMIT 1
        ),
        assignee_id = (
            SELECT membership.actor_id
            FROM task_role AS role
            JOIN role_membership AS membership
              ON membership.task_role_id = role.id
             AND membership.status = 'active'
            WHERE role.task_id = task.id
              AND role.role = 'implementer'
            ORDER BY CASE membership.actor_kind WHEN 'agent' THEN 0 ELSE 1 END,
                     membership.created_at, membership.id
            LIMIT 1
        ),
        updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    WHERE task.assignee_type = 'agent' AND task.assignee_id = OLD.id;
END;

CREATE TABLE role_membership_migration_issue (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    source_key          TEXT NOT NULL UNIQUE,
    source_role         TEXT,
    actor_kind          TEXT,
    actor_id            TEXT,
    reason              TEXT NOT NULL,
    created_at          TEXT NOT NULL
);

-- Normalize the legacy workflow labels only at the new boundary.  Custom
-- workflow role names remain valid TaskRole names; execution-only labels do
-- not become roles.
WITH normalized AS (
    SELECT
        task_id,
        lower(trim(role_name)) AS legacy_role,
        CASE lower(trim(role_name))
            WHEN 'coder' THEN 'implementer'
            WHEN 'worker' THEN 'implementer'
            WHEN 'assignee' THEN 'implementer'
            WHEN 'executor' THEN 'implementer'
            WHEN 'planner' THEN 'planner'
            WHEN 'reviewer' THEN 'reviewer'
            WHEN 'orchestrator' THEN 'orchestrator'
            WHEN 'interactive' THEN NULL
            WHEN 'merge_fixer' THEN NULL
            WHEN 'system' THEN NULL
            ELSE lower(trim(role_name))
        END AS role,
        created_at,
        updated_at
    FROM task_role_assignment
)
INSERT OR IGNORE INTO task_role (
    id, task_id, role, coordination_mode, policy_json, version, created_at, updated_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    task_id,
    role,
    NULL,
    '{}',
    1,
    MIN(created_at),
    MAX(updated_at)
FROM normalized
WHERE role IS NOT NULL AND role <> ''
GROUP BY task_id, role;

-- An explicit role assignment wins over a stale task-level fallback for the
-- implementation role.  Only existing identities become memberships; invalid
-- legacy handles are reported below rather than converted into fake Actors.
WITH normalized AS (
    SELECT
        assignment.id,
        assignment.task_id,
        assignment.role_name,
        lower(trim(assignment.role_name)) AS legacy_role,
        CASE lower(trim(assignment.role_name))
            WHEN 'coder' THEN 'implementer'
            WHEN 'worker' THEN 'implementer'
            WHEN 'assignee' THEN 'implementer'
            WHEN 'executor' THEN 'implementer'
            WHEN 'planner' THEN 'planner'
            WHEN 'reviewer' THEN 'reviewer'
            WHEN 'orchestrator' THEN 'orchestrator'
            WHEN 'interactive' THEN NULL
            WHEN 'merge_fixer' THEN NULL
            WHEN 'system' THEN NULL
            ELSE lower(trim(assignment.role_name))
        END AS role,
        assignment.assignee_type,
        assignment.assignee_id,
        assignment.created_at,
        assignment.updated_at
    FROM task_role_assignment AS assignment
), ranked AS (
    SELECT normalized.*,
           ROW_NUMBER() OVER (
               PARTITION BY normalized.task_id, normalized.role
               ORDER BY normalized.id
           ) AS role_rank
    FROM normalized
), unambiguous AS (
    SELECT first_row.*
    FROM ranked AS first_row
    WHERE first_row.role_rank = 1
      AND NOT EXISTS (
          SELECT 1
          FROM normalized AS other_row
          WHERE other_row.task_id = first_row.task_id
            AND other_row.role = first_row.role
            AND (
                other_row.assignee_type IS NOT first_row.assignee_type
                OR other_row.assignee_id IS NOT first_row.assignee_id
            )
      )
)
INSERT OR IGNORE INTO role_membership (
    id, task_role_id, actor_kind, actor_id, status, version,
    created_at, updated_at, ended_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    task_role.id,
    CASE normalized.assignee_type WHEN 'user' THEN 'human' ELSE 'agent' END,
    normalized.assignee_id,
    'active',
    1,
    normalized.created_at,
    normalized.updated_at,
    NULL
FROM unambiguous AS normalized
JOIN task_role
  ON task_role.task_id = normalized.task_id
 AND task_role.role = normalized.role
WHERE normalized.role IS NOT NULL
  AND normalized.assignee_type IN ('agent', 'user')
  AND normalized.assignee_id IS NOT NULL
  AND (
      (normalized.assignee_type = 'agent' AND EXISTS (
          SELECT 1 FROM agent_identity WHERE id = normalized.assignee_id
      ))
      OR
      (normalized.assignee_type = 'user' AND EXISTS (
          SELECT 1 FROM user WHERE id = normalized.assignee_id
      ))
  );

-- Several legacy labels can normalize to one target role (`coder`, `worker`,
-- `assignee`, and `executor` all become `implementer`).  Do not silently
-- choose one actor when contradictory rows collapse to that role; the
-- unambiguous membership insert above skips the role and this audit records
-- the contradiction for review.
WITH normalized AS (
    SELECT
        assignment.id,
        assignment.task_id,
        assignment.role_name,
        CASE lower(trim(assignment.role_name))
            WHEN 'coder' THEN 'implementer'
            WHEN 'worker' THEN 'implementer'
            WHEN 'assignee' THEN 'implementer'
            WHEN 'executor' THEN 'implementer'
            WHEN 'planner' THEN 'planner'
            WHEN 'reviewer' THEN 'reviewer'
            WHEN 'orchestrator' THEN 'orchestrator'
            WHEN 'interactive' THEN NULL
            WHEN 'merge_fixer' THEN NULL
            WHEN 'system' THEN NULL
            ELSE lower(trim(assignment.role_name))
        END AS role,
        assignment.assignee_type,
        assignment.assignee_id
    FROM task_role_assignment AS assignment
), conflicts AS (
    SELECT first_row.*, second_row.id AS second_id
    FROM normalized AS first_row
    JOIN normalized AS second_row
      ON second_row.task_id = first_row.task_id
     AND second_row.role = first_row.role
     AND first_row.id < second_row.id
    WHERE first_row.role IS NOT NULL
      AND (
          first_row.assignee_type IS NOT second_row.assignee_type
          OR first_row.assignee_id IS NOT second_row.assignee_id
      )
)
INSERT OR IGNORE INTO role_membership_migration_issue (
    id, task_id, source_key, source_role, actor_kind, actor_id, reason, created_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    task_id,
    task_id || ':role-conflict:' || role || ':' || id || ':' || second_id,
    role_name,
    CASE assignee_type WHEN 'user' THEN 'human' WHEN 'agent' THEN 'agent' END,
    assignee_id,
    'multiple_legacy_assignments_collapse_to_one_role',
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM conflicts;

-- Legacy role labels which represent execution mechanics, or malformed actor
-- references, are retained as explicit migration issues for operators.
WITH normalized AS (
    SELECT
        assignment.task_id,
        assignment.role_name,
        assignment.assignee_type,
        assignment.assignee_id,
        CASE lower(trim(assignment.role_name))
            WHEN '' THEN 'role_name_empty'
            WHEN 'interactive' THEN 'execution_role_not_task_role'
            WHEN 'merge_fixer' THEN 'execution_role_not_task_role'
            WHEN 'system' THEN 'execution_role_not_task_role'
            ELSE CASE
                WHEN assignment.assignee_type IS NULL
                 AND assignment.assignee_id IS NULL THEN NULL
                WHEN assignment.assignee_type = 'agent'
                 AND assignment.assignee_id IS NOT NULL
                 AND EXISTS (
                    SELECT 1 FROM agent_identity WHERE id = assignment.assignee_id
                 ) THEN NULL
                WHEN assignment.assignee_type = 'user'
                 AND assignment.assignee_id IS NOT NULL
                 AND EXISTS (
                    SELECT 1 FROM user WHERE id = assignment.assignee_id
                 ) THEN NULL
                ELSE 'actor_reference_not_found'
            END
        END AS reason
    FROM task_role_assignment AS assignment
)
INSERT OR IGNORE INTO role_membership_migration_issue (
    id, task_id, source_key, source_role, actor_kind, actor_id, reason, created_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    task_id,
    task_id || ':role:' || role_name || ':' ||
        coalesce(assignee_type, '') || ':' || coalesce(assignee_id, ''),
    role_name,
    CASE assignee_type WHEN 'user' THEN 'human' WHEN 'agent' THEN 'agent' END,
    assignee_id,
    reason,
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM normalized
WHERE reason IS NOT NULL;

-- The task-level fallback is the legacy implementation assignment unless an
-- explicit implementation-role row exists.  This preserves PR0A precedence
-- without manufacturing a second member from contradictory data.
INSERT OR IGNORE INTO task_role (
    id, task_id, role, coordination_mode, policy_json, version, created_at, updated_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    task.id,
    'implementer',
    NULL,
    '{}',
    1,
    task.created_at,
    task.updated_at
FROM task
WHERE task.assignee_type IN ('agent', 'user')
  AND task.assignee_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM task_role_assignment AS assignment
      WHERE assignment.task_id = task.id
        AND lower(trim(assignment.role_name)) IN
            ('implementer', 'coder', 'worker', 'assignee', 'executor')
  );

-- Replace the two workspace authority predicates that previously treated the
-- singleton task_role_assignment row as eligibility.  All other lease
-- guardrails remain unchanged; a current Agent membership is necessary but
-- never sufficient without the existing execution/governance checks.
DROP TRIGGER IF EXISTS workspace_lease_scope_guard_insert;
CREATE TRIGGER workspace_lease_scope_guard_insert
BEFORE INSERT ON workspace_lease
WHEN NEW.status = 'active'
BEGIN
    SELECT CASE
        WHEN NEW.issuing_principal_type != 'system'
          OR NEW.issuing_principal_id != 'task-service-scheduler'
        THEN RAISE(ABORT, 'Workspace lease may only be issued by the scheduler')
        WHEN EXISTS (
            SELECT 1 FROM project_agent_binding
            WHERE project_id = NEW.project_id
              AND identity_id = NEW.assigned_principal_id
              AND state = 'active'
        ) OR EXISTS (
            SELECT 1 FROM account_main_agent_binding
            WHERE identity_id = NEW.assigned_principal_id
              AND state = 'active'
        ) THEN RAISE(ABORT, 'Orchestration agents cannot receive Workspace leases')
        WHEN NOT EXISTS (
            SELECT 1
            FROM task t
            JOIN project p ON p.id = t.project_id
            JOIN execution assigned_execution ON assigned_execution.id = NEW.execution_id
            WHERE t.id = NEW.task_id AND t.project_id = NEW.project_id
              AND t.version = NEW.task_version
              AND t.repo_id = NEW.repository_binding_id
              AND (
                  EXISTS (
                      SELECT 1
                      FROM task_role tr
                      JOIN role_membership rm ON rm.task_role_id = tr.id
                      WHERE tr.task_id = t.id
                        AND tr.role = CASE lower(trim(assigned_execution.role))
                            WHEN 'coder' THEN 'implementer'
                            WHEN 'worker' THEN 'implementer'
                            WHEN 'assignee' THEN 'implementer'
                            WHEN 'executor' THEN 'implementer'
                            ELSE lower(trim(assigned_execution.role))
                        END
                        AND rm.actor_kind = 'agent'
                        AND rm.actor_id = NEW.assigned_principal_id
                        AND rm.status = 'active'
                  )
                  OR (
                      NOT EXISTS (
                          SELECT 1 FROM task_role tr
                          WHERE tr.task_id = t.id
                            AND tr.role = CASE lower(trim(assigned_execution.role))
                                WHEN 'coder' THEN 'implementer'
                                WHEN 'worker' THEN 'implementer'
                                WHEN 'assignee' THEN 'implementer'
                                WHEN 'executor' THEN 'implementer'
                                ELSE lower(trim(assigned_execution.role))
                            END
                      )
                      AND t.assignee_type = NEW.assigned_principal_type
                      AND t.assignee_id = NEW.assigned_principal_id
                  )
                  OR (
                      NOT EXISTS (
                          SELECT 1 FROM task_role tr
                          WHERE tr.task_id = t.id
                            AND tr.role = CASE lower(trim(assigned_execution.role))
                                WHEN 'coder' THEN 'implementer'
                                WHEN 'worker' THEN 'implementer'
                                WHEN 'assignee' THEN 'implementer'
                                WHEN 'executor' THEN 'implementer'
                                ELSE lower(trim(assigned_execution.role))
                            END
                      )
                      AND EXISTS (
                          SELECT 1 FROM task_role_assignment ra
                          WHERE ra.task_id = t.id
                            AND ra.role_name = assigned_execution.role
                            AND ra.assignee_type = 'agent'
                            AND ra.assignee_id = NEW.assigned_principal_id
                      )
                  )
                  OR (
                      NOT EXISTS (
                          SELECT 1 FROM task_role tr
                          WHERE tr.task_id = t.id
                            AND tr.role = CASE lower(trim(assigned_execution.role))
                                WHEN 'coder' THEN 'implementer'
                                WHEN 'worker' THEN 'implementer'
                                WHEN 'assignee' THEN 'implementer'
                                WHEN 'executor' THEN 'implementer'
                                ELSE lower(trim(assigned_execution.role))
                            END
                      )
                      AND p.charter_status != 'charter_backed'
                      AND p.charter_setup_required != 0
                      AND t.assignee_type IS NULL
                      AND t.assignee_id IS NULL
                  )
              )
        ) THEN RAISE(ABORT, 'Workspace lease Task is cross-Project, stale, or not membership-authorized')
        WHEN NOT EXISTS (
            SELECT 1 FROM execution e
            WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
              AND e.status = 'running'
              AND e.agent_id = NEW.assigned_principal_id
              AND ((NEW.role = 'reviewer' AND e.role = 'reviewer')
                   OR (NEW.role = 'worker'
                       AND length(trim(e.role)) > 0
                       AND e.role != 'reviewer'))
        ) THEN RAISE(ABORT, 'Workspace lease execution is not Task-scoped')
        WHEN NOT EXISTS (
            SELECT 1
            FROM project p
            LEFT JOIN project_task_governance g
              ON g.task_id = NEW.task_id AND g.project_id = p.id
            LEFT JOIN project_execution_baseline b
              ON b.id = g.baseline_id AND b.project_id = g.project_id
            LEFT JOIN project_execution_baseline_revision r
              ON r.id = g.baseline_revision_id AND r.baseline_id = b.id
            LEFT JOIN project_execution_baseline_approval a
              ON a.baseline_id = b.id AND a.revision_id = r.id
            WHERE p.id = NEW.project_id
              AND json_array_length(NEW.capabilities_json) = 1
              AND json_extract(NEW.capabilities_json, '$[0]') =
                  COALESCE(g.capability_class,
                    CASE WHEN (SELECT task_type FROM task WHERE id = NEW.task_id)
                              IN ('planning', 'discovery', 'review', 'validation')
                         THEN 'repository_read' ELSE 'repository_write' END)
              AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
              AND NEW.capability_profile_digest = CASE json_extract(NEW.capabilities_json, '$[0]')
                  WHEN 'repository_read' THEN 'sha256:6035ec533a0bdb74c461ea9ea2d7147a2e47ba7c8b54c8b732052ceec23e8234'
                  WHEN 'repository_write' THEN 'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8'
                  WHEN 'read_only' THEN 'sha256:08fe2de40d5f9027b803131fcbe5ab3c885c044836d6e20c2e9319951d2e82f3'
                  WHEN 'discovery_read' THEN 'sha256:54502cd9c50b5f43a79e75cd1abdedf5e354393ef1422e6c4932c5716c660c43'
                  WHEN 'planning_read' THEN 'sha256:78316b764f1326273f129407de72a33bbcf8db210d3bdfe7154fa1384a7d366d'
                  ELSE '' END
              AND (
                  p.charter_status != 'charter_backed'
                  OR p.charter_setup_required != 0
                  OR (g.runnable = 1
                      AND b.lifecycle = 'active'
                      AND b.current_revision_id = r.id
                      AND r.lifecycle = 'approved'
                      AND r.charter_revision_id = p.current_charter_revision_id
                      AND g.charter_revision_id = p.current_charter_revision_id
                      AND a.principal_type = 'user'
                      AND a.authorization_action = 'project.execution_baseline.approve'
                      AND length(trim(a.authorization_basis)) > 0
                      AND length(trim(a.authorization_occurred_at)) > 0
                      AND length(trim(a.explicit_event)) > 0
                      AND a.lifecycle IN ('active', 'consumed')
                      AND a.content_digest = r.content_digest
                      AND a.rendered_digest = r.rendered_digest)
                  OR (g.runnable = 0
                      AND g.baseline_id IS NULL
                      AND g.baseline_revision_id IS NULL
                      AND g.charter_revision_id = p.current_charter_revision_id
                      AND (SELECT task_type FROM task WHERE id = NEW.task_id)
                          IN ('planning', 'discovery', 'review', 'validation')
                      AND g.capability_class IN
                          ('repository_read', 'read_only', 'discovery_read', 'planning_read'))
              )
        ) THEN RAISE(ABORT, 'Workspace lease requires a runnable user-approved baseline Task')
    END;
END;

DROP TRIGGER IF EXISTS workspace_lease_active_renewal_guard;
CREATE TRIGGER workspace_lease_active_renewal_guard
BEFORE UPDATE ON workspace_lease
WHEN OLD.status = 'active' AND NEW.status = 'active'
BEGIN
    SELECT CASE
        WHEN NEW.expires_at <= OLD.expires_at OR NEW.updated_at IS OLD.updated_at
        THEN RAISE(ABORT, 'Workspace lease renewal must extend expiry')
        WHEN EXISTS (
            SELECT 1 FROM project_agent_binding
            WHERE project_id = NEW.project_id AND identity_id = NEW.assigned_principal_id AND state = 'active'
        ) OR EXISTS (
            SELECT 1 FROM account_main_agent_binding
            WHERE identity_id = NEW.assigned_principal_id AND state = 'active'
        ) THEN RAISE(ABORT, 'Orchestration agents cannot receive Workspace leases')
        WHEN NOT EXISTS (
            SELECT 1
            FROM task t
            JOIN project p ON p.id = t.project_id
            JOIN execution e ON e.id = NEW.execution_id
            LEFT JOIN project_task_governance g ON g.task_id = t.id AND g.project_id = p.id
            LEFT JOIN project_execution_baseline b ON b.id = g.baseline_id AND b.project_id = p.id
            LEFT JOIN project_execution_baseline_revision r ON r.id = g.baseline_revision_id AND r.baseline_id = b.id
            WHERE t.id = NEW.task_id AND t.project_id = NEW.project_id
              AND t.repo_id = NEW.repository_binding_id
              AND e.task_id = t.id AND e.status = 'running' AND e.agent_id = NEW.assigned_principal_id
              AND ((NEW.role = 'reviewer' AND e.role = 'reviewer') OR (NEW.role = 'worker' AND e.role != 'reviewer'))
              AND (
                  EXISTS (
                      SELECT 1
                      FROM task_role tr
                      JOIN role_membership rm ON rm.task_role_id = tr.id
                      WHERE tr.task_id = t.id
                        AND tr.role = CASE lower(trim(e.role))
                            WHEN 'coder' THEN 'implementer'
                            WHEN 'worker' THEN 'implementer'
                            WHEN 'assignee' THEN 'implementer'
                            WHEN 'executor' THEN 'implementer'
                            ELSE lower(trim(e.role))
                        END
                        AND rm.actor_kind = 'agent'
                        AND rm.actor_id = NEW.assigned_principal_id
                        AND rm.status = 'active'
                  )
                  OR (
                      NOT EXISTS (
                          SELECT 1 FROM task_role tr
                          WHERE tr.task_id = t.id
                            AND tr.role = CASE lower(trim(e.role))
                                WHEN 'coder' THEN 'implementer'
                                WHEN 'worker' THEN 'implementer'
                                WHEN 'assignee' THEN 'implementer'
                                WHEN 'executor' THEN 'implementer'
                                ELSE lower(trim(e.role))
                            END
                      )
                      AND (
                          (t.assignee_type = 'agent' AND t.assignee_id = NEW.assigned_principal_id)
                          OR EXISTS (
                              SELECT 1 FROM task_role_assignment ra
                              WHERE ra.task_id = t.id
                                AND ra.role_name = e.role
                                AND ra.assignee_type = 'agent'
                                AND ra.assignee_id = NEW.assigned_principal_id
                          )
                          OR (
                              p.charter_status != 'charter_backed'
                              AND p.charter_setup_required != 0
                              AND t.assignee_type IS NULL
                              AND t.assignee_id IS NULL
                          )
                      )
                  )
              )
              AND json_extract(NEW.capabilities_json, '$[0]') = COALESCE(
                  g.capability_class,
                  CASE WHEN t.task_type IN ('planning', 'discovery', 'review', 'validation')
                       THEN 'repository_read' ELSE 'repository_write' END)
              AND (
                  p.charter_status != 'charter_backed' OR p.charter_setup_required != 0
                  OR (g.runnable = 1 AND b.lifecycle = 'active' AND b.current_revision_id = r.id
                      AND r.lifecycle = 'approved' AND r.charter_revision_id = p.current_charter_revision_id
                      AND EXISTS (SELECT 1 FROM project_execution_baseline_approval a
                                  WHERE a.baseline_id = b.id AND a.revision_id = r.id
                                    AND a.lifecycle IN ('active', 'consumed')
                                    AND a.content_digest = r.content_digest AND a.rendered_digest = r.rendered_digest))
                  OR (g.runnable = 0 AND g.baseline_id IS NULL AND g.baseline_revision_id IS NULL
                      AND g.charter_revision_id = p.current_charter_revision_id
                      AND t.task_type IN ('planning', 'discovery', 'review', 'validation')
                      AND g.capability_class IN ('repository_read', 'read_only', 'discovery_read', 'planning_read'))
              )
        ) THEN RAISE(ABORT, 'Workspace lease renewal authority is stale')
    END;
END;

INSERT OR IGNORE INTO role_membership (
    id, task_role_id, actor_kind, actor_id, status, version,
    created_at, updated_at, ended_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    task_role.id,
    CASE task.assignee_type WHEN 'user' THEN 'human' ELSE 'agent' END,
    task.assignee_id,
    'active',
    1,
    task.created_at,
    task.updated_at,
    NULL
FROM task
JOIN task_role ON task_role.task_id = task.id AND task_role.role = 'implementer'
WHERE task.assignee_type IN ('agent', 'user')
  AND task.assignee_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM task_role_assignment AS assignment
      WHERE assignment.task_id = task.id
        AND lower(trim(assignment.role_name)) IN
            ('implementer', 'coder', 'worker', 'assignee', 'executor')
  )
  AND (
      (task.assignee_type = 'agent' AND EXISTS (
          SELECT 1 FROM agent_identity WHERE id = task.assignee_id
      ))
      OR
      (task.assignee_type = 'user' AND EXISTS (
          SELECT 1 FROM user WHERE id = task.assignee_id
      ))
  );

INSERT OR IGNORE INTO role_membership_migration_issue (
    id, task_id, source_key, source_role, actor_kind, actor_id, reason, created_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    task.id,
    task.id || ':fallback:' || coalesce(task.assignee_type, '') || ':' || coalesce(task.assignee_id, ''),
    'task_fallback',
    CASE task.assignee_type WHEN 'user' THEN 'human' WHEN 'agent' THEN 'agent' END,
    task.assignee_id,
    CASE
        WHEN EXISTS (
            SELECT 1 FROM task_role_assignment AS assignment
            WHERE assignment.task_id = task.id
              AND lower(trim(assignment.role_name)) IN
                  ('implementer', 'coder', 'worker', 'assignee', 'executor')
        ) THEN 'explicit_role_assignment_takes_precedence'
        ELSE 'actor_reference_not_found'
    END,
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM task
WHERE task.assignee_type IN ('agent', 'user')
  AND task.assignee_id IS NOT NULL
  AND (
      EXISTS (
          SELECT 1 FROM task_role_assignment AS assignment
          WHERE assignment.task_id = task.id
            AND lower(trim(assignment.role_name)) IN
                ('implementer', 'coder', 'worker', 'assignee', 'executor')
      )
      OR NOT (
          (task.assignee_type = 'agent' AND EXISTS (
              SELECT 1 FROM agent_identity WHERE id = task.assignee_id
          ))
          OR
          (task.assignee_type = 'user' AND EXISTS (
              SELECT 1 FROM user WHERE id = task.assignee_id
          ))
      )
  );
