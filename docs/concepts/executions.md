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

## Purpose and permission

Initial purposes are plan, implement, review, validate, investigate,
orchestrate, and general. Purpose describes why work exists. Permission,
sandbox, credentials, and workspace lease describe what it may do. They are
independent fields and policies.

The same purpose can run under different permissions, and the same role can
perform different purposes. The platform must not use a plan purpose as a
permission mode.

## Immutability and lineage

Actor, Role, Purpose, Agent, HarnessProfile, capabilities, Workspace, and
HarnessSession references become immutable after the Execution starts. Parent
Execution records causality and rework lineage. It does not imply that a child
shares a session or authority.

Outputs are generic Artifact and Evidence references. Execution status records
operational lifecycle; a reviewer verdict or validation result is not hidden
inside a Task state transition.

## Human and Agent executions

A Human Execution has no HarnessSession. An Agent Execution attaches to an
explicit HarnessSession when continuity is appropriate. A scheduler or
service must not choose a session by role name or latest execution.

## Recovery

Restart recovery uses the persisted Execution, lease, workspace, and session
records. A recovered run either resumes the exact explicitly attached session
when the adapter supports it, or follows an explicit unsupported/fallback
policy. It never silently changes Actor identity.
