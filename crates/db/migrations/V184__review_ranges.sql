-- Review ranges and per-repository coverage: additive persistence (contract C0.3).
-- No backfill: Tasks without a review policy keep their current behaviour.

CREATE TABLE project_review_policy (
    project_id          TEXT PRIMARY KEY REFERENCES project(id) ON DELETE CASCADE,
    version             INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    policy_json         TEXT CHECK (policy_json IS NULL OR (json_valid(policy_json)
                            AND json_type(policy_json) = 'object' AND length(policy_json) <= 4096)),
    digest              TEXT CHECK (digest IS NULL OR length(digest) = 64),
    updated_by_user_id  TEXT NOT NULL REFERENCES user(id) ON DELETE RESTRICT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    CHECK ((policy_json IS NULL) = (digest IS NULL))
);
CREATE TRIGGER project_review_policy_update_guard
BEFORE UPDATE ON project_review_policy
WHEN NEW.project_id IS NOT OLD.project_id OR NEW.created_at IS NOT OLD.created_at
  OR NEW.version != OLD.version + 1
BEGIN SELECT RAISE(ABORT, 'Project review policy advances exactly one version'); END;

CREATE TABLE task_review_policy_revision (
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    revision            INTEGER NOT NULL CHECK (revision >= 1),
    state               TEXT NOT NULL CHECK (state IN ('current', 'superseded')),
    policy_json         TEXT NOT NULL CHECK (json_valid(policy_json)
                            AND json_type(policy_json) = 'object' AND length(policy_json) <= 4096
                            AND json_extract(policy_json, '$.schema_version') = 1),
    digest              TEXT NOT NULL CHECK (length(digest) = 64),
    risk_class          TEXT NOT NULL CHECK (risk_class IN ('r0', 'r1', 'r2')),
    coverage_required   INTEGER NOT NULL CHECK (coverage_required IN (0, 1)),
    source              TEXT NOT NULL CHECK (source IN ('project_default', 'owner')),
    created_by_user_id  TEXT NOT NULL REFERENCES user(id) ON DELETE RESTRICT,
    created_at          TEXT NOT NULL,
    superseded_at       TEXT,
    PRIMARY KEY (task_id, revision),
    CHECK ((state = 'superseded') = (superseded_at IS NOT NULL)),
    CHECK ((risk_class = 'r0') = (coverage_required = 0))
);
CREATE UNIQUE INDEX idx_task_review_policy_current
    ON task_review_policy_revision(task_id) WHERE state = 'current';
CREATE TRIGGER task_review_policy_revision_insert_guard
BEFORE INSERT ON task_review_policy_revision
WHEN NEW.state != 'current'
  OR NEW.revision != COALESCE((SELECT MAX(revision) FROM task_review_policy_revision
                               WHERE task_id = NEW.task_id), 0) + 1
  OR json_extract(NEW.policy_json, '$.risk_class') IS NOT NEW.risk_class
  OR json_extract(NEW.policy_json, '$.coverage_required') IS NOT NEW.coverage_required
BEGIN SELECT RAISE(ABORT, 'Task review policy revision must be the next exact current revision'); END;
CREATE TRIGGER task_review_policy_revision_update_guard
BEFORE UPDATE ON task_review_policy_revision
WHEN OLD.state != 'current' OR NEW.state != 'superseded'
  OR NEW.task_id IS NOT OLD.task_id OR NEW.revision IS NOT OLD.revision
  OR NEW.policy_json IS NOT OLD.policy_json OR NEW.digest IS NOT OLD.digest
  OR NEW.risk_class IS NOT OLD.risk_class OR NEW.coverage_required IS NOT OLD.coverage_required
  OR NEW.source IS NOT OLD.source OR NEW.created_by_user_id IS NOT OLD.created_by_user_id
  OR NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT, 'Task review policy revisions are immutable except supersession'); END;

CREATE TABLE task_review_base (
    task_id      TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id      TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    revision     INTEGER NOT NULL CHECK (revision >= 1),
    base_commit  TEXT NOT NULL CHECK (length(trim(base_commit)) > 0),
    base_tree    TEXT NOT NULL CHECK (length(trim(base_tree)) > 0),
    source       TEXT NOT NULL CHECK (source IN
                     ('first_integration', 'workspace_prepare', 'human_rebaseline')),
    decision_id  TEXT REFERENCES decision(id) ON DELETE RESTRICT,
    pinned_at    TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id, revision),
    CHECK ((revision > 1) = (source = 'human_rebaseline')),
    CHECK ((source = 'human_rebaseline') = (decision_id IS NOT NULL))
);
CREATE TRIGGER task_review_base_insert_guard
BEFORE INSERT ON task_review_base
WHEN NEW.revision != COALESCE((SELECT MAX(revision) FROM task_review_base
        WHERE task_id = NEW.task_id AND repo_id = NEW.repo_id), 0) + 1
  OR NOT EXISTS (
        SELECT 1 FROM task_repository_set_revision v
        JOIN task_repository_set_revision_repo m
          ON m.task_id = v.task_id AND m.revision = v.revision
        WHERE v.task_id = NEW.task_id AND v.state = 'current'
          AND m.repo_id = NEW.repo_id AND m.role IN ('primary', 'affected'))
  OR (NEW.decision_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM decision d
        JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
        JOIN decision_actor a ON a.decision_id = d.id AND a.actor_kind = 'human'
        JOIN task t ON t.id = d.task_id
        JOIN project_member pm ON pm.project_id = t.project_id AND pm.user_id = a.actor_id
          AND pm.role IN ('owner', 'admin')
        WHERE d.id = NEW.decision_id AND d.task_id = NEW.task_id AND d.outcome = 'approve'
          AND p.target_kind = 'review_base' AND p.target_id = NEW.repo_id))
BEGIN SELECT RAISE(ABORT, 'Review base must be sequential, writable and Human-authorized after revision 1'); END;

CREATE TABLE review_commit_attribution (
    id           TEXT PRIMARY KEY,
    task_id      TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id      TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    commit_sha   TEXT NOT NULL CHECK (length(trim(commit_sha)) > 0),
    actor_kind   TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id     TEXT NOT NULL CHECK (length(trim(actor_id)) > 0),
    decision_id  TEXT NOT NULL REFERENCES decision(id) ON DELETE RESTRICT,
    created_at   TEXT NOT NULL,
    UNIQUE (task_id, repo_id, commit_sha)
);
CREATE TRIGGER review_commit_attribution_insert_guard
BEFORE INSERT ON review_commit_attribution
WHEN (NEW.actor_kind = 'human' AND NOT EXISTS (SELECT 1 FROM user u WHERE u.id = NEW.actor_id))
  OR (NEW.actor_kind = 'agent' AND NOT EXISTS (SELECT 1 FROM agent_identity a WHERE a.id = NEW.actor_id))
  OR NOT EXISTS (
        SELECT 1 FROM decision d
        JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
        JOIN decision_actor a ON a.decision_id = d.id AND a.actor_kind = 'human'
        JOIN task t ON t.id = d.task_id
        JOIN project_member pm ON pm.project_id = t.project_id AND pm.user_id = a.actor_id
          AND pm.role IN ('owner', 'admin')
        WHERE d.id = NEW.decision_id AND d.task_id = NEW.task_id AND d.outcome = 'approve'
          AND p.target_kind = 'review_commit' AND p.target_id = NEW.repo_id)
BEGIN SELECT RAISE(ABORT, 'Commit attribution requires an exact owner/admin Decision'); END;

CREATE TABLE review_range (
    id                       TEXT PRIMARY KEY,
    task_id                  TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id                  TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    repository_set_revision  INTEGER NOT NULL,
    base_commit              TEXT NOT NULL CHECK (length(trim(base_commit)) > 0),
    head_commit              TEXT NOT NULL CHECK (length(trim(head_commit)) > 0),
    base_tree                TEXT NOT NULL CHECK (length(trim(base_tree)) > 0),
    head_tree                TEXT NOT NULL CHECK (length(trim(head_tree)) > 0),
    range_digest             TEXT NOT NULL CHECK (length(range_digest) = 64),
    integration_workspace_id TEXT NOT NULL REFERENCES workspace(id) ON DELETE RESTRICT,
    head_integration_id      TEXT NOT NULL
                                 REFERENCES work_unit_repository_integration(id) ON DELETE RESTRICT,
    created_at               TEXT NOT NULL,
    UNIQUE (task_id, repo_id, base_commit, head_commit),
    UNIQUE (id, task_id, repo_id),
    CHECK (base_commit != head_commit),
    FOREIGN KEY (task_id, repository_set_revision, repo_id)
        REFERENCES task_repository_set_revision_repo(task_id, revision, repo_id) ON DELETE RESTRICT
);
CREATE TRIGGER review_range_insert_guard
BEFORE INSERT ON review_range
WHEN NOT EXISTS (SELECT 1 FROM task_review_policy_revision p
                 WHERE p.task_id = NEW.task_id AND p.state = 'current')
  OR NOT EXISTS (
        SELECT 1 FROM task_repository_set_revision v
        JOIN task_repository_set_revision_repo m
          ON m.task_id = v.task_id AND m.revision = v.revision
        WHERE v.task_id = NEW.task_id AND v.state = 'current'
          AND v.revision = NEW.repository_set_revision
          AND m.repo_id = NEW.repo_id AND m.role IN ('primary', 'affected'))
  OR NOT EXISTS (SELECT 1 FROM workspace_scope s
        WHERE s.workspace_id = NEW.integration_workspace_id AND s.task_id = NEW.task_id
          AND s.repo_id = NEW.repo_id AND s.scope_kind = 'integration')
  OR NOT EXISTS (SELECT 1 FROM work_unit_repository_integration i
        WHERE i.id = NEW.head_integration_id AND i.task_id = NEW.task_id
          AND i.repo_id = NEW.repo_id AND i.outcome = 'success'
          AND i.target_after_sha = NEW.head_commit
          AND i.target_workspace_id = NEW.integration_workspace_id)
BEGIN SELECT RAISE(ABORT, 'Review range must end at an exact successful integration of a writable repository'); END;

CREATE TABLE review_checkpoint (
    id                   TEXT PRIMARY KEY,
    task_id              TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id              TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    state                TEXT NOT NULL CHECK (state IN
                             ('queued', 'pending', 'in_review', 'reviewed', 'blocked', 'superseded')),
    reasons_json         TEXT NOT NULL CHECK (json_valid(reasons_json)
                             AND json_type(reasons_json) = 'array'
                             AND json_array_length(reasons_json) BETWEEN 1 AND 7),
    attempt              INTEGER NOT NULL CHECK (attempt >= 1),
    base_commit          TEXT,
    head_commit          TEXT NOT NULL CHECK (length(trim(head_commit)) > 0),
    range_id             TEXT,
    policy_revision      INTEGER NOT NULL,
    blocked_code         TEXT,
    blocked_detail_json  TEXT CHECK (blocked_detail_json IS NULL OR (json_valid(blocked_detail_json)
                             AND json_type(blocked_detail_json) = 'object'
                             AND length(blocked_detail_json) <= 16384)),
    version              INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL,
    UNIQUE (id, task_id, repo_id),
    FOREIGN KEY (range_id, task_id, repo_id)
        REFERENCES review_range(id, task_id, repo_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, policy_revision)
        REFERENCES task_review_policy_revision(task_id, revision) ON DELETE RESTRICT,
    CHECK (state != 'queued' OR (range_id IS NULL AND base_commit IS NULL)),
    CHECK (state NOT IN ('pending', 'in_review', 'reviewed')
           OR (range_id IS NOT NULL AND base_commit IS NOT NULL)),
    CHECK ((state = 'blocked') = (blocked_code IS NOT NULL))
);
CREATE UNIQUE INDEX idx_review_checkpoint_active
    ON review_checkpoint(task_id, repo_id) WHERE state IN ('pending', 'in_review');
CREATE UNIQUE INDEX idx_review_checkpoint_exact
    ON review_checkpoint(task_id, repo_id, base_commit, head_commit, attempt)
    WHERE base_commit IS NOT NULL;
CREATE UNIQUE INDEX idx_review_checkpoint_queued
    ON review_checkpoint(task_id, repo_id, head_commit) WHERE state = 'queued';
CREATE INDEX idx_review_checkpoint_task
    ON review_checkpoint(task_id, created_at DESC, id DESC);
CREATE TRIGGER review_checkpoint_insert_guard
BEFORE INSERT ON review_checkpoint
WHEN NEW.version != 1 OR NEW.state NOT IN ('queued', 'blocked')
  OR NOT EXISTS (SELECT 1 FROM task_review_policy_revision p
        WHERE p.task_id = NEW.task_id AND p.revision = NEW.policy_revision AND p.state = 'current')
  OR (NEW.range_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM review_range r
        WHERE r.id = NEW.range_id AND r.base_commit = NEW.base_commit
          AND r.head_commit = NEW.head_commit))
BEGIN SELECT RAISE(ABORT, 'Review checkpoint must start queued or blocked under the current policy'); END;
CREATE TRIGGER review_checkpoint_update_guard
BEFORE UPDATE ON review_checkpoint
WHEN NEW.version != OLD.version + 1
  OR NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id OR NEW.repo_id IS NOT OLD.repo_id
  OR NEW.attempt IS NOT OLD.attempt OR NEW.head_commit IS NOT OLD.head_commit
  OR NEW.policy_revision IS NOT OLD.policy_revision OR NEW.created_at IS NOT OLD.created_at
  OR (OLD.base_commit IS NOT NULL AND NEW.base_commit IS NOT OLD.base_commit)
  OR (OLD.range_id IS NOT NULL AND NEW.range_id IS NOT OLD.range_id)
  OR (NEW.range_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM review_range r
        WHERE r.id = NEW.range_id AND r.base_commit = NEW.base_commit
          AND r.head_commit = NEW.head_commit))
  OR NOT (
        (OLD.state = NEW.state AND OLD.state IN ('queued', 'pending', 'in_review'))
     OR (OLD.state = 'queued' AND NEW.state IN ('pending', 'blocked', 'superseded'))
     OR (OLD.state = 'pending' AND NEW.state IN ('in_review', 'reviewed', 'blocked', 'superseded'))
     OR (OLD.state = 'in_review' AND NEW.state IN ('reviewed', 'blocked'))
     OR (OLD.state = 'blocked' AND NEW.state = 'superseded'))
BEGIN SELECT RAISE(ABORT, 'Review checkpoint transition or identity is invalid'); END;

CREATE TABLE review_checkpoint_request (
    task_id          TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    idempotency_key  TEXT NOT NULL CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    checkpoint_id    TEXT NOT NULL REFERENCES review_checkpoint(id) ON DELETE RESTRICT,
    source_kind      TEXT NOT NULL CHECK (source_kind IN ('human', 'coordinator', 'scheduler')),
    source_ref       TEXT NOT NULL CHECK (length(trim(source_ref)) > 0),
    reason           TEXT NOT NULL CHECK (length(CAST(reason AS BLOB)) BETWEEN 1 AND 1024),
    created_at       TEXT NOT NULL,
    PRIMARY KEY (task_id, idempotency_key)
);

CREATE TABLE review_plan_checkpoint (
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    plan_artifact_id        TEXT NOT NULL REFERENCES artifact(id) ON DELETE RESTRICT,
    plan_key                TEXT NOT NULL CHECK (length(trim(plan_key)) BETWEEN 1 AND 64),
    work_unit_ids_json      TEXT NOT NULL CHECK (json_valid(work_unit_ids_json)
                                AND json_type(work_unit_ids_json) = 'array'
                                AND json_array_length(work_unit_ids_json) >= 1),
    repository_ids_json     TEXT NOT NULL CHECK (json_valid(repository_ids_json)
                                AND json_type(repository_ids_json) = 'array'
                                AND json_array_length(repository_ids_json) >= 1),
    reason                  TEXT NOT NULL CHECK (length(CAST(reason AS BLOB)) BETWEEN 1 AND 1024),
    state                   TEXT NOT NULL CHECK (state IN ('active', 'superseded')),
    created_at              TEXT NOT NULL,
    superseded_at           TEXT,
    PRIMARY KEY (task_id, plan_artifact_id, plan_key),
    CHECK ((state = 'superseded') = (superseded_at IS NOT NULL))
);
CREATE TRIGGER review_plan_checkpoint_update_guard
BEFORE UPDATE ON review_plan_checkpoint
WHEN OLD.state != 'active' OR NEW.state != 'superseded'
  OR NEW.task_id IS NOT OLD.task_id OR NEW.plan_artifact_id IS NOT OLD.plan_artifact_id
  OR NEW.plan_key IS NOT OLD.plan_key OR NEW.work_unit_ids_json IS NOT OLD.work_unit_ids_json
  OR NEW.repository_ids_json IS NOT OLD.repository_ids_json
  OR NEW.reason IS NOT OLD.reason OR NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT, 'Plan review checkpoints are immutable except supersession'); END;

CREATE TABLE review_range_contributor (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    range_id        TEXT NOT NULL REFERENCES review_range(id) ON DELETE RESTRICT,
    work_unit_id    TEXT REFERENCES work_unit(id) ON DELETE RESTRICT,
    execution_id    TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    attribution_id  TEXT REFERENCES review_commit_attribution(id) ON DELETE RESTRICT,
    actor_kind      TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id        TEXT NOT NULL CHECK (length(trim(actor_id)) > 0),
    created_at      TEXT NOT NULL,
    CHECK ((execution_id IS NOT NULL AND work_unit_id IS NOT NULL AND attribution_id IS NULL)
        OR (execution_id IS NULL AND work_unit_id IS NULL AND attribution_id IS NOT NULL))
);
CREATE UNIQUE INDEX idx_review_range_contributor_execution
    ON review_range_contributor(range_id, execution_id) WHERE execution_id IS NOT NULL;
CREATE UNIQUE INDEX idx_review_range_contributor_attribution
    ON review_range_contributor(range_id, attribution_id) WHERE attribution_id IS NOT NULL;
CREATE INDEX idx_review_range_contributor_actor
    ON review_range_contributor(range_id, actor_kind, actor_id);

CREATE TABLE work_unit_review_range (
    review_work_unit_id    TEXT PRIMARY KEY,
    task_id                TEXT NOT NULL,
    checkpoint_id          TEXT NOT NULL,
    range_id               TEXT NOT NULL,
    repo_id                TEXT NOT NULL,
    input_findings_json    TEXT NOT NULL CHECK (json_valid(input_findings_json)
                               AND json_type(input_findings_json) = 'array'),
    input_findings_digest  TEXT NOT NULL CHECK (length(input_findings_digest) = 64),
    created_at             TEXT NOT NULL,
    FOREIGN KEY (review_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (checkpoint_id, task_id, repo_id)
        REFERENCES review_checkpoint(id, task_id, repo_id) ON DELETE RESTRICT,
    FOREIGN KEY (range_id, task_id, repo_id)
        REFERENCES review_range(id, task_id, repo_id) ON DELETE RESTRICT
);
CREATE INDEX idx_work_unit_review_range_checkpoint
    ON work_unit_review_range(checkpoint_id, review_work_unit_id);

CREATE TRIGGER review_range_contributor_insert_guard
BEFORE INSERT ON review_range_contributor
WHEN NOT EXISTS (SELECT 1 FROM review_range r WHERE r.id = NEW.range_id AND r.task_id = NEW.task_id)
  OR EXISTS (SELECT 1 FROM work_unit_review_range w WHERE w.range_id = NEW.range_id)
  OR (NEW.execution_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM execution e
        WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
          AND e.work_unit_id = NEW.work_unit_id
          AND e.actor_kind = NEW.actor_kind AND e.actor_id = NEW.actor_id))
  OR (NEW.attribution_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM review_commit_attribution a
        JOIN review_range r ON r.id = NEW.range_id AND r.repo_id = a.repo_id
        WHERE a.id = NEW.attribution_id AND a.task_id = NEW.task_id
          AND a.actor_kind = NEW.actor_kind AND a.actor_id = NEW.actor_id))
BEGIN SELECT RAISE(ABORT, 'Range contributors are exact, same-Task and frozen before review'); END;

CREATE TRIGGER work_unit_review_range_insert_guard
BEFORE INSERT ON work_unit_review_range
WHEN EXISTS (SELECT 1 FROM work_unit_review_subject s WHERE s.review_work_unit_id = NEW.review_work_unit_id)
  OR EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = NEW.review_work_unit_id)
  OR NOT EXISTS (
        SELECT 1 FROM work_unit w
        JOIN task_role role ON role.task_id = w.task_id AND role.role = w.role
        JOIN task_role_capability_configuration configuration ON configuration.task_role_id = role.id
        WHERE w.id = NEW.review_work_unit_id AND w.task_id = NEW.task_id
          AND w.status = 'open' AND w.requires_integration = 0
          AND EXISTS (SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
                      WHERE type = 'text' AND value = 'formal_review'))
  OR NOT EXISTS (SELECT 1 FROM review_checkpoint c
        WHERE c.id = NEW.checkpoint_id AND c.range_id = NEW.range_id AND c.state = 'pending')
  OR NOT EXISTS (SELECT 1 FROM work_unit_repository s
        WHERE s.work_unit_id = NEW.review_work_unit_id AND s.repo_id = NEW.repo_id)
  OR EXISTS (SELECT 1 FROM work_unit_repository s
        WHERE s.work_unit_id = NEW.review_work_unit_id AND s.repo_id != NEW.repo_id)
  OR EXISTS (SELECT 1 FROM work_unit w
        JOIN review_range_contributor c ON c.range_id = NEW.range_id
        WHERE w.id = NEW.review_work_unit_id AND w.assigned_actor_kind = 'human'
          AND c.actor_kind = 'human' AND c.actor_id = w.assigned_actor_id)
BEGIN SELECT RAISE(ABORT, 'Range review requires an exact independent formal-review WorkUnit'); END;

CREATE TRIGGER work_unit_review_subject_excludes_range_insert
BEFORE INSERT ON work_unit_review_subject
WHEN EXISTS (SELECT 1 FROM work_unit_review_range r WHERE r.review_work_unit_id = NEW.review_work_unit_id)
BEGIN SELECT RAISE(ABORT, 'A WorkUnit reviews either a result or a commit range'); END;

CREATE TABLE review_range_report (
    report_artifact_id   TEXT PRIMARY KEY REFERENCES artifact(id) ON DELETE RESTRICT,
    task_id              TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id              TEXT NOT NULL,
    range_id             TEXT NOT NULL,
    checkpoint_id        TEXT NOT NULL,
    review_work_unit_id  TEXT NOT NULL
                             REFERENCES work_unit_review_range(review_work_unit_id) ON DELETE RESTRICT,
    execution_id         TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    actor_kind           TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id             TEXT NOT NULL,
    verdict              TEXT NOT NULL CHECK (verdict IN ('pass', 'request_changes', 'questions')),
    report_digest        TEXT NOT NULL CHECK (length(trim(report_digest)) > 0),
    created_at           TEXT NOT NULL,
    UNIQUE (checkpoint_id, execution_id),
    FOREIGN KEY (range_id, task_id, repo_id)
        REFERENCES review_range(id, task_id, repo_id) ON DELETE RESTRICT,
    FOREIGN KEY (checkpoint_id, task_id, repo_id)
        REFERENCES review_checkpoint(id, task_id, repo_id) ON DELETE RESTRICT
);
CREATE INDEX idx_review_range_report_range ON review_range_report(range_id, verdict);
CREATE TRIGGER review_range_report_insert_guard
BEFORE INSERT ON review_range_report
WHEN NOT EXISTS (
    SELECT 1 FROM artifact a
    JOIN artifact_execution_producer p ON p.artifact_id = a.id AND p.task_id = a.task_id
    JOIN execution e ON e.id = p.execution_id AND e.task_id = p.task_id
    JOIN work_unit_review_range w ON w.review_work_unit_id = e.work_unit_id
    JOIN review_checkpoint c ON c.id = w.checkpoint_id
    JOIN review_range r ON r.id = w.range_id
    WHERE a.id = NEW.report_artifact_id AND a.task_id = NEW.task_id AND a.kind = 'review_report'
      AND a.digest = NEW.report_digest AND e.id = NEW.execution_id
      AND e.actor_kind = NEW.actor_kind AND e.actor_id = NEW.actor_id
      AND w.review_work_unit_id = NEW.review_work_unit_id
      AND w.checkpoint_id = NEW.checkpoint_id AND w.range_id = NEW.range_id
      AND w.repo_id = NEW.repo_id AND c.state IN ('pending', 'in_review')
      AND json_extract(a.content, '$.verdict') = NEW.verdict
      AND json_extract(a.content, '$.subject.range.range_digest') = r.range_digest)
BEGIN SELECT RAISE(ABORT, 'Range report must bind its exact active checkpoint, range and producer'); END;

CREATE TRIGGER review_report_range_subject_insert
BEFORE INSERT ON artifact
WHEN NEW.kind = 'review_report' AND (
    EXISTS (
        SELECT 1 FROM execution_artifact_output o
        JOIN execution e ON e.id = o.execution_id AND e.task_id = o.task_id
        JOIN work_unit_review_range w ON w.review_work_unit_id = e.work_unit_id
        JOIN review_range r ON r.id = w.range_id
        WHERE o.artifact_id = NEW.id AND o.task_id = NEW.task_id AND (
              json_extract(NEW.content, '$.subject.kind') IS NOT 'commit_range'
           OR json_extract(NEW.content, '$.subject.checkpoint_id') IS NOT w.checkpoint_id
           OR json_extract(NEW.content, '$.subject.range.id') IS NOT r.id
           OR json_extract(NEW.content, '$.subject.range.range_digest') IS NOT r.range_digest))
    OR (json_extract(NEW.content, '$.subject.kind') = 'commit_range' AND NOT EXISTS (
        SELECT 1 FROM execution_artifact_output o
        JOIN execution e ON e.id = o.execution_id AND e.task_id = o.task_id
        JOIN work_unit_review_range w ON w.review_work_unit_id = e.work_unit_id
        WHERE o.artifact_id = NEW.id AND o.task_id = NEW.task_id)))
BEGIN SELECT RAISE(ABORT, 'ReviewReport commit_range subject must match its frozen range'); END;

CREATE TABLE review_finding (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id             TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    range_id            TEXT NOT NULL REFERENCES review_range(id) ON DELETE RESTRICT,
    report_artifact_id  TEXT NOT NULL
                            REFERENCES review_range_report(report_artifact_id) ON DELETE RESTRICT,
    ordinal             INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 99),
    label               TEXT NOT NULL CHECK (length(trim(label)) BETWEEN 1 AND 32),
    severity            TEXT NOT NULL CHECK (severity IN ('P0', 'P1', 'P2', 'P3')),
    title               TEXT NOT NULL CHECK (length(CAST(title AS BLOB)) BETWEEN 1 AND 256),
    evidence            TEXT NOT NULL CHECK (length(CAST(evidence AS BLOB)) BETWEEN 1 AND 4096),
    invariant_text      TEXT CHECK (invariant_text IS NULL
                            OR length(CAST(invariant_text AS BLOB)) BETWEEN 1 AND 512),
    anchor_commit       TEXT NOT NULL,
    digest              TEXT NOT NULL CHECK (length(digest) = 64),
    created_at          TEXT NOT NULL,
    UNIQUE (report_artifact_id, ordinal),
    UNIQUE (report_artifact_id, label)
);
CREATE INDEX idx_review_finding_task ON review_finding(task_id, repo_id, severity, id);
CREATE TRIGGER review_finding_insert_guard
BEFORE INSERT ON review_finding
WHEN NOT EXISTS (SELECT 1 FROM review_range_report p
    JOIN review_range r ON r.id = p.range_id
    WHERE p.report_artifact_id = NEW.report_artifact_id AND p.task_id = NEW.task_id
      AND p.repo_id = NEW.repo_id AND p.range_id = NEW.range_id
      AND p.verdict != 'questions' AND r.head_commit = NEW.anchor_commit
      AND NOT (p.verdict = 'pass' AND NEW.severity IN ('P0', 'P1')))
BEGIN SELECT RAISE(ABORT, 'Finding must belong to its exact range report'); END;

CREATE TABLE review_finding_location (
    finding_id               TEXT NOT NULL REFERENCES review_finding(id) ON DELETE RESTRICT,
    task_id                  TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    ordinal                  INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 15),
    path                     TEXT NOT NULL CHECK (length(CAST(path AS BLOB)) BETWEEN 1 AND 1024),
    line_start               INTEGER CHECK (line_start IS NULL OR line_start >= 1),
    line_end                 INTEGER,
    attribution_kind         TEXT NOT NULL CHECK (attribution_kind IN
                                 ('work_unit', 'preexisting', 'upstream', 'unattributed')),
    attributed_work_unit_id  TEXT REFERENCES work_unit(id) ON DELETE RESTRICT,
    attributed_execution_id  TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    attributed_actor_kind    TEXT CHECK (attributed_actor_kind IS NULL
                                 OR attributed_actor_kind IN ('human', 'agent')),
    attributed_actor_id      TEXT,
    attributed_commit_sha    TEXT,
    attributed_integration_id TEXT
                                 REFERENCES work_unit_repository_integration(id) ON DELETE RESTRICT,
    attributed_line_count    INTEGER NOT NULL DEFAULT 0 CHECK (attributed_line_count >= 0),
    PRIMARY KEY (finding_id, ordinal),
    CHECK ((line_start IS NULL) = (line_end IS NULL)),
    CHECK (line_end IS NULL OR line_end >= line_start),
    CHECK ((attributed_actor_kind IS NULL) = (attributed_actor_id IS NULL)),
    CHECK ((attribution_kind = 'work_unit') = (attributed_work_unit_id IS NOT NULL))
);

CREATE TABLE review_finding_resolution (
    id                  TEXT PRIMARY KEY,
    finding_id          TEXT NOT NULL REFERENCES review_finding(id) ON DELETE RESTRICT,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id             TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    outcome             TEXT NOT NULL CHECK (outcome IN ('fixed', 'not_reproduced', 'waived')),
    anchor_commit       TEXT NOT NULL CHECK (length(trim(anchor_commit)) > 0),
    report_artifact_id  TEXT REFERENCES review_range_report(report_artifact_id) ON DELETE RESTRICT,
    decision_id         TEXT REFERENCES decision(id) ON DELETE RESTRICT,
    waiver_reference    TEXT CHECK (waiver_reference IS NULL
                            OR (length(CAST(waiver_reference AS BLOB)) BETWEEN 1 AND 512
                                AND length(trim(waiver_reference)) > 0)),
    evidence            TEXT NOT NULL CHECK (length(CAST(evidence AS BLOB)) BETWEEN 1 AND 4096),
    actor_kind          TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id            TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    UNIQUE (finding_id, outcome, anchor_commit),
    CHECK ((outcome = 'waived' AND decision_id IS NOT NULL AND waiver_reference IS NOT NULL
              AND report_artifact_id IS NULL AND actor_kind = 'human')
        OR (outcome != 'waived' AND decision_id IS NULL AND waiver_reference IS NULL
              AND report_artifact_id IS NOT NULL))
);
CREATE INDEX idx_review_finding_resolution_task
    ON review_finding_resolution(task_id, repo_id, finding_id);
CREATE TRIGGER review_finding_resolution_insert_guard
BEFORE INSERT ON review_finding_resolution
WHEN NOT EXISTS (SELECT 1 FROM review_finding f
        WHERE f.id = NEW.finding_id AND f.task_id = NEW.task_id AND f.repo_id = NEW.repo_id)
  OR (NEW.outcome != 'waived' AND NOT EXISTS (
        SELECT 1 FROM review_range_report p
        JOIN review_range r ON r.id = p.range_id
        JOIN review_checkpoint c ON c.id = p.checkpoint_id
        JOIN work_unit_review_range w ON w.review_work_unit_id = p.review_work_unit_id
        WHERE p.report_artifact_id = NEW.report_artifact_id AND p.task_id = NEW.task_id
          AND p.repo_id = NEW.repo_id AND p.verdict != 'questions'
          AND r.head_commit = NEW.anchor_commit
          AND p.actor_kind = NEW.actor_kind AND p.actor_id = NEW.actor_id
          AND EXISTS (SELECT 1 FROM json_each(w.input_findings_json) i
                      WHERE json_extract(i.value, '$.finding_id') = NEW.finding_id)
          AND (NEW.outcome != 'not_reproduced' OR EXISTS (
                SELECT 1 FROM json_each(c.reasons_json) reason WHERE reason.value = 'recheck'))))
  OR (NEW.outcome = 'waived' AND NOT EXISTS (
        SELECT 1 FROM review_finding f
        JOIN decision d ON d.id = NEW.decision_id AND d.task_id = f.task_id AND d.outcome = 'approve'
        JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
        JOIN decision_actor a ON a.decision_id = d.id AND a.actor_kind = 'human'
          AND a.actor_id = NEW.actor_id
        JOIN task t ON t.id = f.task_id
        WHERE f.id = NEW.finding_id AND f.severity != 'P0'
          AND p.target_kind = 'review_finding' AND p.target_id = f.id
          AND p.target_digest = f.digest
          AND length(trim(NEW.waiver_reference)) > 0
          AND (
              EXISTS (SELECT 1 FROM project_member pm
                      WHERE pm.project_id = t.project_id AND pm.user_id = a.actor_id
                        AND pm.role IN ('owner', 'admin'))
              OR EXISTS (
                  SELECT 1 FROM task_role tr
                  JOIN role_membership rm ON rm.task_role_id = tr.id
                  WHERE tr.task_id = f.task_id AND rm.actor_kind = 'human'
                    AND rm.actor_id = a.actor_id AND rm.status = 'active'
              )
          )))
BEGIN SELECT RAISE(ABORT, 'FORGE_REVIEW_FINDING_RESOLUTION_NOT_AUTHORIZED'); END;

CREATE TABLE review_finding_triage (
    finding_id             TEXT NOT NULL REFERENCES review_finding(id) ON DELETE RESTRICT,
    request_id             TEXT NOT NULL
                               REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    task_id                TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    report_artifact_id     TEXT NOT NULL
                               REFERENCES review_range_report(report_artifact_id) ON DELETE RESTRICT,
    execution_id           TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    authorization_version  INTEGER NOT NULL CHECK (authorization_version >= 1),
    disposition            TEXT NOT NULL CHECK (disposition IN ('correct', 'recheck', 'human', 'defer')),
    rationale              TEXT NOT NULL CHECK (length(CAST(rationale AS BLOB)) BETWEEN 1 AND 2048),
    instructions           TEXT NOT NULL CHECK (length(CAST(instructions AS BLOB)) <= 8192),
    created_at             TEXT NOT NULL,
    PRIMARY KEY (finding_id, request_id)
);
CREATE TRIGGER review_finding_triage_insert_guard
BEFORE INSERT ON review_finding_triage
WHEN NOT EXISTS (
        SELECT 1 FROM task_coordination_request q
        WHERE q.id = NEW.request_id AND q.task_id = NEW.task_id AND q.kind = 'review_findings'
          AND q.review_report_artifact_id = NEW.report_artifact_id
          AND q.execution_id = NEW.execution_id
          AND q.authorization_version = NEW.authorization_version)
  OR NOT EXISTS (SELECT 1 FROM review_finding f
        WHERE f.id = NEW.finding_id AND f.task_id = NEW.task_id
          AND (NEW.disposition != 'defer' OR f.severity IN ('P2', 'P3')))
BEGIN SELECT RAISE(ABORT, 'FORGE_COORDINATION_REVIEW_DECISION_STALE'); END;

CREATE TABLE review_rework_round (
    report_artifact_id  TEXT PRIMARY KEY
                            REFERENCES review_range_report(report_artifact_id) ON DELETE RESTRICT,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    round               INTEGER NOT NULL CHECK (round BETWEEN 1 AND 8),
    request_id          TEXT REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    created_by_kind     TEXT NOT NULL CHECK (created_by_kind IN ('human', 'agent')),
    created_by_id       TEXT NOT NULL,
    idempotency_key     TEXT NOT NULL CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    created_at          TEXT NOT NULL,
    UNIQUE (task_id, idempotency_key)
);

CREATE TABLE review_correction_link (
    correction_work_unit_id  TEXT NOT NULL,
    finding_id               TEXT NOT NULL REFERENCES review_finding(id) ON DELETE RESTRICT,
    task_id                  TEXT NOT NULL,
    repo_id                  TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    report_artifact_id       TEXT NOT NULL
                                 REFERENCES review_rework_round(report_artifact_id) ON DELETE RESTRICT,
    base_commit              TEXT NOT NULL CHECK (length(trim(base_commit)) > 0),
    created_at               TEXT NOT NULL,
    PRIMARY KEY (correction_work_unit_id, finding_id),
    FOREIGN KEY (correction_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT
);
CREATE INDEX idx_review_correction_link_finding ON review_correction_link(finding_id);

CREATE TABLE task_review_coverage_state (
    task_id     TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    repo_id     TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    revision    INTEGER NOT NULL CHECK (revision >= 1),
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id)
);
CREATE TRIGGER task_review_coverage_state_monotonic
BEFORE UPDATE ON task_review_coverage_state
WHEN NEW.task_id IS NOT OLD.task_id OR NEW.repo_id IS NOT OLD.repo_id
  OR NEW.revision != OLD.revision + 1
BEGIN SELECT RAISE(ABORT, 'Review coverage revision advances exactly once'); END;

-- Coverage revision: every durable fact that can change coverage advances it,
-- only while the Task has a current review policy.
CREATE TRIGGER review_coverage_bump_policy_insert
AFTER INSERT ON task_review_policy_revision
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT v.task_id, m.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    FROM task_repository_set_revision v
    JOIN task_repository_set_revision_repo m ON m.task_id = v.task_id AND m.revision = v.revision
    WHERE v.task_id = NEW.task_id AND v.state = 'current' AND m.role IN ('primary', 'affected')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

CREATE TRIGGER review_coverage_bump_integration_insert
AFTER INSERT ON work_unit_repository_integration
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT NEW.task_id, NEW.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE EXISTS (SELECT 1 FROM task_review_policy_revision p
                  WHERE p.task_id = NEW.task_id AND p.state = 'current')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

CREATE TRIGGER review_coverage_bump_integration_update
AFTER UPDATE OF outcome, target_after_sha ON work_unit_repository_integration
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT NEW.task_id, NEW.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE EXISTS (SELECT 1 FROM task_review_policy_revision p
                  WHERE p.task_id = NEW.task_id AND p.state = 'current')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

CREATE TRIGGER review_coverage_bump_checkpoint_insert
AFTER INSERT ON review_checkpoint
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT NEW.task_id, NEW.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE EXISTS (SELECT 1 FROM task_review_policy_revision p
                  WHERE p.task_id = NEW.task_id AND p.state = 'current')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

CREATE TRIGGER review_coverage_bump_checkpoint_update
AFTER UPDATE ON review_checkpoint
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT NEW.task_id, NEW.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE EXISTS (SELECT 1 FROM task_review_policy_revision p
                  WHERE p.task_id = NEW.task_id AND p.state = 'current')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

CREATE TRIGGER review_coverage_bump_report_insert
AFTER INSERT ON review_range_report
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT NEW.task_id, NEW.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE EXISTS (SELECT 1 FROM task_review_policy_revision p
                  WHERE p.task_id = NEW.task_id AND p.state = 'current')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

CREATE TRIGGER review_coverage_bump_resolution_insert
AFTER INSERT ON review_finding_resolution
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT NEW.task_id, NEW.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE EXISTS (SELECT 1 FROM task_review_policy_revision p
                  WHERE p.task_id = NEW.task_id AND p.state = 'current')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

CREATE TRIGGER review_coverage_bump_base_insert
AFTER INSERT ON task_review_base
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT NEW.task_id, NEW.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE EXISTS (SELECT 1 FROM task_review_policy_revision p
                  WHERE p.task_id = NEW.task_id AND p.state = 'current')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

CREATE TRIGGER review_coverage_bump_attribution_insert
AFTER INSERT ON review_commit_attribution
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT NEW.task_id, NEW.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    WHERE EXISTS (SELECT 1 FROM task_review_policy_revision p
                  WHERE p.task_id = NEW.task_id AND p.state = 'current')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

CREATE TRIGGER review_coverage_bump_result_line
AFTER UPDATE ON work_unit_review_line
BEGIN
    INSERT INTO task_review_coverage_state(task_id, repo_id, revision, updated_at)
    SELECT v.task_id, m.repo_id, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')
    FROM task_repository_set_revision v
    JOIN task_repository_set_revision_repo m ON m.task_id = v.task_id AND m.revision = v.revision
    WHERE v.task_id = NEW.task_id AND v.state = 'current' AND m.role IN ('primary', 'affected')
      AND EXISTS (SELECT 1 FROM task_review_policy_revision p
                  WHERE p.task_id = NEW.task_id AND p.state = 'current')
    ON CONFLICT(task_id, repo_id) DO UPDATE
        SET revision = revision + 1, updated_at = excluded.updated_at;
END;

-- Immutability. History is removed only by Project teardown.
CREATE TRIGGER task_review_base_immutable_update BEFORE UPDATE ON task_review_base
BEGIN SELECT RAISE(ABORT, 'task_review_base is immutable'); END;
CREATE TRIGGER task_review_base_immutable_delete BEFORE DELETE ON task_review_base
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'task_review_base is preserved outside Project teardown'); END;

CREATE TRIGGER review_commit_attribution_immutable_update BEFORE UPDATE ON review_commit_attribution
BEGIN SELECT RAISE(ABORT, 'review_commit_attribution is immutable'); END;
CREATE TRIGGER review_commit_attribution_immutable_delete BEFORE DELETE ON review_commit_attribution
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_commit_attribution is preserved outside Project teardown'); END;

CREATE TRIGGER review_range_immutable_update BEFORE UPDATE ON review_range
BEGIN SELECT RAISE(ABORT, 'review_range is immutable'); END;
CREATE TRIGGER review_range_immutable_delete BEFORE DELETE ON review_range
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_range is preserved outside Project teardown'); END;

CREATE TRIGGER review_checkpoint_request_immutable_update BEFORE UPDATE ON review_checkpoint_request
BEGIN SELECT RAISE(ABORT, 'review_checkpoint_request is immutable'); END;
CREATE TRIGGER review_checkpoint_request_immutable_delete BEFORE DELETE ON review_checkpoint_request
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_checkpoint_request is preserved outside Project teardown'); END;

CREATE TRIGGER review_range_contributor_immutable_update BEFORE UPDATE ON review_range_contributor
BEGIN SELECT RAISE(ABORT, 'review_range_contributor is immutable'); END;
CREATE TRIGGER review_range_contributor_immutable_delete BEFORE DELETE ON review_range_contributor
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_range_contributor is preserved outside Project teardown'); END;

CREATE TRIGGER work_unit_review_range_immutable_update BEFORE UPDATE ON work_unit_review_range
BEGIN SELECT RAISE(ABORT, 'work_unit_review_range is immutable'); END;
CREATE TRIGGER work_unit_review_range_immutable_delete BEFORE DELETE ON work_unit_review_range
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'work_unit_review_range is preserved outside Project teardown'); END;

CREATE TRIGGER review_range_report_immutable_update BEFORE UPDATE ON review_range_report
BEGIN SELECT RAISE(ABORT, 'review_range_report is immutable'); END;
CREATE TRIGGER review_range_report_immutable_delete BEFORE DELETE ON review_range_report
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_range_report is preserved outside Project teardown'); END;

CREATE TRIGGER review_finding_immutable_update BEFORE UPDATE ON review_finding
BEGIN SELECT RAISE(ABORT, 'review_finding is immutable'); END;
CREATE TRIGGER review_finding_immutable_delete BEFORE DELETE ON review_finding
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_finding is preserved outside Project teardown'); END;

CREATE TRIGGER review_finding_location_immutable_update BEFORE UPDATE ON review_finding_location
BEGIN SELECT RAISE(ABORT, 'review_finding_location is immutable'); END;
CREATE TRIGGER review_finding_location_immutable_delete BEFORE DELETE ON review_finding_location
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_finding_location is preserved outside Project teardown'); END;

CREATE TRIGGER review_finding_resolution_immutable_update BEFORE UPDATE ON review_finding_resolution
BEGIN SELECT RAISE(ABORT, 'review_finding_resolution is immutable'); END;
CREATE TRIGGER review_finding_resolution_immutable_delete BEFORE DELETE ON review_finding_resolution
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_finding_resolution is preserved outside Project teardown'); END;

CREATE TRIGGER review_finding_triage_immutable_update BEFORE UPDATE ON review_finding_triage
BEGIN SELECT RAISE(ABORT, 'review_finding_triage is immutable'); END;
CREATE TRIGGER review_finding_triage_immutable_delete BEFORE DELETE ON review_finding_triage
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_finding_triage is preserved outside Project teardown'); END;

CREATE TRIGGER review_rework_round_immutable_update BEFORE UPDATE ON review_rework_round
BEGIN SELECT RAISE(ABORT, 'review_rework_round is immutable'); END;
CREATE TRIGGER review_rework_round_immutable_delete BEFORE DELETE ON review_rework_round
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_rework_round is preserved outside Project teardown'); END;

CREATE TRIGGER review_correction_link_immutable_update BEFORE UPDATE ON review_correction_link
BEGIN SELECT RAISE(ABORT, 'review_correction_link is immutable'); END;
CREATE TRIGGER review_correction_link_immutable_delete BEFORE DELETE ON review_correction_link
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_correction_link is preserved outside Project teardown'); END;

CREATE TRIGGER task_review_policy_revision_immutable_delete BEFORE DELETE ON task_review_policy_revision
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'task_review_policy_revision is preserved outside Project teardown'); END;

CREATE TRIGGER review_checkpoint_immutable_delete BEFORE DELETE ON review_checkpoint
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_checkpoint is preserved outside Project teardown'); END;

CREATE TRIGGER review_plan_checkpoint_immutable_delete BEFORE DELETE ON review_plan_checkpoint
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'review_plan_checkpoint is preserved outside Project teardown'); END;

CREATE TRIGGER task_review_coverage_state_immutable_delete BEFORE DELETE ON task_review_coverage_state
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
                 WHERE t.id = OLD.task_id)
BEGIN SELECT RAISE(ABORT, 'task_review_coverage_state is preserved outside Project teardown'); END;

CREATE TRIGGER project_review_policy_immutable_delete BEFORE DELETE ON project_review_policy
WHEN NOT EXISTS (SELECT 1 FROM project_deletion_guard g WHERE g.project_id = OLD.project_id)
BEGIN SELECT RAISE(ABORT, 'project_review_policy is preserved outside Project teardown'); END;
