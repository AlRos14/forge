# WorkUnits

> Diseño objetivo aprobado, todavía pendiente de implementación:
> [modelo operativo de tareas y UX](task-operating-model.md). Define la
> separación Copilot/Orquestador, preparación desde objetivo y rutas de calidad.
> El resto de este documento describe el contrato actual salvo indicación expresa.


A WorkUnit is a concrete, executable scope inside a Task. It is not a plan,
requirements language, or second cognitive authority. Its `role` names the
TaskRole used for eligibility; formal review behavior comes from that role's
`formal_review` capability, never the string itself.

~~~text
WorkUnit
  id
  task_id
  parent_id?
  title
  scope
  role
  assigned_actor?
  execution_instructions?
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
Optional `execution_instructions` are the exact harness instructions used
instead of the generic Task/WorkUnit prompt when the dispatcher admits an
Agent Execution. They are prepared scope, not a second Task or Execution form.
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

TaskLifecycle is aggregate Task progress. An Execution admission normally
resolves an Execution-owned block only when the current lifecycle reference,
blocking annotation (when present), and exact parent Execution identify the
same open WorkUnit. A sibling cannot clear a block owned by another WorkUnit.
If crash recovery or timeout reconciliation had to stop multiple Executions
and the aggregate block has no single owner, an authorized Human may select
one exact cancelled Manual Execution with its WorkUnit and current versions.
The selection
atomically resolves the ambiguous Task block and admits that exact child;
remaining WorkUnits retain their own pending Execution history and can be
selected while the Task is Active. An Auto crash or graceful-shutdown candidate
continues only by resuming its same active, explicitly linked HarnessSession;
if that session is not reusable, automatic dispatch waits for an exact Human
selection. The Human may explicitly authorize a fresh child for a cancelled
Auto candidate. `error_annotation` and `blocked_json` carry
different details; each present field must independently identify its owner
before admission may clear it. Lifecycle activation, metadata cleanup, the
child Execution, and its idempotency receipt commit together. A manual
lifecycle transition cannot bypass pending blocking metadata.

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

A Gate requirement can pin the WorkUnit ID and version, dependency snapshot,
Execution result, and exact WorkUnitIntegration. Completion alone does not
satisfy an integration requirement. Gate evaluation does not alter WorkUnit
completion or integration state.

## Dependencies

Dependencies form a same-Task directed acyclic graph. Self-edges, duplicate
edges, cross-Task edges, and indirect cycles are rejected. For a WorkUnit with
`requires_integration`, completion alone does not satisfy a dependency: an
explicit successful integration of its exact Execution result is required.
For a non-repository WorkUnit, completion satisfies the dependency. Readiness
is computed by the core. The TaskDispatcher acts on ready WorkUnits only while
the Task dispatch intent is `running`; pause prevents new admissions and
provider starts.

For repository WorkUnits, declare dependencies before preparing the WorkUnit
Workspace. Once that Workspace exists, its base commit is pinned, so dependency
edges cannot be added or removed; SQLite and the service both enforce this
boundary. The `requires_integration` mode is fixed after a WorkUnit Workspace
or Execution exists, so an established repository scope cannot become an
untracked non-repository attempt.

## Assignment

An Execution may target a WorkUnit and records the exact WorkUnit ID and
admitted version. A WorkUnit can have retries or historical attempts, but
every attempt is a separate Execution with immutable Actor, Role, WorkUnit,
Workspace, and session references.

An open WorkUnit with a completed Execution result is `awaiting_acceptance`.
The dispatcher does not create another automatic attempt while that result is
pending. Acceptance and repository integration remain explicit facts; a
completed Execution alone does not complete the WorkUnit.

The Execution admission transaction also records that an Agent provider Start
is pending. The existing dispatcher may retry only until it atomically records
the one provider-start attempt. A pause before that claim leaves the committed
Execution pending for a later tick; an uncertain provider response cannot
authorize another external Start. Admission receipts and the WorkspaceLease
remain attached to the same Execution throughout recovery.

Heartbeat expiry, daemon reports, and restart recovery treat a pending receipt
as queued work rather than an external provider effect. If a required Workspace
or lease authority has expired, recovery terminalizes the still-pending
Execution and blocks the Task without reviving the lease. This write is
conditional on the absence of the provider-attempt receipt, so a concurrent
Start attempt keeps its existing uncertain or accepted recovery behavior.
Explicit Task cancellation still cancels pending work locally and cleans up
Running Executions even if recovery had already transitioned the Task lifecycle.

## Launch, pause, and cancellation

Task launch is a durable dispatch intent under TaskLifecycle authority. The
Human requests a Task launch with expected Task and TaskLifecycle versions and
an idempotency key; the response distinguishes intent admission, admitted
Executions, and accepted provider starts. Pause prevents new admissions and
provider starts but does not claim to stop work already started. Resume
re-dispatch currently ready WorkUnits; cancellation records the request and
observes exact Execution outcomes separately.

The dispatcher resolves readiness from current records, admits only exact
assigned Agent WorkUnits, and respects Agent capacity and the bounded per-Task
concurrency ceiling. The current ceiling is four concurrent Executions per
Task, enforced inside Execution admission as well as by the dispatcher tick. A
Human-assigned WorkUnit remains pending human work and creates no fake
Execution. A missing or paused Agent leaves an explicit blocker; the dispatcher
never selects another account or Actor. Pause is rechecked at both automatic
admission and Provider Start claim, so a successful pause response prevents
later automatic starts.

## Result subjects and rework

An initial reviewer WorkUnit is prepared with `review_subject` naming the exact
same-Task WorkUnit it will review. The target may still be unassigned or running.
Preparation versions do not identify its result: Forge freezes the admitted
result version and attempt when the target produces its accepted delivery.
Allocation and instruction edits before dispatch must not strand the review on
an earlier preparation version. Until a concrete result is available, the
review is waiting; absence of a frozen result is never permission to run.

An assigned Human submits a report through the reviewer WorkUnit's
`review-report` operation. Forge creates the provenance attempt internally,
stores the report, and completes the review WorkUnit. Retries reuse the exact
attempt/report, including recovery after a report was persisted before WorkUnit
completion. The Human does not select an Execution or a HarnessSession.

A review WorkUnit may record `work_unit_review_subject`: the exact reviewed
WorkUnit version, result Execution, commit, Workspace, and snapshot digest. A
correction WorkUnit may record `work_unit_result_requirement`: the exact source
result it consumes. These are minimal relations, not a workflow DSL. Cognitive
harnesses receive the exact pinned input context at launch. Shell launches
validate the same inputs but preserve literal execution instructions, without
appending that context as executable code.

For multiple deliverables, prepare one package WorkUnit that consumes the exact
source Artifacts and records an immutable manifest for its package result. A
formal review then names that package WorkUnit as its one exact subject. The
manifest records each source Artifact ID and digest, the producing Execution,
and the delivery Artifact. The PO delivery WorkUnit follows the review line and
cannot run until the latest package result has a favorable report.

Requesting changes and a new review is one idempotent operation. It requires a
ReviewReport whose exact subject names the reviewed WorkUnit and version; a
generic Task report cannot be applied to another delivery. The operation
validates the report and subject, creates a correction and a new reviewer
WorkUnit, records the correction result subject, and consumes one bounded
rework round atomically. New Tasks default to sixteen correction/reassessment rounds; historical
limits are preserved and Human extensions are versioned. Replaying the same key consumes no additional round.

A repository correction Workspace is prepared from the reviewed result commit,
not from the Task integration branch, so it can correct a result that has not
been integrated. The new reviewer WorkUnit waits for the exact completed
correction result through the result subject relation; it does not require the
correction to be integrated first. A Human contributor cannot independently
review that result. An Agent may reuse the same saved Actor/profile across
roles, but review requires a fresh native HarnessSession and isolated role
context, without reusing or forking the contributor conversation. V162 session
and current RoleMembership checks enforce that boundary on later allocation
and admission as well. Exhaustion or
an unavailable independent reviewer leaves an explicit human decision instead
of an automatic approval.

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

Cleanup atomically claims a `Ready` WorkUnit workspace as `Cleaning` only when
it has no running Execution or active WorkspaceLease. WorkUnit Execution
admission requires that exact workspace to remain `Ready`, so SQLite decides
whether admission or cleanup wins across server processes. Cleanup also holds
the canonical Task operation file lock while it runs Git cleanup and reconciles
stale Task operations. The file lock serializes filesystem effects; it does not
grant cleanup authority.

`Cleaning` can be resumed after a crash. Recovery removes only the exact
WorkUnit worktree and leaves its branch in place. If the branch is missing or
the recorded branch/path no longer matches the WorkUnit identity, the workspace
stays unavailable and requires explicit recovery. A `Cleaned` workspace can be
rebuilt with the same Workspace identity from that exact branch. Neither path
recreates content from an older base SHA or changes sibling workspaces,
branches, or the Task integration workspace.

## Plan relationship

A plan Artifact may mention or motivate WorkUnits. Forge does not maintain a
mandatory bidirectional plan-to-WorkUnit synchronization. A changed plan does
not rewrite historical WorkUnits; a changed WorkUnit does not rewrite the
historical plan.

Coordination materialization creates WorkUnits only after an exact approved
`work_unit_plan`; the model never names WorkUnit IDs or grants executable scope.
A coordination turn itself is not a WorkUnit and has no WorkspaceLease. The
deterministic materialization receipt makes replay return the same WorkUnits.
