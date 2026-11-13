# Project plan

The Project plan is an optional DAG of work packets. Each node maps to one
ordinary Task; WorkUnits and their dependencies remain the execution scopes
inside that Task. A Project can continue to create and manage Tasks manually
without a plan. Project Copilot may suggest a plan from free-form instructions,
conversation, diagrams, lists, or project material; it returns a typed proposal
with source provenance. Humans do not have to author JSON.

## Graph and proposals

`GET /projects/{id}/plan` includes every visible, non-deleted Task and every
canonical same-Project Task dependency, even before a plan proposal is applied.
An empty Project starts at graph version `1`. Task creation or deletion and
dependency changes advance the graph version, so a caller proposes and applies
against the version it inspected. A stale version returns `409 version_conflict`;
the client must refresh and review instead of silently rebasing.

A proposal records stable node keys, optional existing `task_id` mappings,
descriptions, acceptance criteria, phase and estimate ranges. Edges record
whether a dependency was explicit or inferred and cite source IDs. Sources keep
their label, kind, reference, revision, and excerpt. A proposal can create new
Tasks or map existing Tasks. Applying it preserves manual dependency edges and
history. A durable proposal/node/Task mapping preserves identity across
revisions; reusing a historical key without its explicit `task_id` is rejected
so Forge cannot create a duplicate or guess which Task was intended.

Apply is versioned and idempotent. If Task import is interrupted, a staged Task
is visible with `plan:apply_incomplete` and is blocked from coordination and
execution. Retrying the same apply request with its original idempotency key
and graph version resumes the stable import when the graph still matches the
proposal base. If another graph change advanced the version, apply returns a
version conflict and the proposal must be refreshed and reviewed. Applying a
proposal records the plan and creates or maps Tasks; it never authorizes worker
activity.

## Batch authorization and readiness

A current Project owner or admin explicitly authorizes a selected phase or Task
batch and sets `max_parallel_tasks`. That cap is independent of WorkUnit slots.
Authorization snapshots the configured coordinator TaskRole, Actor, team,
envelope, and target; Forge does not choose an Actor heuristically. Missing or
stale configuration leaves a concrete coordination blocker for the owner to
resolve. Backlog, ready, and lifecycle-blocked Tasks can remain in the batch,
including Tasks waiting on prerequisites. Active, ready-to-merge, merging,
done, and cancelled Tasks are not replanned or revived by authorization.

The scheduler admits Tasks only when their canonical prerequisites are
satisfied, lifecycle permits work, the exact coordination snapshot is current,
the batch is running, and capacity is available. A blocked successor remains
pending in its authorized batch until accepted predecessor delivery makes it
eligible; one phase authorization can therefore progress through a dependency
chain. Pausing prevents new admission; revocation removes the batch grant.
These batch projections do not replace TaskLifecycle or Task dispatch state.

An untouched TaskCoordination manual default may have a stale placeholder team
snapshot after Project defaults were copied into TaskRoles. Batch authorization
refreshes only that unconfigured default using the exact configured coordinator
Role/Actor and current `team_generation` CAS. A Human-customized coordination
keeps strict snapshot matching. The default is identified by its manual mode,
null `requested_by`/`revoked_at`, and supported default envelope values, not by
an authorization version number.

TaskCoordination authority minted for a Project batch is bound to that exact
batch and authorization version; it is not a reusable generic Task grant. The
public `POST /tasks/{id}/coordination/requests` route returns
`409 project_plan_batch_inactive` for that version even after the batch is
paused, revoked, or completed. While a batch is running or paused, the generic
Task authorization route also returns `409 project_plan_batch_inactive`;
`can_authorize` describes caller eligibility and does not override this guard.
To restore standalone Task coordination, wait until the batch is inactive and
have a Human create a fresh authorization with a new version. The previous
batch-bound version admits no generic Agent turn and is not revived by the new
authorization. Batch scheduler requests carry
the current claim token, Quality
Replan requests carry the exact source Artifact and producer Execution, and
Human Replan requests carry the explicitly selected plan Artifact ID. That plan
must have approval valid in the active authorization and batch; if it is
materialized, its receipt must match the exact Artifact, Task, request, digest,
and authorization version. Several plans can retain valid approval in one
authorization; selection uses the supplied ID and never a latest timestamp or
single canonical pointer. These origins are persisted against the Task and
authorization version rather than inferred from version arithmetic.

ReviewTriage requests bind one exact request-changes ReviewReport, its digest,
and its reviewer producer Execution. The shared coordination request has
`kind=review_findings`; it can return only one `correct|recheck|human` decision
for that report, without a plan Artifact. Recheck uses a fresh isolated formal
review of the unchanged result. Further request-changes reports open new exact
turns; replay, pause/resume and restart reuse durable request/event identities.
The terminal request state `triaged` records the immutable decision and grants
no review PASS, Gate waiver, Task Done or successor release.

While the current authorization is batch-bound, generic Task events cannot
create Agent coordinator wakes, including after the batch is paused, revoked,
or completed. The four typed request origins are the only batch-scoped
Agent coordination entries. Human-targeted collaboration remains available.
Terminal events for existing batch-linked coordinator Executions continue to
reconcile against their exact durable wake, attempt, and request; they do not
create generic replacement turns. Provider Start rechecks the exact batch
request and Execution link in its write transaction, so a generic wake that
races with batch authorization fails closed.

The Task coordination overview exposes `project_plan_batch_scope` from the exact
authorization binding: `batch_id`, `batch_state`, `task_state`, and `blocker`.
It reports `can_request: false` while the current authorization is batch-scoped;
the Task UI does not send the generic turn request for that origin. If the
recorded blocker is exactly `coordination:coordination.budget_exhausted`, an
owner/admin can raise `max_turns` without changing the TaskCoordination
authorization version or creating a generic RequestTurn. On its next tick, the
scheduler retries only while the same batch is running, prepared, and still
passes current authorization, Task team, available-turn, and prerequisite
checks. Paused, revoked, completed, and differently blocked Tasks remain
untouched. Quality-request and rework budgets are separate and resume only their
own eligible budget-blocked records. The Task detail shortcut adds eight
coordinator turns only; the other limits stay editable individually.

For a repository dependency, successful Task completion alone is insufficient:
the accepted WorkUnit result must also have the required successful integration.
The scheduler pins each predecessor's exact accepted result and integration
commit into a handoff record. A successor workspace must prove that its base is
at or after the pinned integration commit. Missing or unverified ancestry fails
closed before provider Start and persists the blocker.

## Projections and UI

Readiness describes prerequisites and lifecycle eligibility; it does not mean
that a batch is authorized. Dispatch state describes Task dispatch, and a batch
state describes its own running, paused, revoked, or completed authority. The
graph exposes causal blockers and currently running Executions separately from
potential topological parallelism.

The Project Plan view presents the graph as an SVG plus accessible Task cards
and a dependency table. It distinguishes inferred and explicit edges, shows
sources and revisions, and lets Project writers apply proposals while only
Project owners/admins can select a batch or pause, resume, and revoke one.
Global administrator status alone does not grant Project authority. Missing or
invalid estimates remain unknown. Critical-path
intervals appear only when all required estimates are valid; they are not ETAs
and do not include review, integration, capacity, or external delay.

Opening Copilot from this page uses Project context with no Task attached. The
panel does not send a message or start a model turn until a Project member sends
one.
