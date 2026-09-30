# Roles and memberships

Roles are Task-scoped responsibility. They are not Agent classes and do not
describe the intrinsic type of an Actor.

## TaskRole

~~~text
TaskRole
  task_id
  role
  coordination_mode
  policy
  version

RoleMembership
  task_role_id
  actor_ref
  status
  created_at
  ended_at
~~~

A TaskRole contains zero or more memberships. The initial role vocabulary is
planner, implementer, reviewer, and orchestrator. Persistence may support
future role names without hard-coding role-specific classes. During the
singular-assignment migration, `coder`, `worker`, `assignee`, and `executor`
are normalized to `implementer`; `interactive`, `merge_fixer`, and `system`
remain execution labels rather than becoming TaskRoles.

RoleMembership records participation in a TaskRole only. It does not contain a
WorkUnit, path, concrete scope, or assignment reference. Allocation belongs to
WorkUnit and Execution: an implementer may remain a member while moving
between WorkUnits, and changing that allocation never rewrites membership
history.

The same Actor can belong to several roles on one Task. Each Execution records
the concrete Role, so an Actor acting as orchestrator and later reviewer has
two distinguishable historical records.

## Coordination modes

* partitioned — members have distinct scopes, usually WorkUnits;
* collaborative — members share state and coordinate decisions;
* independent — members deliberately work without influencing each other.

Coordination mode is data on the TaskRole. It is not inferred from a role name
or from the number of memberships.

Legacy singleton TaskRoles created by the additive migration may have a null
coordination mode while they have zero or one current member. That value is a
transitional absence of a multi-member coordination decision, not a fourth
mode. Before a second current membership is added, the caller must set one of
the three modes above; newly created TaskRoles require it immediately.

## Membership operations

Membership status is `active`, `suspended`, or `ended`. Adding, ending,
replacing, and suspending membership use optimistic concurrency and preserve
history. Removing one membership cannot remove other memberships for the same
Actor or role. Membership changes do not rewrite completed Executions.

Legacy singleton assignment writes may update a deterministic compatibility
projection, but `RoleMembership` is the current eligibility authority once its
TaskRole exists. The old `task.assignee_*` and `task_role_assignment` values
must never select scheduling candidates, concrete Execution Actors,
follow-up/re-execute Actors, or workspace/terminal authority in that case. A
compatibility representative is never an allocation, execution, or
workspace-authority decision. The authoritative membership mutation and any
required compatibility projection commit together; membership and task events
are emitted only after that transaction succeeds.

An Execution keeps the concrete historical Actor that performed it. Workspace
authority remains scoped to that concrete Execution and its matching
WorkspaceLease; adding another Actor to the same TaskRole does not grant that
Actor access to the existing workspace, lease, terminal, or repository write
scope.

Role policy is a small deterministic JSON object, not a general workflow
language. The target architecture may eventually use it for capacity,
independence, approval, and disruptive-action constraints. PR6 currently
understands only the following TaskRole policy:

~~~json
{
  "schema_version": 1,
  "automatic_orchestration": true,
  "allowed_actions": ["message", "handoff", "create_work_unit", "proposal"],
  "max_actions_per_execution": 16,
  "max_work_unit_creations_per_execution": 4
}
~~~

Every field is optional. `{}` means schema version 1, automatic Agent
orchestration enabled, every PR6 action allowed, and the limits shown above.
`automatic_orchestration: false` prevents automatic Agent dispatch; a Human
orchestrator still receives durable `awaiting_human` work. `allowed_actions`
can only restrict the four typed PR6 actions. The two numeric limits can be
lowered but not raised above 16 actions or 4 WorkUnit creations per Execution.
The Agent's current execution capacity is still enforced by the existing
capacity check; PR6 does not interpret a per-Role `capacity` field.

PR6 rejects duplicate action names, malformed values, unsupported schema
versions, and unknown fields when it evaluates the policy. The API and database
continue to preserve any valid JSON object, but an unknown field such as
`capacity`, `approval`, or `independence` makes PR6 fail the wake closed rather
than silently ignore that constraint. Invalid JSON and non-object values still
fail at write time.

A wake snapshots the TaskRole version and exact `policy_json` in addition to
the fixed PR6 runtime policy identity. Dispatch requires the current TaskRole
version, policy JSON, and coordination mode to match that snapshot. Before
each action and replay, PR6 rechecks the same snapshot and applies the action
allowlist and limits in deterministic code. A TaskRole policy or coordination
change makes an already admitted wake fail closed. A durable TaskRole-change
event gives the new policy an opportunity only where current coordination mode
provides an unambiguous target; PR6 does not turn an ambiguous independent or
partitioned role update into a generic fanout.
