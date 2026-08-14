-- Repair runtime contracts introduced with the Project Charter orchestration
-- schema. Historical immutable records remain immutable during ordinary
-- operation; the deletion guard is populated only by ProjectRepo's bounded,
-- transactional teardown path.

CREATE TABLE project_deletion_guard (
    project_id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL
);

DROP TRIGGER project_charter_revision_immutable_delete;
CREATE TRIGGER project_charter_revision_immutable_delete
BEFORE DELETE ON project_charter_revision
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_charter c ON c.project_id = g.project_id
    WHERE c.id = OLD.charter_id
)
BEGIN
    SELECT RAISE(ABORT, 'Project Charter revisions are immutable');
END;

DROP TRIGGER project_charter_approval_event_immutable_delete;
CREATE TRIGGER project_charter_approval_event_immutable_delete
BEFORE DELETE ON project_charter_approval_event
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_charter c ON c.project_id = g.project_id
    JOIN project_charter_approval a ON a.charter_id = c.id
    WHERE a.id = OLD.approval_id
)
BEGIN
    SELECT RAISE(ABORT, 'Charter approval events are immutable');
END;

DROP TRIGGER project_canonical_conflict_immutable_delete;
CREATE TRIGGER project_canonical_conflict_immutable_delete
BEFORE DELETE ON project_canonical_conflict
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g WHERE g.project_id = OLD.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'Canonical conflicts are immutable');
END;

DROP TRIGGER project_reconciliation_resolution_immutable_delete;
CREATE TRIGGER project_reconciliation_resolution_immutable_delete
BEFORE DELETE ON project_reconciliation_resolution
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_reconciliation_record r ON r.project_id = g.project_id
    WHERE r.id = OLD.reconciliation_id
)
BEGIN
    SELECT RAISE(ABORT, 'Reconciliation resolutions are immutable');
END;

DROP TRIGGER project_charter_approval_immutable_delete;
CREATE TRIGGER project_charter_approval_immutable_delete
BEFORE DELETE ON project_charter_approval
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_charter c ON c.project_id = g.project_id
    WHERE c.id = OLD.charter_id
)
BEGIN
    SELECT RAISE(ABORT, 'Charter approvals are immutable');
END;

DROP TRIGGER project_document_revision_immutable_delete;
CREATE TRIGGER project_document_revision_immutable_delete
BEFORE DELETE ON project_document_revision
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_document d ON d.project_id = g.project_id
    WHERE d.id = OLD.document_id
)
BEGIN
    SELECT RAISE(ABORT, 'Project Document revisions are immutable');
END;

DROP TRIGGER project_document_approval_immutable_delete;
CREATE TRIGGER project_document_approval_immutable_delete
BEFORE DELETE ON project_document_approval
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_document d ON d.project_id = g.project_id
    WHERE d.id = OLD.document_id
)
BEGIN
    SELECT RAISE(ABORT, 'Project Document approvals are immutable');
END;

DROP TRIGGER project_decision_immutable_delete;
CREATE TRIGGER project_decision_immutable_delete
BEFORE DELETE ON project_decision
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g WHERE g.project_id = OLD.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'Project Decision records are append-only');
END;

DROP TRIGGER project_execution_baseline_approval_immutable_delete;
CREATE TRIGGER project_execution_baseline_approval_immutable_delete
BEFORE DELETE ON project_execution_baseline_approval
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_execution_baseline b ON b.project_id = g.project_id
    WHERE b.id = OLD.baseline_id
)
BEGIN
    SELECT RAISE(ABORT, 'Execution baseline approval receipts are immutable');
END;

DROP TRIGGER project_execution_baseline_revision_immutable_delete;
CREATE TRIGGER project_execution_baseline_revision_immutable_delete
BEFORE DELETE ON project_execution_baseline_revision
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_execution_baseline b ON b.project_id = g.project_id
    WHERE b.id = OLD.baseline_id
)
BEGIN
    SELECT RAISE(ABORT, 'Execution baseline revisions are immutable');
END;

DROP TRIGGER workspace_lease_immutable_delete;
CREATE TRIGGER workspace_lease_immutable_delete
BEFORE DELETE ON workspace_lease
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g WHERE g.project_id = OLD.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'Workspace leases are immutable');
END;

DROP TRIGGER project_milestone_revision_immutable_delete;
CREATE TRIGGER project_milestone_revision_immutable_delete
BEFORE DELETE ON project_milestone_revision
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_milestone m ON m.project_id = g.project_id
    WHERE m.id = OLD.milestone_id
)
BEGIN
    SELECT RAISE(ABORT, 'Milestone definition revisions are immutable');
END;

DROP TRIGGER project_milestone_check_result_immutable_delete;
CREATE TRIGGER project_milestone_check_result_immutable_delete
BEFORE DELETE ON project_milestone_check_result
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g WHERE g.project_id = OLD.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'Milestone check results are immutable');
END;

DROP TRIGGER project_readiness_input_immutable_delete;
CREATE TRIGGER project_readiness_input_immutable_delete
BEFORE DELETE ON project_readiness_input
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_readiness_snapshot s ON s.project_id = g.project_id
    WHERE s.id = OLD.readiness_snapshot_id
)
BEGIN
    SELECT RAISE(ABORT, 'Readiness snapshot inputs are immutable');
END;

DROP TRIGGER project_readiness_snapshot_immutable_delete;
CREATE TRIGGER project_readiness_snapshot_immutable_delete
BEFORE DELETE ON project_readiness_snapshot
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g WHERE g.project_id = OLD.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'Readiness snapshots are immutable');
END;

DROP TRIGGER project_release_immutable_delete;
CREATE TRIGGER project_release_immutable_delete
BEFORE DELETE ON project_release
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g WHERE g.project_id = OLD.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'Project releases are immutable');
END;

DROP TRIGGER project_release_reference_immutable_delete;
CREATE TRIGGER project_release_reference_immutable_delete
BEFORE DELETE ON project_release_reference
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN project_release r ON r.project_id = g.project_id
    WHERE r.id = OLD.release_id
)
BEGIN
    SELECT RAISE(ABORT, 'Release references are immutable');
END;

DROP TRIGGER project_release_media_pin_immutable_delete;
CREATE TRIGGER project_release_media_pin_immutable_delete
BEFORE DELETE ON project_release_media_pin
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g WHERE g.project_id = OLD.project_id
)
BEGIN
    SELECT RAISE(ABORT, 'Release media pins are immutable');
END;

DROP TRIGGER media_asset_tombstone_immutable_delete;
CREATE TRIGGER media_asset_tombstone_immutable_delete
BEFORE DELETE ON media_asset_tombstone
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN media_asset a ON a.project_id = g.project_id
    WHERE a.id = OLD.asset_id
)
BEGIN
    SELECT RAISE(ABORT, 'Media asset tombstones are immutable');
END;

-- A lease is renewable only by changing its expiry, version, and update time,
-- and only while the exact scheduler authority still resolves to a running
-- Task execution. Terminal lifecycle transitions remain one-way.
DROP TRIGGER workspace_lease_immutable_update;
CREATE TRIGGER workspace_lease_immutable_update
BEFORE UPDATE ON workspace_lease
WHEN OLD.id IS NOT NEW.id
  OR OLD.project_id IS NOT NEW.project_id
  OR OLD.task_id IS NOT NEW.task_id
  OR OLD.task_version IS NOT NEW.task_version
  OR OLD.execution_id IS NOT NEW.execution_id
  OR OLD.operation_idempotency_key IS NOT NEW.operation_idempotency_key
  OR OLD.repository_binding_id IS NOT NEW.repository_binding_id
  OR OLD.base_ref IS NOT NEW.base_ref
  OR OLD.role IS NOT NEW.role
  OR OLD.capabilities_json IS NOT NEW.capabilities_json
  OR OLD.assigned_principal_type IS NOT NEW.assigned_principal_type
  OR OLD.assigned_principal_id IS NOT NEW.assigned_principal_id
  OR OLD.capability_profile_revision IS NOT NEW.capability_profile_revision
  OR OLD.capability_profile_digest IS NOT NEW.capability_profile_digest
  OR OLD.issuing_principal_type IS NOT NEW.issuing_principal_type
  OR OLD.issuing_principal_id IS NOT NEW.issuing_principal_id
  OR OLD.issued_at IS NOT NEW.issued_at
  OR OLD.created_at IS NOT NEW.created_at
  OR OLD.version IS NOT NEW.version - 1
  OR OLD.status != 'active'
  OR NEW.status NOT IN ('active', 'expired', 'revoked')
  OR (NEW.status = 'active' AND NEW.revoked_at IS NOT NULL)
  OR (NEW.status IN ('expired', 'revoked') AND NEW.revoked_at IS NULL)
  OR (NEW.status != 'active' AND OLD.expires_at IS NOT NEW.expires_at)
BEGIN
    SELECT RAISE(ABORT, 'Workspace leases are immutable except for renewal or terminal lifecycle');
END;

DROP TRIGGER workspace_lease_active_immutable_guard;
CREATE TRIGGER workspace_lease_active_renewal_guard
BEFORE UPDATE ON workspace_lease
WHEN OLD.status = 'active' AND NEW.status = 'active'
BEGIN
    SELECT CASE
        WHEN NEW.expires_at <= OLD.expires_at
          OR NEW.updated_at IS OLD.updated_at
        THEN RAISE(ABORT, 'Workspace lease renewal must extend expiry')
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
            JOIN execution e ON e.id = NEW.execution_id
            LEFT JOIN project_task_governance g
              ON g.task_id = t.id AND g.project_id = p.id
            LEFT JOIN project_execution_baseline b
              ON b.id = g.baseline_id AND b.project_id = g.project_id
            LEFT JOIN project_execution_baseline_revision r
              ON r.id = g.baseline_revision_id AND r.baseline_id = b.id
            WHERE t.id = NEW.task_id
              AND t.project_id = NEW.project_id
              AND t.version = NEW.task_version
              AND t.repo_id = NEW.repository_binding_id
              AND e.task_id = NEW.task_id
              AND e.status = 'running'
              AND e.agent_id = NEW.assigned_principal_id
              AND ((NEW.role = 'reviewer' AND e.role = 'reviewer')
                   OR (NEW.role = 'worker' AND e.role != 'reviewer'))
              AND (
                  (t.assignee_type = NEW.assigned_principal_type
                   AND t.assignee_id = NEW.assigned_principal_id)
                  OR EXISTS (
                      SELECT 1 FROM task_role_assignment ra
                      WHERE ra.task_id = NEW.task_id
                        AND ra.role_name = e.role
                        AND ra.assignee_type = NEW.assigned_principal_type
                        AND ra.assignee_id = NEW.assigned_principal_id
                  )
                  OR ((p.charter_status != 'charter_backed'
                       OR p.charter_setup_required != 0)
                      AND t.assignee_type IS NULL AND t.assignee_id IS NULL)
              )
              AND json_array_length(NEW.capabilities_json) = 1
              AND json_extract(NEW.capabilities_json, '$[0]') =
                  COALESCE(g.capability_class,
                    CASE WHEN t.task_type IN ('planning_task', 'discovery')
                         THEN 'repository_read' ELSE 'repository_write' END)
              AND (
                  p.charter_status != 'charter_backed'
                  OR p.charter_setup_required != 0
                  OR (
                      g.runnable = 1
                      AND g.charter_revision_id = p.current_charter_revision_id
                      AND b.lifecycle = 'active'
                      AND b.current_revision_id = r.id
                      AND r.lifecycle = 'approved'
                      AND r.charter_revision_id = p.current_charter_revision_id
                      AND EXISTS (
                          SELECT 1 FROM project_execution_baseline_approval a
                          WHERE a.baseline_id = b.id AND a.revision_id = r.id
                            AND a.lifecycle IN ('active', 'consumed')
                            AND a.content_digest = r.content_digest
                            AND a.rendered_digest = r.rendered_digest
                      )
                  )
                  OR (
                      g.runnable = 0
                      AND g.baseline_id IS NULL
                      AND g.baseline_revision_id IS NULL
                      AND g.charter_revision_id = p.current_charter_revision_id
                      AND t.task_type IN ('planning_task', 'discovery')
                      AND g.capability_class IN
                          ('repository_read', 'read_only', 'discovery_read', 'planning_read')
                  )
              )
        ) THEN RAISE(ABORT, 'Workspace lease renewal authority is stale')
    END;
END;

-- Successful purge reconciliation is durable so each startup pass advances
-- instead of revisiting the same oldest rows forever.
CREATE TABLE media_asset_purge_reconciliation (
    asset_id       TEXT PRIMARY KEY REFERENCES media_asset(id) ON DELETE CASCADE,
    reconciled_at  TEXT NOT NULL
);

-- Scope approval/check idempotency identities to the operation, Project, and
-- principal while preserving the caller's key as the final opaque segment.
-- The physical UNIQUE constraints can now remain simple and race-safe.
UPDATE project_charter_approval
SET idempotency_key = 'forge-idem-v1:' || lower(hex('charter-approval')) || ':' ||
    lower(hex((SELECT COALESCE(c.project_id, 'account:' || c.account_id)
               FROM project_charter c WHERE c.id = charter_id))) || ':' ||
    lower(hex(approving_principal_id)) || ':' || idempotency_key;

UPDATE project_document_approval
SET idempotency_key = 'forge-idem-v1:' || lower(hex('document-approval')) || ':' ||
    lower(hex((SELECT d.project_id FROM project_document d WHERE d.id = document_id))) || ':' ||
    lower(hex(principal_id)) || ':' || idempotency_key;

UPDATE project_execution_baseline_approval
SET idempotency_key = 'forge-idem-v1:' || lower(hex('baseline-approval')) || ':' ||
    lower(hex((SELECT b.project_id FROM project_execution_baseline b WHERE b.id = baseline_id))) || ':' ||
    lower(hex(principal_id)) || ':' || idempotency_key;

UPDATE project_milestone_check_result
SET idempotency_key = 'forge-idem-v1:' || lower(hex('milestone-check')) || ':' ||
    lower(hex(project_id)) || ':' || lower(hex(principal_id)) || ':' || idempotency_key;
