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

The additive persistence shape currently stores `actor_kind` and `actor_id` as
the physical form of `actor_ref`, plus one of these exact purposes:
`plan`, `implement`, `review`, `validate`, `investigate`, `orchestrate`, or
`general`. Purpose is supplied by the semantic creation path; it is not a
permanent alias for a Role or for the transitional executor
`permission_policy: plan` value.

The old `execution.agent_id` and `execution.agent_session_id` columns remain
nullable compatibility projections. For a new Agent Execution,
`actor_ref = Agent(agent_id)` and the Agent id is projected to `agent_id`. A
new Human Execution stores the real user id, has no Agent projection, and has
no HarnessSession.

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
                                                     |
                                      legacy agent_session_id projection
~~~

The generic HarnessSession records the Agent, opaque harness kind, effective
profile/configuration snapshot, capability snapshot, optional workspace scope,
and a small `pending`/`active`/`ended`/`failed` lifecycle. Its Agent and
harness identity are immutable. A pending session is not resumable until an
executor result supplies the external identity.

## Recovery

Restart recovery uses the persisted Execution, lease, workspace, and session
records. A recovered run either resumes the exact explicitly attached session
when the adapter supports it, or follows an explicit unsupported/fallback
policy. It never silently changes Actor identity.
