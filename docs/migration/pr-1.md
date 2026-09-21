# Plan PR1 — Implementation / Exit Ledger

## Status

Plan PR1 implementation complete and awaiting final review/merge.

Implementation branch: `feat/plan-pr1-actor-multi-role`.

This document is the implementation and exit ledger. The audit in
`docs/migration/plan-pr1-preflight.md` remains unchanged as the pre-implementation
inventory.

## Additive contract

Plan PR1 adds the following additive domain records and value types:

* `ActorRef::Human(user_id)` and `ActorRef::Agent(agent_id)`;
* Task-scoped `TaskRole`;
* multi-actor `RoleMembership`;
* `CoordinationMode` with `partitioned`, `collaborative`, and `independent`.

`RoleMembership` represents Task-scoped participation and eligibility. It is
not allocation, a WorkUnit, an Execution, a workspace, a branch, a scope, or a
harness session. Human and Agent references are ordinary peer Actor kinds; no
synthetic Human Agent is introduced.

## Authority

Once a `TaskRole` exists, `RoleMembership` is the current role-eligibility
authority. The legacy singleton values are not authoritative for:

* scheduling;
* Execution Actor selection;
* follow-up or re-execute Actor selection;
* workspace, terminal, or lease authority.

The concrete historical work principal remains on the Execution, and concrete
workspace authority remains on the matching Execution and `WorkspaceLease`.

Legacy singleton values may remain as bounded display and public compatibility
projections, or as pre-V088 fallback when no replacement `TaskRole` exists.

During the transition, an untouched pre-V088 task may still use its legacy
assignment as the bounded fallback. Backfilled or newly created TaskRoles have
the membership authority described above; the fallback is not consulted once
the replacement TaskRole exists.

## Old writer → new writer

The old writer surface consists of `task.assignee_type`, `task.assignee_id`,
`task_role_assignment`, and the legacy role-assignment service/API paths.

The semantic writer is TaskService mutation of `TaskRole` and
`RoleMembership`. Initial assignments, defaults, reassignment, membership
changes, and claim-established membership use the service Actor/Project
validation boundary and the transaction-aware membership mutation path.

Legacy entry points remain only where old internal or public surfaces still
exist. For a canonical TaskRole they translate into authoritative membership
changes and then derive the singleton projection. They do not create a second
source of truth. Execution labels such as `interactive`, `merge_fixer`, and
`system` remain outside the TaskRole vocabulary during this migration.

## Old reader → new reader

Authority-sensitive readers consume `TaskRole` and current active or suspended
`RoleMembership` records. Candidate selection filters authoritative Agent
memberships through the existing operational usability checks before choosing a
candidate; membership does not become active work by itself.

Legacy reads remain only for:

* display and projection compatibility;
* old public API, MCP, CLI, and UI shapes that have not yet moved to the target
  surface;
* bounded pre-V088 fallback when no replacement TaskRole exists;
* explicitly historical compatibility that does not reinterpret an old
  Execution.

## Compatibility direction

The only compatibility projection direction is:

```text
RoleMembership authority
        ↓
legacy singleton compatibility projection
```

This is not a bidirectional authority:

```text
legacy assignment ↔ RoleMembership
```

Legacy mutation surfaces may translate into a membership mutation, but the
resulting `task_role_assignment` and `task.assignee_*` values are derived from
the membership authority. A deterministic Agent-before-Human representative,
ordered by membership creation and id, is a compatibility display value only.

## Atomicity and divergence protection

Authoritative membership mutation and required compatibility projection
synchronization commit in one database transaction. A failed projection or
compatibility update rolls back the membership change. Scope-revocation sweeps
return deduplicated affected Task effects, and their `task.updated` events are
published only after the Project-scope transaction commits successfully. Task
and membership events are never emitted for a rolled-back sweep.

The transition is protected by:

* unique Task-local TaskRole identity;
* uniqueness for one Actor's current membership in a role;
* optimistic membership and TaskRole version checks;
* `active`, `suspended`, and `ended` lifecycle preservation;
* the coordination-mode guard before a legacy singleton TaskRole receives a
  second current member;
* explicit rejection of legacy singleton mutations that would collapse a
  multi-member role.

The legacy writer cannot independently overwrite a replacement TaskRole. Its
canonical path delegates to the same semantic authority and projection
transaction; its bounded fallback is permitted only when no replacement
TaskRole exists.

## Historical preservation

Historical Executions retain their existing persisted principal representation
and are never reinterpreted from current RoleMembership state.

Existing Agent-backed Executions preserve `execution.agent_id`. Human
RoleMembership is first-class in Plan PR1, while Execution persistence remains
transitional and does not yet carry the final ActorRef principal
representation. The existing `execution.agent_session_id` remains transitional
until its named later migration. Membership changes do not rewrite completed
or existing Execution principals.

## Workspace authority

RoleMembership alone grants no workspace authority. Concrete access remains
scoped to an eligible concrete Execution, its matching `WorkspaceLease`, and
the existing deterministic admission, renewal, recovery, terminal, and
governance checks. Adding another member to a role does not transfer a
workspace, lease, terminal, or repository write scope.

Scope revocation removes the Actor from new eligibility and admission; it does
not rewrite or cancel an existing Execution. Existing Execution and lease
lifecycle rules continue to govern already-running work.

## Human / Agent validity

Human and Agent are peer ActorRef kinds. A Human membership requires an
existing User who is either the Project owner or a Project member. The literal
`"human"` value remains a bounded legacy sentinel and never becomes
`ActorRef::Human("human")`.

TaskService applies one Project-scoped Agent validity rule before authoritative
membership creation. An existing Agent is valid for a Task's Project when it
is global, when it is account-visible and its `owner_id` is the Project owner
or an active/current Project member, or when it has the current active
`project_agent_binding` for that Project. This is identity/scope validity
only: paused, busy, offline, and other runtime states remain separate
scheduler usability questions. Requester authorization remains at the
API/service boundary that has requester context; it is not part of this Actor
validity predicate.

Project-scoped Actor validity is live rather than creation-time-only. When a
Human membership, account-Agent ownership scope, or Project Agent binding is
revoked, current TaskRole memberships that no longer have another valid Project
scope are ended transactionally and legacy compatibility projections are
rebuilt from surviving active memberships. Historical membership and
Execution records are preserved.

## Migration

`V088__task_roles_and_memberships.sql` is additive and data-preserving. It
creates the replacement TaskRole and RoleMembership persistence and records
contradictory legacy assignments for review rather than guessing. No
historical migration was rewritten, and no destructive schema removal was
performed.

## Compatibility debt intentionally retained

The following remain intentional migration debt rather than Plan PR1 failures:

* `task_role_assignment`;
* `task.assignee_type` and `task.assignee_id`;
* `execution.agent_session_id`;
* the current workflow state machine;
* the special review runtime;
* legacy role REST and other public surfaces.

## Exact future ownership

* Plan PR2 owns `ExecutionPurpose`, `HarnessSession`, and explicit
  execution/session continuity. It must reconcile the target
  `Execution.actor_ref` model without rewriting historical principal identity
  from current RoleMembership state.
* Plan PR5 owns `WorkUnit` and concrete isolated parallel allocation.
* Plan PR12 owns the final target REST, API, MCP, CLI, and UI surfaces.
* Plan PR13 owns removal of singular-assignment compatibility, remaining old
  readers/writers, and destructive schema cleanup after migration
  preconditions hold.

## Validation

The final closure pass deliberately did not run Cargo, Rust compiler,
rust-analyzer, frontend, lint, or runtime validation because of the explicit
compute-budget decision.

Focused regression coverage was added for the final authority-cutover
counterexamples:

* an account-scoped Agent outside the Task Project is rejected;
* an account-scoped Agent owned by a current Project member is accepted;
* a global Agent is accepted;
* an Agent with an active Project binding is accepted;
* a paused but Project-valid Agent is accepted as membership;
* an orchestration-only Agent is skipped by repository candidate selection;
* invalid explicit and default Actors fail before Task persistence;
* invalid or multi-member legacy mutations preserve running Executions.
* Project member removal and Project Agent binding replacement end only
  memberships that lose all valid Project scope, preserve alternate validity,
  rebuild projections, leave unrelated Projects untouched, and publish
  post-commit `task.updated` effects for affected Tasks.

The existing Plan PR1 authority-cutover regression tests remain in the branch
and were not executed in this pass. Focused tests were added for review; no
test result is claimed here.

## Exit state

Plan PR1 leaves the singular assignment records as bounded compatibility
projections and preserves the explicit PR2 boundary for execution purpose and
session continuity. Plan PR2 may begin only after Plan PR1 is reviewed and
merged.
