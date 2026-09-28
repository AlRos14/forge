# Plan PR5: WorkUnit DAG and isolated integration

Status: PR5 implementation ledger for this branch. It records authority
implemented here separately from runtime paths retained for later plans.

This Plan PR makes WorkUnit the authority for concrete executable scope,
dependencies, and allocation. It preserves Task as the aggregate work owner,
Execution as one immutable historical attempt, Workspace as repository
isolation, and explicit integration as the only path from a WorkUnit result to
the Task integration branch. It does not migrate planning, Task lifecycle,
review, validation, or orchestration cognition.

## Authority boundary

`RoleMembership` answers which Actors may participate in a TaskRole. It does
not allocate a WorkUnit. A WorkUnit records a Task-local scope, required Role,
optional assigned Actor, dependency edges, and a small lifecycle. The assigned
Actor must have active membership in the WorkUnit's Role. Reallocation changes
only WorkUnit state; it does not rewrite membership or historical Executions.

An Execution records its exact WorkUnit ID and admitted WorkUnit version when
the caller uses the WorkUnit-aware path. Actor, Role, Purpose, WorkUnit, and
Workspace identity remain historical. Retries create new Executions. WorkUnit
scope text is coordination context and is not a filesystem sandbox.

No plan table is read or written by the WorkUnit path. Plan Artifacts and
legacy `task_plan_revision` remain outside this authority until PR7.
Collaboration Proposals and Decisions record intent and outcomes only; they do
not allocate, cancel, dispatch, or integrate work.

## Physical model and lifecycle

The additive schema stores WorkUnits and same-Task dependency edges, including
Task-local parent and Role constraints, optional Actor allocation, provenance,
optimistic version, and `requires_integration`. Dependencies are directed from
the dependent WorkUnit to its prerequisite. Self-edges, cross-Task edges,
duplicate edges, and direct or indirect cycles are rejected. Adding an
existing edge returns a conflict; removing a missing edge uses the normal
versioned not-found/conflict path. For repository WorkUnits, the dependency
set is fixed once its isolated Workspace exists: that Workspace pins the Task
integration HEAD from which the WorkUnit's attempts proceed. A later dependency
change could make the recorded graph say "ready" while the branch still lacks
the prerequisite, so service checks reject the edit and SQLite repeats the
guard.

WorkUnit lifecycle is `open`, `completed`, or `cancelled`. A failed or stopped
Execution does not terminalize its WorkUnit. Readiness, dependency blocking,
active execution, and integration state are derived from WorkUnit,
dependencies, Executions, Workspaces, and integration records; they are not
additional lifecycle states.

Completion is rejected while a prerequisite is unsatisfied. A repository
WorkUnit must have a completed Execution with a result SHA admitted for the
current WorkUnit version before completion. Its terminal transition freezes
that exact result for later integration.

Executable scope, allocation, and dependency edges cannot be edited while a
WorkUnit has a running Execution. This keeps each admitted attempt tied to the
scope and readiness decision it recorded.
The `requires_integration` mode is also pinned once a WorkUnit Workspace or
Execution exists; the core rejects changing an established scope between
repository and non-repository execution.

Allocation may be empty. An optional allocated Actor is accepted only when
that Actor has active membership in the named TaskRole. Suspended or ended
membership does not authorize a new allocation. Existing Execution history
continues to show the Actor that actually performed it.
Readiness rechecks an existing allocation against active membership. A
suspended or ended assignee makes the WorkUnit non-runnable and ready for
reallocation without rewriting the stored assignment.
Repository mutation currently requires an Agent Actor under the existing
WorkspaceLease policy. A Human may hold a valid allocation, but that repository
WorkUnit remains non-runnable until it is allocated to an eligible Agent.

## Dependency satisfaction and readiness

A dependency is satisfied when its prerequisite WorkUnit is completed and:

* if `requires_integration` is false, completion is sufficient; or
* if `requires_integration` is true, a successful durable integration of an
  exact source SHA from that WorkUnit is recorded in the owning Task's
  integration workspace.

An Execution finishing does not satisfy a repository-backed dependency. A
dependent WorkUnit must be based on an integration HEAD that includes the
prerequisite result. Readiness is a deterministic query only; PR6 owns event
consumption, automatic dispatch, and wakeups.

## Workspace and lease identity

Workspace rows distinguish the Task integration workspace from WorkUnit
workspaces. An old Task workspace migrates in place as Task integration scope:
its ID, path, branch, SHA, status, and timestamps are preserved, and no
historical WorkUnit is fabricated. A WorkUnit workspace is keyed by its exact
Task, WorkUnit, repository, and opaque Workspace ID. Partial uniqueness allows
one canonical integration workspace per Task and one canonical workspace per
WorkUnit.

New paths and branch refs derive from opaque Task/Workspace/WorkUnit IDs, not
titles or scope. WorkspaceManager gains exact-Workspace create, recover, and
cleanup operations. Task-wide legacy operations remain bounded to legacy Task
workspaces; ambiguous Task lookups fail closed. WorkUnit cleanup removes only
its worktree registration and directory and never removes the integration
workspace, a sibling WorkUnit, or another branch. Legacy plan capture remains
limited to the legacy Task cleanup path.

New WorkUnit repository authority binds Project, Task, WorkUnit, Execution,
Workspace, repository binding, base ref/SHA, Role, assigned principal,
capability profile, issuer, expiry, and idempotency. Active lease uniqueness
is per WorkUnit/Workspace for new rows. The Task-only lease getter and
uniqueness remain only for historical/legacy Executions with no WorkUnit.
The lease and `execution.started` event are persisted in one transaction. The
current repository mutation profile requires an active Agent TaskRole member
and the Task's approved governance/capability envelope. Human Actors remain
valid WorkUnit creators, allocations, and non-repository Execution principals;
this lease path does not grant repository mutation to a Human Actor. No path,
handle, capability profile, or bearer material is exposed in a public DTO.

## Execution and concurrency

Execution gains nullable WorkUnit ID/version fields. Historical rows remain
valid. The WorkUnit-aware service start validates same-Task scope, exact
WorkUnit version, open/runnable status, Role compatibility, allocation
eligibility, Workspace identity, and lease identity before returning a
repository-capable Execution. Independent WorkUnits can hold distinct active
repository Executions, Workspaces, and leases concurrently; one running
Execution per WorkUnit is admitted. The Task integration workspace is never
issued to a WorkUnit worker. PR5 does not route this service path through the
legacy Task dispatcher or daemon runner; PR6 owns that runtime consumer.

Once a Task has WorkUnits, Task-scoped repository launches fail closed. A
WorkUnit cannot be added while an unbound Task Execution, Task lease, or
Task-integration terminal session is active. The database repeats these
checks at the authority boundary so a legacy Task worker cannot later claim
the Task integration workspace as its mutable workspace.

Task-scoped `get_by_task_id` readers are classified individually: public or
legacy Task behavior remains bounded to the integration workspace, while
WorkUnit operations use explicit WorkUnit/Workspace IDs. No reader selects a
WorkUnit workspace by row order or latest timestamp. The Task-level terminal
remains on the integration workspace. The legacy terminal surface does not
accept WorkUnit scope and never guesses among siblings.

## Explicit integration

Completing a WorkUnit never merges it. Integration pins the exact completed
Execution, WorkUnit Workspace, source branch/ref/SHA, Task integration target,
and target-before SHA in a durable attempt. Task integration is locked and
serialized. A retry with the same idempotency identity and exact source/target
bindings returns the recorded outcome; a changed source cannot reuse it.

Outcomes include success, conflict, and rejected/failed. A Git conflict is
durable; the merge is aborted so the integration worktree remains valid, and
the source WorkUnit workspace remains available for later work. PR5 does not
select an Agent to resolve the conflict. Task final delivery/PR publication
remains distinct from WorkUnit integration. For a Task with WorkUnits,
MergeService uses the Task integration workspace's recorded branch and does
not modify a WorkUnit Execution. Tasks without WorkUnits keep the latest
legacy Task executor source, filtered to Executions with no WorkUnit binding.
Publication, WorkUnit workspace preparation, WorkUnit creation, and
integration-workspace cleanup share the Task integration lock. Operations that
touch the integration worktree also take its exact Workspace lock, which is
shared with Task-level terminals. WorkUnit workspace cleanup and Execution
admission share the WorkUnit's exact Workspace lock.
Cleanup retains the Task integration workspace once WorkUnits exist and does
not capture the legacy plan on that path. An active durable WorkUnit
integration makes final delivery fail closed.

## Collaboration and events

Proposal may target a same-Task WorkUnit using PR4's resolve-ID, authorize
Task-to-Project, then load semantics order. Message and Handoff may each carry
an explicit contextual WorkUnit relation; recipients remain Actor, Role, or
Task. Their WorkUnit context never grants authority or implies HarnessSession.

Every WorkUnit, allocation, dependency, completion/cancellation, and
integration mutation commits its bounded `domain_event` in the same database
transaction. Payloads contain IDs, versions, and enum status/outcome only.
EventBus notification remains post-commit. PR6 may consume these events later;
PR5 adds no event consumer or orchestrator loop.

## Project teardown

Guarded Project deletion removes WorkUnit relation rows, dependency edges,
integration records, WorkUnit rows, and WorkUnit-aware leases/workspaces in FK
order. Immutable-row guards are bypassed only by the existing project deletion
guard. The durable event ledger remains under its existing retention contract;
foreign-key enforcement is never disabled globally during teardown.

## Compatibility and deferrals

Tasks without WorkUnits retain the existing Task-scoped execution and
Task-workspace path. Existing workspace rows remain Task integration rows.
Task workspace and lease APIs remain only where their caller is demonstrably
legacy or integration-scoped; all WorkUnit authority is explicit and
fail-closed on ambiguity. The existing Task dispatcher, runner, and Task-level
terminal and cleanup consumers remain bounded compatibility paths; the
WorkUnit runtime service does not select them implicitly. PR13 owns the
remaining historical readers and physical cleanup.

PR6 owns event-driven orchestration, worker selection, dispatch, and wakeups.
PR7 owns plan Artifacts and legacy plan migration. PR8 owns ValidationRun,
Evidence, and review migration. PR9 owns aggregate Task lifecycle and Gates.
PR13 owns final legacy-schema and compatibility cleanup. PR5 creates no plan
engine, ValidationRun, Evidence engine, Gate engine, scheduler loop, or
automatic integration.
