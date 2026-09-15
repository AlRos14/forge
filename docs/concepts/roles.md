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
  scope or work-unit references
  created_at
  ended_at
~~~

A TaskRole contains zero or more memberships. The initial role vocabulary is
planner, implementer, reviewer, and orchestrator. Persistence may support
future role names without hard-coding role-specific classes.

The same Actor can belong to several roles on one Task. Each Execution records
the concrete Role, so an Actor acting as orchestrator and later reviewer has
two distinguishable historical records.

## Coordination modes

* partitioned — members have distinct scopes, usually WorkUnits;
* collaborative — members share state and coordinate decisions;
* independent — members deliberately work without influencing each other.

Coordination mode is data on the TaskRole. It is not inferred from a role name
or from the number of memberships.

## Membership operations

Adding, ending, replacing, and suspending membership use optimistic
concurrency and preserve history. Removing one membership cannot remove other
memberships for the same Actor or role. Membership changes do not rewrite
completed Executions.

Role policy can constrain capacity, independence, approval, or disruptive
actions. It must remain a small deterministic policy representation rather than
a general workflow language.
