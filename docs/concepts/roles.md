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
projection, but `RoleMembership` is the eligibility authority once its
TaskRole exists. A compatibility representative is never an allocation,
execution, or workspace-authority decision.

Role policy can constrain capacity, independence, approval, or disruptive
actions. It must remain a small deterministic policy representation rather than
a general workflow language.
