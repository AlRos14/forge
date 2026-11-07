# ADR 0016: Task-scoped coordination turns

Status: accepted for implementation
Date: 2026-11-07
Related: ADR 0009 (event-driven orchestration), ADR 0014 (Task launch and
purpose retirement), ADR 0015 (TaskRole capabilities and review lines)

## Context

A new Task has an approved Project team but no WorkUnits. The current launch
preflight rejects that Task because dispatch needs concrete allocated
WorkUnits. The only current way to create those WorkUnits before launch is a
Human editing scopes, Roles, Actors, dependencies, review subjects, and
inspection inputs manually. That is a circular bootstrap for the guided
journey: the Task cannot start the coordination turn that would prepare the
WorkUnits because launch requires the WorkUnits first.

Forge already has three relevant mechanisms, and none is a complete answer:

1. **Orchestrator wakes.** A committed Task-scoped `domain_event` admits a
   durable `orchestrator_wake` for an exact active member of a TaskRole with
   the `orchestration` capability. Dispatch revalidates the TaskRole version,
   policy snapshot, coordination mode, active membership, lease, and attempt,
   then creates a workspace-free, read-only Execution. The path already has
   bounded actions, action receipts, retries, stale-authority failure,
   provider Start receipts, and uncertain-result handling. Its trigger is
   generic event classification and its output is the fixed
   `message|handoff|create_work_unit|proposal` envelope.
2. **Project Copilot.** It is a durable Project management run with a
   conversation and a live Project-scoped MCP delegation. From a Task it can
   propose or create WorkUnits, but its authority is Project management
   authority. It is not an exact TaskRole membership decision, it has no Task
   lifecycle binding, and its run is deliberately not a Task Execution.
3. **A coordination WorkUnit.** A WorkUnit can carry a real planning or
   investigation deliverable, but a coordination turn is not a deliverable
   scope. Creating a placeholder WorkUnit to pass the launch guard would make
   a management attempt look like executable work, consume WorkUnit identity
   and result/acceptance semantics, and invite a second readiness authority.

The operating model requires a bounded preparation and supervision turn that
is separate from worker Executions, bound to the selected team, and unable to
grant itself worker, Role, repository, Gate, or merge authority.

## Decision

Add a **Task coordination request** whose execution reuses the existing
orchestrator wake, attempt, Execution, and provider Start machinery.

The coordination request is a new durable Task record. A Human with current
Task mutation authority records the coordination mode and exact coordinator
selection in a versioned authorization. A request then emits a
`task.coordination_requested` domain event. The existing orchestrator consumer
classifies that event as an **exact-Actor wake** for the authorized
orchestration-capable TaskRole and coordinator; it does not fan out to every
orchestrator member. No second scheduler is introduced.

The coordination turn remains a real Execution: exact Task, Agent Actor,
TaskRole version/configuration/policy, HarnessSession rules, and provider
receipts apply. It is workspace-free and read-only, like the current
orchestrator path. It has its own bounded JSON output contract and does not
use the generic orchestrator action envelope. The output is a versioned
`work_unit_plan` Artifact with an exact producer Execution, not a WorkUnit and
not a worker start.

Materialization is a separate deterministic, idempotent operation. It takes
the exact plan Artifact digest, authorization version, Task versions, and
current team snapshot, validates every WorkUnit/Role/Actor/dependency/review
reference, and creates the WorkUnits in one transaction. The model's text is
never permission. Worker dispatch still requires the existing durable launch
intent, exact WorkUnit allocation, and WorkspaceLease where applicable.

Two modes are fixed for the first vertical slice:

* `proposal` produces an inspectable plan and waits for an explicit Human
  approval. It creates no WorkUnit and starts no provider.
* `prepare_and_execute` lets the authorizing Human delegate materialization
  and launch of a plan that stays inside the recorded envelope. An item
  outside the envelope becomes a proposal/exception and waits for a Human.
  Each worker still passes the normal admission and lease checks.

`manual` remains the default and preserves today's behavior. Existing Tasks
are migrated to `manual`; no coordination turn is triggered retrospectively.

Project Copilot remains a Project management surface. It may help a Human
draft choices, but it cannot become the authority for an active Task's
coordination turn, and its Project authority is not copied into the Task
path. A future Task-scoped Copilot delegation may reuse the same plan and
intervention contracts, but it must enter through the same authorization and
request records.

## Reasons

* It breaks the circular bootstrap without inventing a WorkUnit that satisfies
  a guard but does not represent deliverable work.
* It reuses the strongest existing reliability boundary: durable wakes,
  attempt numbering, provider Start receipts, stale-authority guards, and
  uncertain-result reconciliation. A new scheduler would duplicate these.
* It keeps the coordination turn distinct from worker authority. Coordination
  can prepare work; it cannot execute repository work or grant a lease.
* It preserves the current TaskLifecycle, WorkUnit, review, Gate, and
  collaboration authorities without a second source of truth.
* It gives the Human a real approval boundary and gives the UI facts it can
  explain: no coordinator, no capability, stale team, plan waiting, plan
  materialized, worker blocked, or human decision required.

## Rejected alternatives

### A coordination WorkUnit

A WorkUnit may legitimately produce a plan or investigation Artifact. It is
the right shape when the plan itself is a deliverable someone asked for. It is
the wrong shape for the bootstrap turn because it requires a WorkUnit
allocation before the turn can run, makes management look like deliverable
scope, and would need special rules for completion, dependency satisfaction,
and result acceptance. The same Actor may still perform a planning WorkUnit
when a Task explicitly needs one.

### Project Copilot as the coordinator

Project Copilot has conversation and WorkUnit management, but its durable
authority is the Project owner, not an exact TaskRole membership. A Task can
have a different team, different coordination mode, and stricter policy. Using
the Copilot run as the turn would either copy Project authority into the Task
or create a second, weaker check. Copilot remains assistance, not the Task's
coordination authority.

### A hidden pre-launch worker Execution

Creating an ordinary worker Execution to plan would need a WorkUnit, an
Actor allocation, and a Workspace decision before it could start. It would
also blur provider Start accounting and make a planning turn look like
implementation. The coordination request keeps the attempt identifiable and
workspace-free.

### A new Task scheduler

The dispatcher, orchestrator consumer, wake leases, and provider receipts
already solve durable admission and recovery. A parallel queue would create a
second authority and a second failure model.

## Consequences

* Additive persistence for the authorization, request, plan binding, and
  materialization receipt. Existing wake and action tables remain historical
  and unchanged unless implementation proves a narrow additive column is
  necessary.
* New API, MCP, CLI, SSE, and web projections for the authorization, request,
  plan, approval, and materialization states. They reuse Task mutation
  permissions and exact version conflicts.
* New tests for exact-Actor wake classification, authorization/team drift,
  provider uncertainty, pause/cancel, replay, atomic materialization, and the
  two guided modes.
* No change to TaskLifecycle as the aggregate Task state authority, no change
  to WorkUnit as executable scope, and no change to Gate or review authority.
* Historical orchestrator wakes and Executions are not relabeled or replayed.
  Existing Tasks receive `manual` coordination and keep working exactly as
  they do now.

## Compliance summary

| Invariant | How this decision complies |
|---|---|
| INV-001/002 | The coordinating Agent and the authorizing Human are ordinary Actors; no special Agent class. |
| INV-003/004/005/006 | The turn binds an exact harness-bound Agent, TaskRole ID/version, active membership, and one role. The same Actor may hold other roles. |
| INV-007 | Coordination state is separate from the Task lifecycle; no cognitive state is encoded in the Task. |
| INV-008/009/011 | The turn is an Execution with an exact Actor and no purpose classification; a Human coordinator does not receive a fake harness session. |
| INV-010 | No session is inferred from Role, Task, or latest Execution. |
| INV-012/013 | The adapter must prove the read-only capability; unsupported hosts fail closed. |
| INV-014/015/016 | The plan is an Execution-produced Artifact; WorkUnits are created only by deterministic materialization. |
| INV-017 | The turn has no Workspace or repository write authority. |
| INV-018/019/020/021/022 | Coordination prepares work and may supervise later; it does not become a reviewer, micromanage reasoning, or run continuously. |
| INV-023/024/025/026 | Collaboration remains Message/Handoff/Proposal/Decision; disruptive actions stay policy-controlled. |
| INV-027/028/029/030 | Plan Artifact is generic; review, validation, Gate, and deterministic authority are unchanged. |
| INV-031/032 | The model proposes; Forge validates. Unsupported behavior is reported, not simulated. |
| INV-033/034/035 | New records preserve history; one authority per fact; additive migrations only. |
| INV-038/039/040 | Task mutation authority, Project ownership, and Task parent rules remain exact. |

The detailed authority matrix, proposed record shapes, request/response
examples, UX projections, transition plan, and acceptance matrix are in
[PR0 — Task coordination contract](../plans/pr0-task-coordination-contract.md).
