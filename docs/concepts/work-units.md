# WorkUnits

A WorkUnit is a concrete, executable scope inside a Task. It is not a plan,
requirements language, or second cognitive authority.

~~~text
WorkUnit
  id
  task_id
  parent_id?
  title
  scope
  status
  dependencies
  created_by
  version
~~~

WorkUnits can be created manually, by an orchestrator, from a plan Artifact,
from an issue, or from another WorkUnit. The source is provenance only; the
WorkUnit remains independently durable.

## Dependencies

Dependencies form a directed acyclic graph. Creation and update reject
obvious cycles and cross-Task edges. A blocked dependency prevents scheduling
an eligible mutating Execution, but does not pretend that the whole Task is
thinking in one role.

## Assignment

An Execution may target a WorkUnit. A WorkUnit can have retries or historical
attempts, but every attempt is a separate Execution with immutable Actor,
Role, Purpose, workspace, and session references.

## Isolation

Concurrent mutating WorkUnits use separate branches/worktrees and explicit
workspace leases. Scope text helps coordination but is not a filesystem
security boundary. A Task integration branch is written only by an explicit,
locked integration operation.

Integration success, conflict, rejection, and retry are durable operational
outcomes and Evidence. A WorkUnit being complete does not silently merge its
changes or make another WorkUnit's workspace writable.

## Plan relationship

A plan Artifact may mention or motivate WorkUnits. Forge does not maintain a
mandatory bidirectional plan-to-WorkUnit synchronization. A changed plan does
not rewrite historical WorkUnits; a changed WorkUnit does not rewrite the
historical plan.
