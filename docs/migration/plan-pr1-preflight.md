# Plan PR1 preflight: Actor and multi-actor TaskRole migration

Status: audit only. No Plan PR1 schema, writer, reader, scheduler, execution,
HarnessSession, or authority implementation is included in Plan PR0.

This preflight records the migration surface found in the current transitional
repository so Plan PR1 can add the replacement representation deliberately.
It does not make the current singular assignment model authoritative for the
target architecture.

## Target boundary

Plan PR1 will add the target participation model:

~~~text
TaskRole {
  task_id
  role
  coordination_mode
  policy
  version
}

RoleMembership {
  task_role_id
  actor_ref
  status
  created_at
  ended_at?
}
~~~

`RoleMembership` answers only which Actors participate in a TaskRole. It does
not carry a WorkUnit, implementation scope, workspace, active Execution, or
current assignment. Work allocation remains a WorkUnit/Execution concern.
One Actor may hold several roles, and one role may have several Human and
Agent members.

## Persistence surface

The following records are migration inputs, not target truth:

| Current record | Current location | Plan PR1 concern |
| --- | --- | --- |
| Task-level assignee | `task.assignee_type`, `task.assignee_id`; introduced and rewritten by `V002`, `V010`, `V012`, `V021`, `V034`, and later migrations | Legacy fallback/display and some authority paths. Must not become the multi-actor membership table. |
| Singular role assignment | `task_role_assignment` from `V009`, normalized by `V012` and later rewrites | Current one-row-per-task/role authority. New membership data must be additive and its authority transition explicit. |
| Agent identity/profile | `agent_identity`, `agent_profile`, `agent_current` from `V059`; repository/model access in `crates/db/src/sqlite/agent.rs` and `crates/db/src/models.rs` | Agent identity remains harness/account-bound. A Human reference cannot be forced through the Agent tables. |
| Execution principal | `execution.agent_id`, `execution.role`, `execution.agent_session_id` from `V001`/`V021` and current row mappers | Historical Execution ownership must remain immutable. Plan PR1 must not infer a new Actor from a role name. |
| Embedded session/context | `agent_session` and related context tables from `V062`, plus legacy execution session fields | These remain compatibility inputs until Plan PR2 makes HarnessSession explicit. |
| Workspace authority | `workspace_lease` and governance records from `V076`/`V077`, plus the additive Plan PR0A authority correction | Plan PR1 must preserve deterministic lease checks while replacing assignment lookup. |

No Plan PR1 migration should edit historical migrations. The replacement must
be a new numbered, data-preserving migration after the then-current head.

## Current writers

The current code creates or changes singular role ownership in these paths:

* `crates/services/src/task_service/create.rs` — validates and creates
  initial role assignments and applies project defaults.
* `crates/services/src/task_service/roles.rs` — reassigns, removes, and
  reports current role assignments, including workspace-reset behavior.
* `crates/services/src/task_service/claim.rs` and
  `crates/services/src/task_service/actions.rs` — claims and action paths
  resolve a single effective role/assignee.
* `crates/services/src/task_service/transition.rs` and
  `crates/services/src/workflow/actions/` — lifecycle hooks resolve role
  assignment during transitions and gates.
* `crates/services/src/task_dispatcher/initial_scheduling.rs` and
  `active_recovery.rs` — scheduler and recovery choose one assignment.
* `crates/services/src/task_service/execution/launch.rs`, `follow_up.rs`,
  `recovery.rs`, and `cascade.rs` — execution selection, continuation, and
  cascade logic consume the current role assignment.
* `crates/services/src/agent_service.rs` and
  `integration_service.rs` — agent/default-assignment setup can write role
  rows.
* `crates/api/src/routes/tasks/crud.rs` and `tasks/roles.rs` — Task create and
  role assignment endpoints write the singular public model.
* `crates/api/src/routes/coordination.rs` and `routes/projects.rs` — project
  coordination/default role configuration can materialize assignments.
* `crates/mcp-server/src/tools/handlers.rs` and `crates/forge-client/src/`
  task/run callers — MCP and CLI requests carry the current assignment shape.
* `web/src/api/hooks.ts`, `web/src/components/task-controls.tsx`,
  `web/src/components/task-detail/`, and `web/src/pages/task-detail/` — the
  UI submits one assignee per role.

Every writer must be re-searched at the Plan PR1 head before adding a new
writer. The new writer's authority and any temporary dual-write direction
must be named in the Plan PR1 PR description.

## Current readers

The singular model is read for different purposes; these uses must not be
treated as one migration problem:

### Display and projection

* `crates/api/src/routes/mod.rs` maps task assignments into Task responses.
* `web/src/pages/task-detail/`, `web/src/components/task-detail/`,
  `web/src/features/board/`, and `web/src/api/` render role assignees.
* MCP task inspection and CLI task views expose the same one-row shape.
* SSE/event projections include task assignee fields.

### Scheduling and execution selection

* `crates/services/src/workflow/mod.rs` resolves an effective role from the
  current workflow state.
* `crates/services/src/workflow/actions/dispatch.rs`, `gates.rs`,
  `review.rs`, and `lifecycle.rs` select or check one role assignment.
* `crates/services/src/task_dispatcher/initial_scheduling.rs` and
  `active_recovery.rs` resolve a candidate Agent.
* `crates/services/src/task_service/execution/launch.rs`, `follow_up.rs`,
  `recovery.rs`, and `cascade.rs` determine whether an Execution matches the
  current role.

### Authority and workspace safety

* `crates/services/src/task_service/governance.rs` verifies assignment and
  lease admission.
* `crates/db/migrations/V076__project_charter_milestones_media.sql`,
  `V077__orchestration_runtime_repairs.sql`, and the additive Plan PR0A lease
  migration enforce SQL-side assignment/authority predicates.
* `crates/services/src/task_service/execution/guards.rs` and recovery paths
  protect launch and restart authority.
* `crates/services/src/terminal_service.rs` and coordination routes check
  role ownership before granting operational access.

### Historical continuity

* `execution.agent_id` and `agent_session_id` are used by execution detail,
  follow-up, recovery, logs, usage, and task diagnostics.
* `crates/services/src/task_service/action_resolver.rs` exposes resume and
  re-execute actions based on the current legacy role.

The key distinction for Plan PR1 is that display membership, scheduling
selection, workspace authority, and historical Execution ownership currently
share singular data but must become separate questions in the target model.

## Plan PR1 authority audit

| Current use | Current authority | Migration treatment |
| --- | --- | --- |
| Task/role display | `task` plus `task_role_assignment` projections | Replace the projection with membership sets; retain a singular summary only if explicitly derived for a legacy surface. |
| Scheduler candidate | effective workflow role plus assignment | Read eligible RoleMemberships and let later scheduling policy choose an Actor; do not make membership equal active work. |
| Execution selection | current role assignment, with legacy Task fallback | New Execution Actor must be selected explicitly; historical Actor cannot be replaced by latest role lookup. |
| Workspace lease | deterministic SQL/service predicates | Preserve core-enforced authority and update the assignment input only after replacement data is authoritative. |
| Resume/re-execute | historical Execution/session plus current legacy role | Remains transitional in Plan PR1; exact HarnessSession continuity is Plan PR2. Reassignment regression is carried forward, not solved by another singular lookup. |
| Human participation | current `assignee_type=user` or user callback paths | Map Human to `ActorRef::Human` in the new membership model; do not create a special Human domain path. |

## Public surfaces and tests

The Plan PR1 public-surface audit must include:

* REST handlers and router entries under `crates/api/src/routes/`, request and
  response types under `crates/api-types/src/`, generated bindings under
  `crates/api-types/bindings/` and `web/src/types/generated/`, and `docs/api.md`.
* CLI request/response code under `crates/forge-client/src/` and any
  `forge-ctl` role/assignment command.
* MCP descriptors, parameter types, handlers, and tests under
  `crates/mcp-server/src/`.
* SSE event contexts and task projections under `crates/events/` and
  `crates/api/src/routes/events.rs`.
* Task detail, board, role picker, execution, and agent UI under `web/src/`.

Fixtures and tests assuming one Actor per role include the TaskService role
cases under `crates/services/src/task_service/tests/`, workflow and dispatcher
tests under `crates/services/src/workflow/` and
`crates/services/src/task_dispatcher/`, DB migration/lease tests under
`crates/db/`, API task and execution tests under `crates/api/tests/`, MCP tests,
and frontend task-detail/board tests. Re-run the exact search at the Plan PR1
head and add multi-member/Human parity cases before changing readers.

## Proposed additive migration boundary

Plan PR1 should document this transition before implementation:

1. **Old writer:** singular Task/TaskRoleAssignment writers listed above.
   **New writer:** TaskRole/RoleMembership repository/service APIs.
2. **Authoritative source:** initially the existing singular records for
   untouched runtime behavior; once new membership writes and backfill are
   proven, explicitly switch authority to RoleMembership. Do not leave this
   implicit.
3. **Dual-write:** only if required by a still-live legacy reader. The
   direction must be new membership → bounded legacy projection, with one
   documented compatibility owner and divergence tests. Avoid writing the
   legacy row as an independent source of truth.
4. **Reader transition:** migrate display and non-authority readers first,
   then scheduler/execution selection, then lease/recovery authority readers.
   A legacy reader must name its removal Plan PR.
5. **Divergence protection:** optimistic versions, membership uniqueness,
   Task-local foreign keys, explicit Actor kind, and tests for multi-member,
   cross-role, removal, and reassignment cases.
6. **Rollback/data preservation:** backfill is repeatable and non-destructive;
   historical Executions retain their Actor/agent reference; failed migration
   leaves legacy runtime usable. No WorkUnit or HarnessSession is introduced
   as part of this preflight.
7. **Cleanup:** stop legacy writes, stop legacy reads, remove public aliases
   and compatibility conversion, then drop singular persistence only in the
   final cleanup Plan PR after all readers/writers move.

## Explicit non-goals

This preflight does not create ActorRef persistence, RoleMembership tables,
dual writes, scheduler changes, Execution ownership changes, HarnessSession
storage, WorkUnits, or a new authorization abstraction. It is the audit
boundary immediately before Plan PR1.
