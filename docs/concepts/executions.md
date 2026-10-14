# Executions

An Execution is the atomic historical work record:

~~~text
Execution
  id
  task_id
  actor_ref
  role
  purpose
  work_unit_id?
  harness_session_id?
  workspace_id?
  parent_execution_id?
  status
  configuration_snapshot
  capabilities_snapshot
  usage_snapshot?
  outputs
  started_at
  finished_at?
~~~

It represents exactly one Actor acting under exactly one Role for one Purpose.
The role is historical context, not an Actor subtype.

## Plan PR2 persistence

The current persistence stores `actor_kind` and `actor_id` as the physical
form of `actor_ref`, plus one of these exact purposes:
`plan`, `implement`, `review`, `validate`, `investigate`, `orchestrate`, or
`general`. Purpose is supplied by the semantic creation path; it is not a
permanent alias for a Role or for the transitional executor
`permission_policy: plan` value.

`actor_kind` and `actor_id` are the canonical ActorRef. `execution.agent_id`
remains an Agent-only lookup projection for Agent execution statistics;
`execution.agent_session_id` was removed by V124 after exact provenance capture.
A new Human Execution stores the real user id, has no Agent projection, and
has no HarnessSession.

## Purpose and permission

Initial purposes are plan, implement, review, validate, investigate,
orchestrate, and general. Purpose describes why work exists. Permission,
sandbox, credentials, and workspace lease describe what it may do. They are
independent fields and policies.

The same purpose can run under different permissions, and the same role can
perform different purposes. The platform must not use a plan purpose as a
permission mode.

Purpose `validate` is for Actor-driven validation cognition, such as
investigation, reproduction, or interpretation. Deterministic commands and
trusted checks use a core-controlled ValidationRun instead; they do not need a
Human, Agent, HarnessSession, or fake System Actor.

## Immutability and lineage

Actor, Role, Purpose, Agent, HarnessProfileRevision, capabilities, Workspace,
and HarnessSession references become immutable after the Execution starts.
Parent Execution records causality and rework lineage. It does not imply that
a child shares a session or authority.

Outputs are generic Artifact and Evidence references. Execution status records
operational lifecycle; a reviewer verdict or validation result is not hidden
inside a Task state transition.

## Human and Agent executions

A Human Execution has no HarnessSession. An Agent Execution attaches to an
explicit HarnessSession when continuity is appropriate. A scheduler or
service must not choose a session by role name, model, Task, or latest
execution. Parent lineage and session continuity are separate facts.

In PR2, the continuity authority is:

~~~text
Execution.harness_session_id -> HarnessSession -> external_session_id
~~~

The generic HarnessSession records the Agent, opaque harness kind, effective
profile/configuration snapshot, capability snapshot, optional workspace scope,
and a small `pending`/`active`/`ended`/`failed` lifecycle. Its Agent and
harness identity are immutable. A pending session is not resumable until an
executor result supplies the external identity.

V124 copied old `execution.agent_session_id` values to immutable
`pr13_execution_session_projection_history` and removed the field. Those
values are never a Resume fallback. Without an exact reusable HarnessSession,
the Execution cannot Resume; any new attempt needs its explicit target context.

Plan PR3 snapshots effective `harness_capabilities` separately from the legacy
Agent `capabilities_json` tags/filters. The historical HarnessSession snapshot
is not recomputed when an Agent profile changes. Runtime invocation is generic:
an Execution without a HarnessSession starts; an Execution with one resumes the
exact external session through its HarnessAdapter. Unknown or unsupported
resume fails explicitly. Start may advance through the configured fallback
route, and its actual winner's candidate and capability evidence are stored on
the Execution and any newly established HarnessSession.

## Recovery

Restart recovery uses the persisted Execution, lease, workspace, and session
records. A recovered run either resumes the exact explicitly attached session
when historical and current adapter capability evidence allow it, or returns
an explicit unavailable/unsupported result. It never silently changes Actor
identity or turns Resume into Start.

## Durable admission and provider Start

WorkUnit Execution admission and the external provider Start are separate
boundaries. One SQLite transaction commits the Running Execution, its exact
WorkspaceLease when required, `execution.started`, and a
`execution.provider_start_pending` receipt. The pending receipt means that no
provider Start attempt has been claimed yet. Admissions created before this
receipt existed are not inferred to be pending because their provider outcome
cannot be established from the Execution row alone.

The existing TaskDispatcher discovers pending Agent WorkUnit Executions across
ordinary launch, manual recovery, follow-up, and Auto recovery. If the Agent is
paused, the dispatcher leaves the same Execution pending and sends no external
request. After unpause, a later tick can start that exact Execution without
creating another child or changing its WorkUnit, parent lineage, HarnessSession,
WorkspaceLease, or admission events. Graceful shutdown, startup CrashRecovery,
and the heartbeat stall scan preserve this unattempted admission for the
dispatcher instead of cancelling it or reporting it as a stalled provider run.

Before crossing the provider boundary, Forge commits one
`execution.provider_start_attempted` receipt. It then records the observed
accepted, rejected, or uncertain outcome. Concurrent ticks and retries use the
same Execution receipt: an accepted or rejected attempt is replayed locally,
while an uncertain outcome is surfaced as `execution_start_uncertain` and is
never sent again. An uncertain outcome leaves the Execution Running for an
external terminal result or the existing stalled-Execution monitor; it is not
treated as a fresh provider attempt. A crash after the pending receipt but
before the attempt claim is recoverable; a crash after the attempt claim is
fail-closed against a duplicate external effect.
