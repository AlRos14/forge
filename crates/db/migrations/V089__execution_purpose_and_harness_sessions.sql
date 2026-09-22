-- Plan PR2 is additive.  The legacy execution.agent_id and
-- execution.agent_session_id columns remain compatibility projections until
-- the final cleanup plan.  V062 agent_session is deliberately not renamed or
-- reused: it belongs to the embedded Agent Runtime/Agent Host vertical.

CREATE TABLE harness_session (
    id                         TEXT PRIMARY KEY,
    agent_id                   TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE RESTRICT,
    harness_kind               TEXT NOT NULL CHECK (length(trim(harness_kind)) > 0),
    external_session_id        TEXT,
    profile_id                 TEXT REFERENCES agent_profile(id) ON DELETE SET NULL,
    profile_snapshot_json      TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(profile_snapshot_json)),
    capabilities_snapshot_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(capabilities_snapshot_json)),
    -- Workspace rows are replaceable operational state. Keep the historical
    -- scope token without an FK so deleting/resetting a workspace cannot
    -- silently widen a session into unscoped continuity.
    workspace_id               TEXT,
    status                     TEXT NOT NULL DEFAULT 'pending'
                                 CHECK (status IN ('pending', 'active', 'ended', 'failed'))
                                 CHECK (external_session_id IS NULL
                                        OR length(trim(external_session_id)) > 0)
                                 CHECK (
                                     (status = 'pending' AND external_session_id IS NULL)
                                     OR (status = 'active'
                                         AND external_session_id IS NOT NULL
                                         AND length(trim(external_session_id)) > 0)
                                     OR status IN ('ended', 'failed')
                                 ),
    predecessor_session_id     TEXT REFERENCES harness_session(id) ON DELETE RESTRICT,
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL,
    last_activity_at           TEXT
);

CREATE INDEX idx_harness_session_agent_status
    ON harness_session(agent_id, status, updated_at DESC, id DESC);
CREATE INDEX idx_harness_session_workspace
    ON harness_session(workspace_id, updated_at DESC, id DESC)
    WHERE workspace_id IS NOT NULL;
CREATE UNIQUE INDEX idx_harness_session_external_identity
    ON harness_session(agent_id, harness_kind, external_session_id)
    WHERE external_session_id IS NOT NULL;

CREATE TABLE execution_session_migration_issue (
    id             TEXT PRIMARY KEY,
    execution_id   TEXT NOT NULL REFERENCES execution(id) ON DELETE CASCADE,
    issue_kind     TEXT NOT NULL,
    details_json   TEXT NOT NULL DEFAULT '{}',
    created_at     TEXT NOT NULL
);
CREATE INDEX idx_execution_session_migration_issue_execution
    ON execution_session_migration_issue(execution_id, created_at ASC, id ASC);

ALTER TABLE execution ADD COLUMN actor_kind TEXT
    CHECK (actor_kind IS NULL OR actor_kind IN ('human', 'agent'));
ALTER TABLE execution ADD COLUMN actor_id TEXT;
ALTER TABLE execution ADD COLUMN purpose TEXT
    CHECK (purpose IS NULL OR purpose IN (
        'plan', 'implement', 'review', 'validate', 'investigate', 'orchestrate', 'general'
    ));
ALTER TABLE execution ADD COLUMN harness_session_id TEXT
    REFERENCES harness_session(id) ON DELETE RESTRICT;

CREATE INDEX idx_execution_actor
    ON execution(actor_kind, actor_id, created_at ASC, id ASC);
CREATE INDEX idx_execution_purpose
    ON execution(purpose, created_at ASC, id ASC);
CREATE INDEX idx_execution_harness_session
    ON execution(harness_session_id, created_at ASC, id ASC)
    WHERE harness_session_id IS NOT NULL;

-- Agent history is the only exact principal available for old executions with
-- agent_id.  An agentless row is intentionally not guessed from current task
-- assignment or RoleMembership.
UPDATE execution
SET actor_kind = 'agent',
    actor_id = agent_id
WHERE agent_id IS NOT NULL
  AND lower(trim(agent_id)) <> 'human';

INSERT INTO execution_session_migration_issue (id, execution_id, issue_kind, details_json, created_at)
SELECT
    lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        lower(hex(randomblob(6))),
    id,
    'historical_actor_unresolved',
    '{"reason":"legacy execution has no exact persisted Actor identity (NULL or reserved human sentinel); current assignment was not consulted"}',
    updated_at
FROM execution
WHERE agent_id IS NULL
   OR lower(trim(agent_id)) = 'human';

-- Historical purpose is a deterministic role-only mapping.  It is deliberately
-- not reused by new runtime writers, which must supply the semantic purpose.
UPDATE execution
SET purpose = CASE lower(trim(role))
    WHEN 'planner' THEN 'plan'
    WHEN 'reviewer' THEN 'review'
    WHEN 'auditor' THEN 'review'
    WHEN 'coder' THEN 'implement'
    WHEN 'worker' THEN 'implement'
    WHEN 'implementer' THEN 'implement'
    WHEN 'executor' THEN 'implement'
    WHEN 'merge_fixer' THEN 'implement'
    WHEN 'orchestrator' THEN 'orchestrate'
    ELSE 'general'
END;

-- A single external thread is safely materialized only when its historical
-- agent, harness identity, profile, and workspace evidence are coherent.
-- The executor snapshot is retained as opaque profile/config evidence; it is
-- not replaced by current Agent configuration.
WITH session_rows AS (
    SELECT
        e.id,
        e.agent_id,
        e.agent_session_id AS external_session_id,
        CASE
            WHEN json_valid(e.executor_config_snapshot_json)
            THEN COALESCE(NULLIF(json_extract(e.executor_config_snapshot_json, '$.executor_type'), ''), 'legacy')
            ELSE 'legacy'
        END AS harness_kind,
        CASE
            WHEN json_valid(e.executor_config_snapshot_json) THEN (
                SELECT ap.id
                FROM agent_profile AS ap
                WHERE ap.id = NULLIF(json_extract(e.executor_config_snapshot_json, '$.profile_id'), '')
                  AND ap.identity_id = e.agent_id
                LIMIT 1
            )
            ELSE NULL
        END AS profile_id,
        CASE
            WHEN json_valid(e.executor_config_snapshot_json)
             AND NULLIF(json_extract(e.executor_config_snapshot_json, '$.profile_id'), '') IS NOT NULL
             AND NOT EXISTS (
                 SELECT 1
                 FROM agent_profile AS ap
                 WHERE ap.id = NULLIF(json_extract(e.executor_config_snapshot_json, '$.profile_id'), '')
                   AND ap.identity_id = e.agent_id
             )
            THEN 1
            ELSE 0
        END AS profile_invalid,
        e.executor_config_snapshot_json,
        e.workspace_id,
        e.created_at,
        e.updated_at
    FROM execution AS e
    WHERE e.agent_id IS NOT NULL
      AND lower(trim(e.agent_id)) <> 'human'
      AND NULLIF(trim(e.agent_session_id), '') IS NOT NULL
), grouped AS (
    SELECT
        agent_id,
        harness_kind,
        external_session_id,
        COUNT(DISTINCT COALESCE(profile_id, '')) AS profile_count,
        MAX(profile_invalid) AS profile_invalid,
        COUNT(DISTINCT COALESCE(workspace_id, '')) AS workspace_count
    FROM session_rows
    GROUP BY agent_id, harness_kind, external_session_id
), coherent AS (
    SELECT *
    FROM grouped
    WHERE profile_count <= 1
      AND profile_invalid = 0
      AND workspace_count <= 1
)
INSERT INTO harness_session (
    id,
    agent_id,
    harness_kind,
    external_session_id,
    profile_id,
    profile_snapshot_json,
    capabilities_snapshot_json,
    workspace_id,
    status,
    predecessor_session_id,
    created_at,
    updated_at,
    last_activity_at
)
SELECT
    lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        lower(hex(randomblob(6))),
    coherent.agent_id,
    coherent.harness_kind,
    coherent.external_session_id,
    first.profile_id,
    CASE
        WHEN json_valid(first.executor_config_snapshot_json)
        THEN first.executor_config_snapshot_json
        ELSE '{}'
    END,
    CASE
        WHEN json_valid(first.executor_config_snapshot_json)
         AND json_valid(json_extract(first.executor_config_snapshot_json, '$.capabilities'))
        THEN json_extract(first.executor_config_snapshot_json, '$.capabilities')
        ELSE '{}'
    END,
    CASE WHEN coherent.workspace_count = 1 THEN first.workspace_id ELSE NULL END,
    'active',
    NULL,
    first.created_at,
    first.updated_at,
    first.updated_at
FROM coherent
JOIN session_rows AS first
  ON first.agent_id = coherent.agent_id
 AND first.harness_kind = coherent.harness_kind
 AND first.external_session_id = coherent.external_session_id
WHERE NOT EXISTS (
    SELECT 1
    FROM session_rows AS earlier
    WHERE earlier.agent_id = first.agent_id
      AND earlier.harness_kind = first.harness_kind
      AND earlier.external_session_id = first.external_session_id
      AND (earlier.created_at < first.created_at
           OR (earlier.created_at = first.created_at AND earlier.id < first.id))
);

-- Bind only coherent historical rows.  Contradictory profile/workspace groups
-- remain on bounded legacy compatibility and are recorded below.
UPDATE execution
SET harness_session_id = (
    SELECT hs.id
    FROM harness_session AS hs
    WHERE hs.agent_id = execution.agent_id
      AND hs.harness_kind = CASE
          WHEN json_valid(execution.executor_config_snapshot_json)
          THEN COALESCE(NULLIF(json_extract(execution.executor_config_snapshot_json, '$.executor_type'), ''), 'legacy')
          ELSE 'legacy'
      END
      AND hs.external_session_id = execution.agent_session_id
)
WHERE execution.agent_id IS NOT NULL
  AND lower(trim(execution.agent_id)) <> 'human'
  AND NULLIF(trim(execution.agent_session_id), '') IS NOT NULL;

WITH session_rows AS (
    SELECT
        e.id,
        e.agent_id,
        e.agent_session_id AS external_session_id,
        CASE
            WHEN json_valid(e.executor_config_snapshot_json)
            THEN COALESCE(NULLIF(json_extract(e.executor_config_snapshot_json, '$.executor_type'), ''), 'legacy')
            ELSE 'legacy'
        END AS harness_kind,
        CASE
            WHEN json_valid(e.executor_config_snapshot_json) THEN (
                SELECT ap.id
                FROM agent_profile AS ap
                WHERE ap.id = NULLIF(json_extract(e.executor_config_snapshot_json, '$.profile_id'), '')
                  AND ap.identity_id = e.agent_id
                LIMIT 1
            )
            ELSE NULL
        END AS profile_id,
        CASE
            WHEN json_valid(e.executor_config_snapshot_json)
             AND NULLIF(json_extract(e.executor_config_snapshot_json, '$.profile_id'), '') IS NOT NULL
             AND NOT EXISTS (
                 SELECT 1
                 FROM agent_profile AS ap
                 WHERE ap.id = NULLIF(json_extract(e.executor_config_snapshot_json, '$.profile_id'), '')
                   AND ap.identity_id = e.agent_id
             )
            THEN 1
            ELSE 0
        END AS profile_invalid,
        e.workspace_id
    FROM execution AS e
    WHERE e.agent_id IS NOT NULL
      AND lower(trim(e.agent_id)) <> 'human'
      AND NULLIF(trim(e.agent_session_id), '') IS NOT NULL
), ambiguous AS (
    SELECT agent_id, harness_kind, external_session_id
    FROM session_rows
    GROUP BY agent_id, harness_kind, external_session_id
    HAVING COUNT(DISTINCT COALESCE(profile_id, '')) > 1
        OR MAX(profile_invalid) > 0
        OR COUNT(DISTINCT COALESCE(workspace_id, '')) > 1
)
INSERT INTO execution_session_migration_issue (id, execution_id, issue_kind, details_json, created_at)
SELECT
    lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        lower(hex(randomblob(6))),
    rows.id,
    'historical_session_ambiguous',
    json_object(
        'agent_id', rows.agent_id,
        'harness_kind', rows.harness_kind,
        'external_session_id', rows.external_session_id,
        'reason', 'historical profile or workspace evidence conflicts; no generic session was selected'
    ),
    (SELECT updated_at FROM execution WHERE id = rows.id)
FROM session_rows AS rows
JOIN ambiguous
  ON ambiguous.agent_id = rows.agent_id
 AND ambiguous.harness_kind = rows.harness_kind
 AND ambiguous.external_session_id = rows.external_session_id;

CREATE TRIGGER execution_actor_ref_guard_insert
BEFORE INSERT ON execution
WHEN (NEW.actor_kind IS NULL AND NEW.actor_id IS NOT NULL)
  OR (NEW.actor_kind IS NOT NULL AND NEW.actor_id IS NULL)
  OR lower(trim(NEW.actor_id)) = 'human'
  OR (NEW.actor_kind = 'human' AND NOT EXISTS (SELECT 1 FROM user WHERE id = NEW.actor_id))
  OR (NEW.actor_kind = 'agent' AND NOT EXISTS (SELECT 1 FROM agent_identity WHERE id = NEW.actor_id))
BEGIN
    SELECT RAISE(ABORT, 'execution actor_ref is invalid');
END;

CREATE TRIGGER execution_actor_ref_guard_update
BEFORE UPDATE OF actor_kind, actor_id ON execution
WHEN (NEW.actor_kind IS NULL AND NEW.actor_id IS NOT NULL)
  OR (NEW.actor_kind IS NOT NULL AND NEW.actor_id IS NULL)
  OR lower(trim(NEW.actor_id)) = 'human'
  OR (NEW.actor_kind = 'human' AND NOT EXISTS (SELECT 1 FROM user WHERE id = NEW.actor_id))
  OR (NEW.actor_kind = 'agent' AND NOT EXISTS (SELECT 1 FROM agent_identity WHERE id = NEW.actor_id))
BEGIN
    SELECT RAISE(ABORT, 'execution actor_ref is invalid');
END;

CREATE TRIGGER execution_agent_projection_guard_insert
BEFORE INSERT ON execution
WHEN (NEW.actor_kind = 'human' AND NEW.agent_id IS NOT NULL)
  OR (NEW.actor_kind = 'agent' AND (NEW.agent_id IS NULL OR NEW.agent_id != NEW.actor_id))
BEGIN
    SELECT RAISE(ABORT, 'execution agent projection is invalid');
END;

CREATE TRIGGER execution_agent_projection_guard_update
BEFORE UPDATE OF actor_kind, actor_id, agent_id ON execution
WHEN (NEW.actor_kind = 'human' AND NEW.agent_id IS NOT NULL)
  OR (NEW.actor_kind = 'agent' AND (NEW.agent_id IS NULL OR NEW.agent_id != NEW.actor_id))
BEGIN
    SELECT RAISE(ABORT, 'execution agent projection is invalid');
END;

CREATE TRIGGER execution_purpose_guard_insert
BEFORE INSERT ON execution
WHEN NEW.actor_kind IS NOT NULL
 AND NEW.purpose IS NULL
BEGIN
    SELECT RAISE(ABORT, 'actor-bearing Execution requires an explicit Purpose');
END;

CREATE TRIGGER execution_purpose_guard_update
BEFORE UPDATE OF actor_kind, purpose ON execution
WHEN NEW.actor_kind IS NOT NULL
 AND NEW.purpose IS NULL
BEGIN
    SELECT RAISE(ABORT, 'actor-bearing Execution requires an explicit Purpose');
END;

CREATE TRIGGER execution_harness_session_guard_insert
BEFORE INSERT ON execution
WHEN NEW.harness_session_id IS NOT NULL
 AND (
      NEW.actor_kind IS NOT 'agent'
      OR NOT EXISTS (
          SELECT 1
          FROM harness_session AS hs
          WHERE hs.id = NEW.harness_session_id
            AND hs.agent_id = NEW.actor_id
            AND hs.status IN ('pending', 'active')
            AND (hs.workspace_id IS NULL OR hs.workspace_id = NEW.workspace_id)
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'execution HarnessSession is incompatible with Actor or workspace');
END;

CREATE TRIGGER execution_harness_session_guard_update
BEFORE UPDATE OF harness_session_id, actor_kind, actor_id, workspace_id ON execution
WHEN NEW.harness_session_id IS NOT NULL
 AND (
      NEW.actor_kind IS NOT 'agent'
      OR NOT EXISTS (
          SELECT 1
          FROM harness_session AS hs
          WHERE hs.id = NEW.harness_session_id
            AND hs.agent_id = NEW.actor_id
            AND hs.status IN ('pending', 'active')
            AND (hs.workspace_id IS NULL OR hs.workspace_id = NEW.workspace_id)
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'execution HarnessSession is incompatible with Actor or workspace');
END;

CREATE TRIGGER execution_harness_projection_guard_insert
BEFORE INSERT ON execution
WHEN NEW.harness_session_id IS NOT NULL
 AND (
      NOT EXISTS (
          SELECT 1
          FROM harness_session AS hs
          WHERE hs.id = NEW.harness_session_id
            AND NEW.agent_session_id IS hs.external_session_id
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'Execution legacy session projection diverges from HarnessSession');
END;

CREATE TRIGGER execution_harness_projection_guard_update
BEFORE UPDATE OF harness_session_id, agent_session_id ON execution
WHEN NEW.harness_session_id IS NOT NULL
 AND (
      NOT EXISTS (
          SELECT 1
          FROM harness_session AS hs
          WHERE hs.id = NEW.harness_session_id
            AND NEW.agent_session_id IS hs.external_session_id
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'Execution legacy session projection diverges from HarnessSession');
END;

-- A direct lifecycle/external-identity update still flows one way from the
-- generic session authority into every historical Execution projection. The
-- result writer performs the same update in its surrounding transaction.
CREATE TRIGGER harness_session_projection_update
AFTER UPDATE OF external_session_id, status ON harness_session
WHEN NEW.external_session_id IS NOT NULL
BEGIN
    UPDATE execution
    SET agent_session_id = NEW.external_session_id,
        updated_at = NEW.updated_at
    WHERE harness_session_id = NEW.id;
END;

CREATE TRIGGER execution_actor_role_purpose_immutable
BEFORE UPDATE OF actor_kind, actor_id, role, purpose ON execution
WHEN OLD.actor_kind IS NOT NEW.actor_kind
  OR OLD.actor_id IS NOT NEW.actor_id
  OR OLD.role IS NOT NEW.role
  OR OLD.purpose IS NOT NEW.purpose
BEGIN
    SELECT RAISE(ABORT, 'Execution Actor, role, and Purpose are immutable');
END;

CREATE TRIGGER execution_harness_session_reference_guard
BEFORE UPDATE OF harness_session_id ON execution
WHEN (OLD.harness_session_id IS NOT NULL AND NEW.harness_session_id IS NULL)
  OR (OLD.harness_session_id IS NOT NULL
      AND NEW.harness_session_id IS NOT OLD.harness_session_id)
BEGIN
    SELECT RAISE(ABORT, 'Execution HarnessSession reference is immutable once attached');
END;

CREATE TRIGGER harness_session_identity_immutable
BEFORE UPDATE OF agent_id, harness_kind ON harness_session
BEGIN
    SELECT RAISE(ABORT, 'HarnessSession Agent and harness identity are immutable');
END;

CREATE TRIGGER harness_session_external_identity_immutable
BEFORE UPDATE OF external_session_id ON harness_session
WHEN OLD.external_session_id IS NOT NULL
 AND NEW.external_session_id IS NOT OLD.external_session_id
BEGIN
    SELECT RAISE(ABORT, 'HarnessSession external identity is immutable once known');
END;

CREATE TRIGGER harness_session_lifecycle_guard
BEFORE UPDATE OF status, external_session_id ON harness_session
WHEN (OLD.status IN ('ended', 'failed') AND NEW.status IN ('pending', 'active'))
  OR (OLD.status = 'active' AND NEW.status = 'pending')
BEGIN
    SELECT RAISE(ABORT, 'HarnessSession lifecycle transition is invalid');
END;

CREATE TRIGGER harness_session_snapshot_immutable
BEFORE UPDATE OF profile_id, profile_snapshot_json, capabilities_snapshot_json,
                 workspace_id, predecessor_session_id ON harness_session
WHEN OLD.profile_id IS NOT NEW.profile_id
  OR OLD.profile_snapshot_json IS NOT NEW.profile_snapshot_json
  OR OLD.capabilities_snapshot_json IS NOT NEW.capabilities_snapshot_json
  OR OLD.workspace_id IS NOT NEW.workspace_id
  OR OLD.predecessor_session_id IS NOT NEW.predecessor_session_id
BEGIN
    SELECT RAISE(ABORT, 'HarnessSession historical snapshot is immutable');
END;

CREATE TRIGGER harness_session_profile_guard_insert
BEFORE INSERT ON harness_session
WHEN NEW.profile_id IS NOT NULL
 AND NOT EXISTS (
     SELECT 1
     FROM agent_profile AS ap
     WHERE ap.id = NEW.profile_id
       AND ap.identity_id = NEW.agent_id
 )
BEGIN
    SELECT RAISE(ABORT, 'HarnessSession profile does not belong to its Agent');
END;

CREATE TRIGGER harness_session_profile_guard_update
BEFORE UPDATE OF profile_id ON harness_session
WHEN NEW.profile_id IS NOT NULL
 AND NOT EXISTS (
     SELECT 1
     FROM agent_profile AS ap
     WHERE ap.id = NEW.profile_id
       AND ap.identity_id = NEW.agent_id
 )
BEGIN
    SELECT RAISE(ABORT, 'HarnessSession profile does not belong to its Agent');
END;

CREATE TRIGGER harness_session_predecessor_guard_insert
BEFORE INSERT ON harness_session
WHEN NEW.predecessor_session_id IS NOT NULL
 AND NOT EXISTS (
     SELECT 1
     FROM harness_session AS predecessor
     WHERE predecessor.id = NEW.predecessor_session_id
       AND predecessor.agent_id = NEW.agent_id
       AND predecessor.harness_kind = NEW.harness_kind
 )
BEGIN
    SELECT RAISE(ABORT, 'HarnessSession predecessor has incompatible identity');
END;

CREATE TRIGGER harness_session_predecessor_guard_update
BEFORE UPDATE OF predecessor_session_id ON harness_session
WHEN NEW.predecessor_session_id IS NOT NULL
 AND NOT EXISTS (
     SELECT 1
     FROM harness_session AS predecessor
     WHERE predecessor.id = NEW.predecessor_session_id
       AND predecessor.agent_id = NEW.agent_id
       AND predecessor.harness_kind = NEW.harness_kind
 )
BEGIN
    SELECT RAISE(ABORT, 'HarnessSession predecessor has incompatible identity');
END;
