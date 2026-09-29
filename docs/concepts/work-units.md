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
  role
  assigned_actor?
  status
  dependencies
  requires_integration
  provenance
  created_by
  version
~~~

WorkUnits can be created manually, by an orchestrator, from a plan Artifact,
from an issue, or from another WorkUnit. The source is provenance only; the
WorkUnit remains independently durable.

Actor provenance records a typed `ActorRef` and must resolve to an existing
Human or Agent. WorkUnit and Artifact provenance must belong to the same Task.
External provenance remains an opaque reference. Old V091 Actor references
that cannot be resolved unambiguously remain readable as `legacy_actor` and
cannot be written by new requests.

## Allocation

`RoleMembership` says which Actors may participate in a TaskRole. WorkUnit
allocation names the Role required for the scope and may name one eligible
Actor. Allocation never rewrites membership, and changing allocation does not
rewrite historical Executions.

## Lifecycle

The persisted lifecycle is `open`, `completed`, or `cancelled`. A failed
Execution remains an attempt on an open WorkUnit, so a retry creates a new
Execution without destroying the scope. Runnable, blocked, running, and
awaiting-integration are derived from the WorkUnit, dependencies, Executions,
Workspaces, and integration outcomes.

An assigned WorkUnit is runnable only while its Actor still has active
membership in the WorkUnit's TaskRole. If membership is suspended or ended,
readiness asks for reallocation; it does not change the historical assignment
or rewrite RoleMembership. The current repository mutation policy admits
Agents only; an active Human assignment to a repository-mutating WorkUnit
remains recorded but is not runnable through this lease path.

A WorkUnit cannot complete while a prerequisite is unsatisfied. A repository
WorkUnit also needs a completed Execution result SHA admitted for its current
WorkUnit version; the completion transition freezes that result for explicit
integration.

## Dependencies

Dependencies form a same-Task directed acyclic graph. Self-edges, duplicate
edges, cross-Task edges, and indirect cycles are rejected. For a WorkUnit with
`requires_integration`, completion alone does not satisfy a dependency: an
explicit successful integration of its exact Execution result is required.
For a non-repository WorkUnit, completion satisfies the dependency. Readiness
is computed by the core; PR6 owns acting on ready WorkUnits.

For repository WorkUnits, declare dependencies before preparing the WorkUnit
Workspace. Once that Workspace exists, its base commit is pinned, so dependency
edges cannot be added or removed; SQLite and the service both enforce this
boundary. The `requires_integration` mode is fixed after a WorkUnit Workspace
or Execution exists, so an established repository scope cannot become an
untracked non-repository attempt.

## Assignment

An Execution may target a WorkUnit and records the exact WorkUnit ID and
admitted version. A WorkUnit can have retries or historical attempts, but
every attempt is a separate Execution with immutable Actor, Role, Purpose,
WorkUnit, Workspace, and session references.

## Isolation

The Task has an integration workspace; each repository-mutating WorkUnit has
its own branch/worktree and exact short-lived WorkspaceLease. Scope text helps
coordination but is not a filesystem security boundary. A Task integration
branch is written only by an explicit, serialized integration operation.
Completing a WorkUnit never integrates it automatically.

Integration success, conflict, rejection, and failure are durable operational
outcomes. A WorkUnit being complete does not silently merge its changes or
make another WorkUnit's workspace writable. Existing Task-scoped workspace
and execution paths remain bounded legacy behavior where they do not select a
WorkUnit workspace by Task ID.

The integration workspace has one durable `task_integration_operation` claim
per Task, shared by WorkUnit integration, final Task merge, PR publication, and
other exclusive operations on that workspace. SQLite atomically admits the
claim before Git starts. A process-lifetime OS lock lets a later server reclaim
the row after a crashed process releases the lock; Git does not hold a database
transaction open. Active terminal sessions on that workspace use matching
SQLite guards. Recovery retries from the recorded target-before SHA, recognizes
an exact already-materialized result, and fails closed on an unrelated target
HEAD without resetting it.

Cleaning a WorkUnit workspace leaves its branch in place. A `Cleaned` workspace
can be rebuilt with the same Workspace identity from that exact branch. If the
branch is missing, preparation requires explicit reset or recovery rather than
rebuilding from an older base and potentially discarding commits.

## Plan relationship

A plan Artifact may mention or motivate WorkUnits. Forge does not maintain a
mandatory bidirectional plan-to-WorkUnit synchronization. A changed plan does
not rewrite historical WorkUnits; a changed WorkUnit does not rewrite the
historical plan.
