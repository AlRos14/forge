-- Plan PR4 adds a generic collaboration island. Legacy planning, review,
-- Agent Chat, handoff, and Project OS records remain independent authorities.

CREATE UNIQUE INDEX idx_execution_id_task
    ON execution(id, task_id);

CREATE TABLE artifact (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    kind            TEXT NOT NULL CHECK (length(trim(kind)) > 0),
    storage_kind    TEXT NOT NULL CHECK (storage_kind IN ('inline', 'external')),
    content         TEXT,
    content_ref     TEXT,
    metadata_json   TEXT NOT NULL DEFAULT '{}'
                        CHECK (json_valid(metadata_json) AND json_type(metadata_json) = 'object'),
    digest          TEXT,
    created_at      TEXT NOT NULL,
    UNIQUE(id, task_id),
    CHECK (
        (storage_kind = 'inline' AND content IS NOT NULL AND content_ref IS NULL)
        OR
        (storage_kind = 'external' AND content IS NULL
         AND content_ref IS NOT NULL AND length(trim(content_ref)) > 0)
    )
);
CREATE INDEX idx_artifact_task_created
    ON artifact(task_id, created_at DESC, id DESC);

CREATE TABLE artifact_execution_producer (
    artifact_id     TEXT NOT NULL,
    execution_id    TEXT NOT NULL,
    task_id         TEXT NOT NULL,
    PRIMARY KEY (artifact_id),
    FOREIGN KEY (artifact_id, task_id)
        REFERENCES artifact(id, task_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT
);
CREATE INDEX idx_artifact_execution_producer_execution
    ON artifact_execution_producer(execution_id);

CREATE TABLE message (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    sender_actor_kind   TEXT NOT NULL CHECK (sender_actor_kind IN ('human', 'agent')),
    sender_actor_id     TEXT NOT NULL CHECK (length(trim(sender_actor_id)) > 0),
    target_kind         TEXT NOT NULL CHECK (target_kind IN ('actor', 'role', 'task')),
    target_actor_kind   TEXT CHECK (target_actor_kind IN ('human', 'agent')),
    target_actor_id     TEXT,
    target_role_id      TEXT REFERENCES task_role(id) ON DELETE RESTRICT,
    body                TEXT NOT NULL CHECK (length(trim(body)) > 0),
    created_at          TEXT NOT NULL,
    UNIQUE(id, task_id),
    CHECK (
        (target_kind = 'actor'
         AND target_actor_kind IS NOT NULL
         AND target_actor_id IS NOT NULL
         AND length(trim(target_actor_id)) > 0
         AND target_role_id IS NULL)
        OR
        (target_kind = 'role'
         AND target_actor_kind IS NULL
         AND target_actor_id IS NULL
         AND target_role_id IS NOT NULL)
        OR
        (target_kind = 'task'
         AND target_actor_kind IS NULL
         AND target_actor_id IS NULL
         AND target_role_id IS NULL)
    )
);
CREATE INDEX idx_message_task_created
    ON message(task_id, created_at DESC, id DESC);
CREATE INDEX idx_message_sender
    ON message(sender_actor_kind, sender_actor_id, created_at DESC, id DESC);

CREATE TABLE message_artifact (
    message_id      TEXT NOT NULL,
    artifact_id     TEXT NOT NULL,
    task_id         TEXT NOT NULL,
    PRIMARY KEY (message_id, artifact_id),
    FOREIGN KEY (message_id, task_id)
        REFERENCES message(id, task_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (artifact_id, task_id)
        REFERENCES artifact(id, task_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX idx_message_artifact_task
    ON message_artifact(task_id, message_id);

CREATE TABLE handoff (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    created_by_actor_kind   TEXT NOT NULL CHECK (created_by_actor_kind IN ('human', 'agent')),
    created_by_actor_id     TEXT NOT NULL CHECK (length(trim(created_by_actor_id)) > 0),
    source_role_id          TEXT REFERENCES task_role(id) ON DELETE RESTRICT,
    target_kind             TEXT NOT NULL CHECK (target_kind IN ('actor', 'role', 'task')),
    target_actor_kind       TEXT CHECK (target_actor_kind IN ('human', 'agent')),
    target_actor_id         TEXT,
    target_role_id          TEXT REFERENCES task_role(id) ON DELETE RESTRICT,
    intent                  TEXT NOT NULL CHECK (
                                intent IN ('rework', 'delegation', 'question', 'answer',
                                           'investigate', 'decision_request')
                            ),
    parent_execution_id     TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    expected_policy_ref     TEXT,
    status                  TEXT NOT NULL CHECK (
                                status IN ('pending', 'accepted', 'completed', 'declined', 'cancelled')
                            ),
    version                 INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    UNIQUE(id, task_id),
    CHECK (
        (target_kind = 'actor'
         AND target_actor_kind IS NOT NULL
         AND target_actor_id IS NOT NULL
         AND length(trim(target_actor_id)) > 0
         AND target_role_id IS NULL)
        OR
        (target_kind = 'role'
         AND target_actor_kind IS NULL
         AND target_actor_id IS NULL
         AND target_role_id IS NOT NULL)
        OR
        (target_kind = 'task'
         AND target_actor_kind IS NULL
         AND target_actor_id IS NULL
         AND target_role_id IS NULL)
    )
);
CREATE INDEX idx_handoff_task_created
    ON handoff(task_id, created_at DESC, id DESC);
CREATE INDEX idx_handoff_target_actor
    ON handoff(target_actor_kind, target_actor_id, created_at DESC, id DESC);

CREATE TABLE handoff_artifact (
    handoff_id      TEXT NOT NULL,
    artifact_id     TEXT NOT NULL,
    task_id         TEXT NOT NULL,
    PRIMARY KEY (handoff_id, artifact_id),
    FOREIGN KEY (handoff_id, task_id)
        REFERENCES handoff(id, task_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (artifact_id, task_id)
        REFERENCES artifact(id, task_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX idx_handoff_artifact_task
    ON handoff_artifact(task_id, handoff_id);

CREATE TABLE proposal (
    id                          TEXT PRIMARY KEY,
    task_id                     TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    proposer_actor_kind         TEXT NOT NULL CHECK (proposer_actor_kind IN ('human', 'agent')),
    proposer_actor_id           TEXT NOT NULL CHECK (length(trim(proposer_actor_id)) > 0),
    target_kind                 TEXT NOT NULL CHECK (target_kind IN ('task', 'execution', 'workspace')),
    target_id                   TEXT NOT NULL CHECK (length(trim(target_id)) > 0),
    action                      TEXT NOT NULL CHECK (
                                    length(trim(action)) BETWEEN 1 AND 128
                                ),
    reason                      TEXT NOT NULL,
    target_version              INTEGER,
    target_digest               TEXT,
    required_policy_ref         TEXT,
    required_policy_version     INTEGER,
    required_policy_digest      TEXT,
    content_version             INTEGER NOT NULL DEFAULT 1 CHECK (content_version = 1),
    status                      TEXT NOT NULL CHECK (
                                    status IN ('open', 'resolved', 'withdrawn', 'superseded')
                                ),
    supersedes_proposal_id      TEXT REFERENCES proposal(id) ON DELETE NO ACTION
                                    DEFERRABLE INITIALLY DEFERRED,
    created_at                  TEXT NOT NULL,
    UNIQUE(id, task_id)
);
CREATE INDEX idx_proposal_task_created
    ON proposal(task_id, created_at DESC, id DESC);
CREATE INDEX idx_proposal_target
    ON proposal(target_kind, target_id, created_at DESC, id DESC);

CREATE TABLE proposal_artifact (
    proposal_id     TEXT NOT NULL,
    artifact_id     TEXT NOT NULL,
    task_id         TEXT NOT NULL,
    PRIMARY KEY (proposal_id, artifact_id),
    FOREIGN KEY (proposal_id, task_id)
        REFERENCES proposal(id, task_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (artifact_id, task_id)
        REFERENCES artifact(id, task_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX idx_proposal_artifact_task
    ON proposal_artifact(task_id, proposal_id);

CREATE TABLE decision (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    proposal_id         TEXT NOT NULL REFERENCES proposal(id) ON DELETE RESTRICT,
    proposal_version    INTEGER NOT NULL CHECK (proposal_version >= 1),
    outcome             TEXT NOT NULL CHECK (outcome IN ('approve', 'reject', 'supersede')),
    rationale           TEXT NOT NULL,
    policy_ref          TEXT,
    policy_version      INTEGER,
    policy_digest       TEXT,
    created_at          TEXT NOT NULL,
    UNIQUE(proposal_id),
    UNIQUE(id, task_id)
);
CREATE INDEX idx_decision_task_created
    ON decision(task_id, created_at DESC, id DESC);

CREATE TABLE decision_actor (
    decision_id     TEXT NOT NULL,
    actor_kind      TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id        TEXT NOT NULL CHECK (length(trim(actor_id)) > 0),
    task_id         TEXT NOT NULL,
    PRIMARY KEY (decision_id, actor_kind, actor_id),
    FOREIGN KEY (decision_id, task_id)
        REFERENCES decision(id, task_id) ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX idx_decision_actor_identity
    ON decision_actor(actor_kind, actor_id, decision_id);

-- ActorRef polymorphism is guarded at the database boundary. System is not a
-- valid participant in these generic primitives.
CREATE TRIGGER artifact_execution_producer_actor_guard
BEFORE INSERT ON artifact_execution_producer
WHEN NOT EXISTS (
    SELECT 1
    FROM execution e
    WHERE e.id = NEW.execution_id
      AND e.task_id = NEW.task_id
      AND e.actor_kind IN ('human', 'agent')
      AND e.actor_id IS NOT NULL
      AND ((e.actor_kind = 'human' AND EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
        OR (e.actor_kind = 'agent' AND EXISTS (
                SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id
            )))
)
BEGIN
    SELECT RAISE(ABORT, 'Artifact Execution producer has no valid ActorRef');
END;

CREATE TRIGGER artifact_execution_producer_insert_order_guard
BEFORE INSERT ON artifact_execution_producer
WHEN EXISTS (SELECT 1 FROM artifact a WHERE a.id = NEW.artifact_id)
BEGIN
    SELECT RAISE(ABORT, 'Artifact producer is immutable after Artifact creation');
END;

CREATE TRIGGER artifact_producer_required_insert
BEFORE INSERT ON artifact
WHEN NOT EXISTS (
    SELECT 1 FROM artifact_execution_producer p
    JOIN execution e ON e.id = p.execution_id AND e.task_id = p.task_id
    WHERE p.artifact_id = NEW.id
      AND p.task_id = NEW.task_id
      AND e.actor_kind IN ('human', 'agent')
)
BEGIN
    SELECT RAISE(ABORT, 'Artifact requires one valid Execution producer');
END;

CREATE TRIGGER artifact_immutable_update
BEFORE UPDATE ON artifact
BEGIN
    SELECT RAISE(ABORT, 'Artifacts are immutable');
END;

CREATE TRIGGER artifact_immutable_delete
BEFORE DELETE ON artifact
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Artifacts are immutable outside Project teardown');
END;

CREATE TRIGGER artifact_execution_producer_immutable_update
BEFORE UPDATE ON artifact_execution_producer
BEGIN
    SELECT RAISE(ABORT, 'Artifact producers are immutable');
END;

CREATE TRIGGER artifact_execution_producer_immutable_delete
BEFORE DELETE ON artifact_execution_producer
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Artifact producers are immutable outside Project teardown');
END;

CREATE TRIGGER message_actor_guard_insert
BEFORE INSERT ON message
WHEN (NEW.sender_actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user u WHERE u.id = NEW.sender_actor_id
      ))
  OR (NEW.sender_actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.sender_actor_id
      ))
  OR (NEW.target_kind = 'actor' AND NEW.target_actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user u WHERE u.id = NEW.target_actor_id
      ))
  OR (NEW.target_kind = 'actor' AND NEW.target_actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.target_actor_id
      ))
  OR (NEW.target_kind = 'role' AND NOT EXISTS (
          SELECT 1 FROM task_role tr
          WHERE tr.id = NEW.target_role_id AND tr.task_id = NEW.task_id
      ))
BEGIN
    SELECT RAISE(ABORT, 'Message ActorRef or target is invalid');
END;

CREATE TRIGGER message_artifact_insert_order_guard
BEFORE INSERT ON message_artifact
WHEN EXISTS (SELECT 1 FROM message m WHERE m.id = NEW.message_id)
BEGIN
    SELECT RAISE(ABORT, 'Message Artifact relationships are immutable after creation');
END;

CREATE TRIGGER message_artifact_immutable_update
BEFORE UPDATE ON message_artifact
BEGIN
    SELECT RAISE(ABORT, 'Message Artifact relationships are immutable');
END;

CREATE TRIGGER message_artifact_immutable_delete
BEFORE DELETE ON message_artifact
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Message Artifact relationships are immutable outside Project teardown');
END;

CREATE TRIGGER message_immutable_update
BEFORE UPDATE ON message
BEGIN
    SELECT RAISE(ABORT, 'Messages are immutable');
END;

CREATE TRIGGER message_immutable_delete
BEFORE DELETE ON message
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Messages are immutable outside Project teardown');
END;

CREATE TRIGGER handoff_actor_target_guard_insert
BEFORE INSERT ON handoff
WHEN (NEW.created_by_actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user u WHERE u.id = NEW.created_by_actor_id
      ))
  OR (NEW.created_by_actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.created_by_actor_id
      ))
  OR (NEW.target_kind = 'actor' AND NEW.target_actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user u WHERE u.id = NEW.target_actor_id
      ))
  OR (NEW.target_kind = 'actor' AND NEW.target_actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.target_actor_id
      ))
  OR (NEW.target_kind = 'role' AND NOT EXISTS (
          SELECT 1 FROM task_role tr
          WHERE tr.id = NEW.target_role_id AND tr.task_id = NEW.task_id
      ))
  OR (NEW.source_role_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM task_role tr
          WHERE tr.id = NEW.source_role_id AND tr.task_id = NEW.task_id
      ))
  OR (NEW.parent_execution_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM execution e
          WHERE e.id = NEW.parent_execution_id AND e.task_id = NEW.task_id
      ))
BEGIN
    SELECT RAISE(ABORT, 'Handoff ActorRef, role, or Execution scope is invalid');
END;

CREATE TRIGGER handoff_immutable_update_guard
BEFORE UPDATE ON handoff
WHEN NEW.id IS NOT OLD.id
  OR NEW.task_id IS NOT OLD.task_id
  OR NEW.created_by_actor_kind IS NOT OLD.created_by_actor_kind
  OR NEW.created_by_actor_id IS NOT OLD.created_by_actor_id
  OR NEW.source_role_id IS NOT OLD.source_role_id
  OR NEW.target_kind IS NOT OLD.target_kind
  OR NEW.target_actor_kind IS NOT OLD.target_actor_kind
  OR NEW.target_actor_id IS NOT OLD.target_actor_id
  OR NEW.target_role_id IS NOT OLD.target_role_id
  OR NEW.intent IS NOT OLD.intent
  OR NEW.parent_execution_id IS NOT OLD.parent_execution_id
  OR NEW.expected_policy_ref IS NOT OLD.expected_policy_ref
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.version != OLD.version + 1
  OR NEW.status = OLD.status
  OR NOT (
       (OLD.status = 'pending' AND NEW.status IN ('accepted', 'declined', 'cancelled'))
       OR (OLD.status = 'accepted' AND NEW.status IN ('completed', 'cancelled'))
  )
BEGIN
    SELECT RAISE(ABORT, 'Handoff lifecycle update is invalid');
END;

CREATE TRIGGER handoff_artifact_insert_order_guard
BEFORE INSERT ON handoff_artifact
WHEN EXISTS (SELECT 1 FROM handoff h WHERE h.id = NEW.handoff_id)
BEGIN
    SELECT RAISE(ABORT, 'Handoff Artifact relationships are immutable after creation');
END;

CREATE TRIGGER handoff_artifact_immutable_update
BEFORE UPDATE ON handoff_artifact
BEGIN
    SELECT RAISE(ABORT, 'Handoff Artifact relationships are immutable');
END;

CREATE TRIGGER handoff_artifact_immutable_delete
BEFORE DELETE ON handoff_artifact
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Handoff Artifact relationships are immutable outside Project teardown');
END;

CREATE TRIGGER handoff_immutable_delete
BEFORE DELETE ON handoff
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Handoffs are immutable outside Project teardown');
END;

CREATE TRIGGER proposal_actor_target_guard_insert
BEFORE INSERT ON proposal
WHEN (NEW.proposer_actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user u WHERE u.id = NEW.proposer_actor_id
      ))
  OR (NEW.proposer_actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.proposer_actor_id
      ))
  OR (NEW.target_kind = 'task' AND NEW.target_id != NEW.task_id)
  OR (NEW.target_kind = 'execution' AND NOT EXISTS (
          SELECT 1 FROM execution e
          WHERE e.id = NEW.target_id AND e.task_id = NEW.task_id
      ))
  OR (NEW.target_kind = 'workspace' AND NOT EXISTS (
          SELECT 1 FROM workspace w
          WHERE w.id = NEW.target_id AND w.task_id = NEW.task_id
      ))
  OR (NEW.supersedes_proposal_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM proposal p
          WHERE p.id = NEW.supersedes_proposal_id
            AND p.task_id = NEW.task_id
            AND p.status = 'superseded'
      ))
BEGIN
    SELECT RAISE(ABORT, 'Proposal ActorRef, target, or supersedes scope is invalid');
END;

CREATE TRIGGER proposal_artifact_insert_order_guard
BEFORE INSERT ON proposal_artifact
WHEN EXISTS (SELECT 1 FROM proposal p WHERE p.id = NEW.proposal_id)
BEGIN
    SELECT RAISE(ABORT, 'Proposal Artifact relationships are immutable after creation');
END;

CREATE TRIGGER proposal_artifact_immutable_update
BEFORE UPDATE ON proposal_artifact
BEGIN
    SELECT RAISE(ABORT, 'Proposal Artifact relationships are immutable');
END;

CREATE TRIGGER proposal_artifact_immutable_delete
BEFORE DELETE ON proposal_artifact
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Proposal Artifact relationships are immutable outside Project teardown');
END;

CREATE TRIGGER proposal_content_immutable_update
BEFORE UPDATE ON proposal
WHEN NEW.id IS NOT OLD.id
  OR NEW.task_id IS NOT OLD.task_id
  OR NEW.proposer_actor_kind IS NOT OLD.proposer_actor_kind
  OR NEW.proposer_actor_id IS NOT OLD.proposer_actor_id
  OR NEW.target_kind IS NOT OLD.target_kind
  OR NEW.target_id IS NOT OLD.target_id
  OR NEW.action IS NOT OLD.action
  OR NEW.reason IS NOT OLD.reason
  OR NEW.target_version IS NOT OLD.target_version
  OR NEW.target_digest IS NOT OLD.target_digest
  OR NEW.required_policy_ref IS NOT OLD.required_policy_ref
  OR NEW.required_policy_version IS NOT OLD.required_policy_version
  OR NEW.required_policy_digest IS NOT OLD.required_policy_digest
  OR NEW.content_version IS NOT OLD.content_version
  OR NEW.supersedes_proposal_id IS NOT OLD.supersedes_proposal_id
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.status = OLD.status
  OR OLD.status != 'open'
  OR NEW.status NOT IN ('resolved', 'withdrawn', 'superseded')
BEGIN
    SELECT RAISE(ABORT, 'Proposal content is immutable or lifecycle transition is invalid');
END;

CREATE TRIGGER proposal_lifecycle_evidence_guard
BEFORE UPDATE OF status ON proposal
WHEN (NEW.status = 'resolved' AND NOT EXISTS (
          SELECT 1 FROM decision d
          WHERE d.proposal_id = OLD.id AND d.outcome IN ('approve', 'reject')
      ))
  OR (NEW.status = 'superseded' AND NOT EXISTS (
          SELECT 1 FROM decision d
          WHERE d.proposal_id = OLD.id AND d.outcome = 'supersede'
      ))
  OR (NEW.status = 'withdrawn' AND EXISTS (
          SELECT 1 FROM decision d WHERE d.proposal_id = OLD.id
      ))
BEGIN
    SELECT RAISE(ABORT, 'Proposal status requires matching Decision evidence');
END;

CREATE TRIGGER proposal_immutable_delete
BEFORE DELETE ON proposal
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Proposals are immutable outside Project teardown');
END;

CREATE TRIGGER decision_actor_insert_order_guard
BEFORE INSERT ON decision_actor
WHEN EXISTS (SELECT 1 FROM decision d WHERE d.id = NEW.decision_id)
  OR (NEW.actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user u WHERE u.id = NEW.actor_id
      ))
  OR (NEW.actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.actor_id
      ))
BEGIN
    SELECT RAISE(ABORT, 'Decision actors are immutable or ActorRef is invalid');
END;

CREATE TRIGGER decision_actor_immutable_update
BEFORE UPDATE ON decision_actor
BEGIN
    SELECT RAISE(ABORT, 'Decision actors are immutable');
END;

CREATE TRIGGER decision_actor_immutable_delete
BEFORE DELETE ON decision_actor
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Decision actors are immutable outside Project teardown');
END;

CREATE TRIGGER decision_proposal_guard_insert
BEFORE INSERT ON decision
WHEN NOT EXISTS (
    SELECT 1 FROM proposal p
    WHERE p.id = NEW.proposal_id
      AND p.task_id = NEW.task_id
      AND p.content_version = NEW.proposal_version
      AND p.status = 'open'
)
  OR NOT EXISTS (
    SELECT 1 FROM decision_actor da
    WHERE da.decision_id = NEW.id AND da.task_id = NEW.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Decision requires an open same-Task Proposal and at least one decider');
END;

CREATE TRIGGER decision_immutable_update
BEFORE UPDATE ON decision
BEGIN
    SELECT RAISE(ABORT, 'Decisions are immutable');
END;

CREATE TRIGGER decision_immutable_delete
BEFORE DELETE ON decision
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Decisions are immutable outside Project teardown');
END;
