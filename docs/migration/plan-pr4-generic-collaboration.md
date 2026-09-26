# Plan PR4: generic collaboration primitives

## Purpose and authority

PR4 adds a small, generic collaboration island: Artifact, Message, Handoff,
Proposal, and Decision. Each is authoritative for data created through the new
generic service/API. Existing vertical records remain authoritative for their
own data until the assigned migration. There is no automatic legacy
projection, compatibility layer, or dual-write.

The legacy-to-PR mapping remains explicit:

| Legacy authority | Migration |
|---|---|
| `task_plan_revision`, `task_plan_approval` | PR7 |
| `review`, `review_evidence_bundle`, `task_decision_request`, `task_decision_answer` | PR8; related Task/gate work is PR9 |
| Agent Chat, `agent_handoff`, commitments, inbox, questions, actions, Project documents and Project decisions/baselines/milestones/releases, memory proposal/decision authority, and legacy rooms | PR11, with relevant task/gate work in PR9 |
| Physical removal of migrated legacy schema and cleanup, including legacy rooms | PR13 |

The PR4 writers do not write any of these tables. In particular, a legacy plan,
chat message, handoff, or Project Decision does not create a generic record.

## Physical model and invariants

Migration `V090__generic_collaboration_primitives.sql` is additive and leaves
legacy tables intact. It creates `artifact`,
`artifact_execution_producer`, `message`, `message_artifact`, `handoff`,
`handoff_artifact`, `proposal`, `proposal_artifact`, `decision`, and
`decision_actor`. Task scope and cross-Task Artifact relationships are guarded
by composite keys/foreign keys and service validation.

Artifact storage is exactly one of `inline` content or an `external`
`content_ref`, enforced by a database CHECK. Artifact has no producer actor
column. PR4 supports exactly one Execution producer through
`artifact_execution_producer`; its Human/Agent ActorRef is derived through
Execution. Reads fail closed for missing, dangling, or cross-Task producers.
PR8 can add a separate `artifact_validation_run_producer` relation once
ValidationRun exists. PR4 deliberately creates no FK to that future table.

ActorRef uses the PR1 Human/Agent identity. DB triggers check referenced Users
and Agent identities, reject `System`, and validate Role/Execution scope.
Authenticated HTTP requests derive Human identity from `AuthenticatedUser`;
the request DTOs contain no sender/proposer/creator/decider identity fields.
Trusted service callers derive Agent identity from a same-Task persisted
Execution and its active RoleMembership. Agent Handoffs do not change
RoleMembership.

Message is immutable communication to Actor, Role, or Task. Handoff keeps
creator, source Role, and target separate; only its controlled status/version/
timestamps change. Proposal supports Task, Execution, and Workspace targets in
PR4; `work_unit` is unsupported until PR5. Its content is immutable at
`content_version = 1`, and material revisions are new linked Proposals.
Decision is immutable, requires at least one Human/Agent decider, supports
multiple mixed deciders, and records `approve`, `reject`, or `supersede`.
Supersede is followed by a new Proposal. Decision never executes action.

Policy references and snapshots are opaque evidence. They are not a DSL,
permission, authorization result, or bypass; deterministic consumers decide
what is allowed. Message delivery/read state, automatic dispatch, consensus,
WorkUnit/Gate/ValidationRun links, and action execution are not in PR4.

## Events and transactions

PR4 reuses `domain_event` as the durable event ledger. Each persisted record
and its creation/lifecycle event commit in the same SQLite transaction.
EventBus notifications are post-commit only. Event payloads carry bounded IDs,
kinds, action/outcome/status/version, and safe digests; they exclude Artifact
content and `content_ref`, Message body, Proposal reason, Decision rationale,
filesystem paths, workspace handles, prompts, credentials, and authorization
material.

## API and authorization

The REST surface stays under `/api/v1`; it provides create/list/detail for
Artifact and Message, create/list/detail/transition for Handoff,
create/list/detail/withdraw for Proposal, and create/list/detail for Decision.
Lists use stable opaque keyset cursors. Artifact list responses omit inline
content; no response includes `content_ref`.

Generic Decision list/create uses
`/api/v1/tasks/{task_id}/collaboration/decisions`: the existing
`/api/v1/tasks/{id}/decisions` GET route is owned by legacy task-decision
requests, and Axum cannot register both parameterized patterns independently.
The scoped collaboration segment keeps the legacy route and its authority
unchanged.

Every operation resolves record to Task to Project before authorization. Reads
by ID do not disclose content or existence to an unauthorized user. Message,
Handoff, Proposal, and Decision grant no membership or privilege. HTTP writes
derive the Human from auth; the Agent service path accepts only a persisted
Execution context for reads and writes and checks its Task, ActorRef, and
active membership.

## Project teardown

The guarded `ProjectRepo::delete` flow removes PR4 relationship rows and
immutable base rows in FK order while `project_deletion_guard` is present.
Immutability delete triggers permit only that guarded teardown. The durable
`domain_event` ledger is not a Project-owned FK cascade and remains historical
evidence under its existing lifecycle. No global foreign-key setting is
changed.

## Deferred to later Plan PRs

* PR5: WorkUnit, DAG, dependency and schedule targets/relations.
* PR6: orchestration engine, dispatch, wake and worker scheduling.
* PR7: Planning migration; no automatic `task_plan_revision` conversion.
* PR8: ValidationRun, review migration and additive ValidationRun Artifact producer.
* PR9: final Task aggregate and Gate engine.
* PR10: embedded runtime removal.
* PR11: Agent Host/Project OS migration; legacy records stay authoritative until then.
* PR12: global naming cleanup, including legacy `ArtifactRef`.
* PR13: physical legacy cleanup.
* PR14–15: broad release/reliability and final CI/conformance work.
