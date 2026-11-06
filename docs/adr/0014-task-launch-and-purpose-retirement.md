# ADR 0014: Task launch owns dispatch; Execution purpose is retired

Status: Accepted for implementation on 2026-11-05

## Context

The current runtime still requires a Human or Agent to create an Execution for
one already-allocated WorkUnit and to choose an `ExecutionPurpose`. That makes
the ordinary flow Task → WorkUnit → manual Execution configuration instead of
Task → prepare WorkUnits → launch Task. Purpose is also used as a proxy for
review, planning, and orchestration authority, even though the exact Role,
WorkUnit, Artifact, subject, and Wake records already carry that authority.

The product contract in
[plan-task-driven-workunits.md](../migration/plan-task-driven-workunits.md)
requires durable Task launch/pause/resume/cancel, automatic WorkUnit
admission, exact review subjects, bounded rework, and retirement of purpose
from public and active persistence.

## Decision

1. `TaskLifecycleService` remains the aggregate Task authority. A durable
   `task_dispatch` intent records `idle`, `running`, `paused`, or `cancelled`
   with a version and an immutable idempotency receipt. Launch, pause, resume,
   and cancel requests bind the expected Task and TaskLifecycle versions.
2. The existing `TaskDispatcher` performs automatic admission. It reads the
   running dispatch intent, derives readiness from current WorkUnit,
   dependency, assignment, Workspace, Execution, and integration facts, and
   admits only exact assigned Agent WorkUnits. It never allocates a Role,
   selects an arbitrary member, or creates WorkUnits.
3. A Human-assigned WorkUnit is pending human work. It creates no synthetic
   CLI, HarnessSession, or Execution.
4. Executions remain the immutable historical attempt records. Admission,
   provider-start receipts, HarnessSessions, WorkspaceLeases, usage, outputs,
   and lineage remain attached to the exact Execution.
5. `ExecutionPurpose` is not a public contract or runtime classification.
   Historical values are preserved in
   `pr13_execution_purpose_history`; the active column and purpose-dependent
   triggers are removed by a forward migration. Formal review is identified
   by the `reviewer` Role and its exact ReviewReport output. Planning and
   orchestration are ordinary WorkUnit work or the `orchestrator` Role bound to
   its exact Wake.
6. Dynamic rework is one idempotent operation over an exact
   request-changes ReviewReport. It creates a correction WorkUnit and a new
   reviewer WorkUnit in one transaction, records the exact consumed result and
   review subject, consumes a bounded per-Task budget atomically, and leaves
   external dispatch to the dispatcher.
7. A completed Execution does not silently complete a WorkUnit. An open
   WorkUnit with a completed result is `awaiting_acceptance`; a new automatic
   attempt is not admitted until the result is accepted or the WorkUnit is
   explicitly cancelled/reassigned.
8. Public REST, MCP, CLI, and web surfaces expose Task dispatch operations.
   Manual Execution creation returns an explicit retirement error. No silent
   alias or second launch contract is retained.

## Consequences

- A prepared Task progresses without manual Execution creation.
- Purpose-dependent queries, DTOs, events, and triggers must be removed or
  rewritten in the same change; historical purpose is not authority.
- Review and correction keep exact provenance and independence checks without
  a new workflow DSL.
- Pause prevents new admissions and provider starts; it does not claim to stop
  work already started. Cancel records the request and observes exact results.
- An uncertain provider outcome still cannot authorize a second Start.

## Migration

V141 adds dispatch intent and receipts. V142 preserves historical purpose
values and removes the active column and purpose-dependent triggers. V143 adds
rework receipts and result/review subject relations. V144 adds optional
WorkUnit harness instructions. V145 closes Task pause and per-Task concurrency
races inside Execution admission. Existing Executions, sessions, leases, outputs,
and receipts retain their IDs and relationships. Project teardown explicitly
removes the new mutable records while preserving historical purpose evidence.
