# Plan PR2 — HEAD-era execution/session preflight

## Baseline

This audit was performed on the merged Plan PR1 baseline:

| Item | Value |
| --- | --- |
| Branch | `feat/plan-pr2-execution-session` |
| HEAD | `87795825b3cdb93098502d35fd9291a519c61b9b` |
| Parent | Merge of Plan PR1, `f47667d691e58270dcda893cff09c7a7f8a32790` |
| Migration head | `V088__task_roles_and_memberships.sql` |
| Existing untracked state | `.recovery-pr0a/` preserved and not inspected as product source |

The audit is deliberately limited to Plan PR2. It does not redesign the
executor facade, workflow cognition, review runtime, WorkUnit model, or the
embedded Agent Runtime.

## Current Execution persistence

`execution` currently persists `agent_id`, `role`, `parent_execution_id`, and
`agent_session_id`. `CreateExecution` and `UpdateExecution` expose those legacy
fields. `ExecutionRepo::create` inserts through the transaction-aware
`create_execution_in_tx`; `ExecutionRepo::update` writes mutable terminal and
telemetry fields. `executor_config_snapshot_json` is already historical data
and remains independent of the new HarnessSession snapshot.

## Production writer inventory

Every production `CreateExecution` path was classified before implementation:

| Path | Classification | PR2 treatment |
| --- | --- | --- |
| `task_service/claim.rs` | A: claim creates the initial execution | Explicit ActorRef and Purpose; pending HarnessSession for Agent work where materialized |
| `task_service/execution/launch.rs` | A: manual, workflow, interactive, and resume launches | Explicit semantic purpose; explicit session continuity only |
| `task_service/execution/follow_up.rs` | A: review/rework follow-up | Select Actor first; reuse only an explicit compatible session |
| `task_service/execution/recovery.rs` | A/C: blocked recovery and re-execution | Use the referenced blocked/parent execution, not latest role data |
| `task_service/execution/cascade.rs` | A: cascade/subtask propagation | Default new/no session unless continuation is explicit |
| `task_service/config.rs` | A: failed execution records | Explicit principal/purpose for the failed historical record |
| `review/src/runner.rs` | A: reviewer/auditor execution records | Review purpose; no role-derived global purpose |
| `merge_service.rs`, `agent_service.rs`, `memory.rs`, `shutdown.rs`, `db/sqlite/analytics.rs` | F: test-only execution fixtures | Updated struct shape; not production creation authority |

The shared DB input is extended so production writers cannot omit the target
principal or purpose without an explicit compatibility helper. Test fixtures
are not treated as production authority.

## Session reader/writer inventory

### A — creates a new Execution

The service launch, claim, follow-up, recovery, cascade, failed-record, and
review paths listed above create Executions. These are all required to supply
ActorRef and Purpose. Session creation is centralized in the TaskService
execution-start path so retrying an Execution with an existing reference does
not create another generic session. The embedded/operational paths in the
table are test fixtures, not production creation authority.

### B — writes external harness session identity

The task execution runner (`task_service/execution/runner.rs`), review/auditor
runner (`review/src/runner.rs`), and remote daemon terminal notification path
(`task_service.rs`) consume `ExecutionResult.agent_session_id` or terminal
notification session data. The value is an external harness-native
identifier. PR2 writes it first to `harness_session.external_session_id` and
then projects it to the legacy `execution.agent_session_id` field in the same
transaction where practical.

CLI adapters continue to emit the external value. They do not know the generic
HarnessSession primary key.

### C — selects a previous session for resume

The current selectors are in:

- `task_service/action_resolver.rs` and `actions.rs` resumability checks;
- `task_service/execution/follow_up.rs`;
- `task_service/execution/launch.rs`;
- `task_service/execution/cascade.rs`;
- `task_service/execution/recovery.rs`;
- review configuration/dispatch paths that pass the executor thread ID.

The workflow dispatch loader still selects a latest terminal execution for
causal prompt context when a legacy workflow policy requests it. That result
is lineage/context only; follow-up creation performs the Actor-first,
explicit-HarnessSession check and the loader never selects a session.

PR2 changes new-session selection to use `Execution.harness_session_id` and
the resolved HarnessSession. A legacy `execution.agent_session_id` fallback is
retained only for pre-authority historical Agent rows with an exact legacy
Agent id and no generic reference; agentless or ambiguous history fails closed.
The fallback is marked for PR13 removal. Role/latest-execution lookup is not a
new authority.

### D — merely displays session data

Execution API responses, daemon terminal notifications, operator status,
execution logs, MCP projections, and web-generated bindings expose or carry
legacy session data. PR2 adds the new additive fields where execution details
are already exposed and retains the old fields for compatibility.

### E — embedded Agent Runtime / Agent Host

`AgentSessionRepo`, `agent_session`, `agent_context_scope`,
`protected_agent_session_state`, `protected_interaction`, context manifests,
`embedded_agent_service`, `embedded_task_executor`, `agent_chat_turn_worker`,
and `agent-host` remain a separate legacy runtime vertical. Their session is
not the generic HarnessSession authority. PR10 owns behavioral
removal/extraction; PR13 owns remaining schema cleanup.

### F — historical compatibility

Existing `agent_id`, `agent_session_id`, executor snapshot, workflow-role,
review, and task-assignment readers remain only where needed to preserve old
rows or public shapes. New code documents the direction of every projection;
it does not make legacy fields bidirectional authority.

## Required PR2 invariants

1. New Executions persist a real `ActorRef` and an explicit `ExecutionPurpose`.
2. Human Executions persist a real user ID, no Agent, and no HarnessSession.
3. Agent continuity uses `Execution.harness_session_id` → HarnessSession →
   external session ID.
4. Same role, Task, model, or latest Execution never independently selects a
   session.
5. Agent, harness identity, profile snapshot, and capability snapshot remain
   historical and immutable at the HarnessSession boundary.
6. V062 `agent_session` remains embedded-runtime-only compatibility.

## Preflight migration finding and resolution

The HEAD-era preflight found four invalid UPDATE target aliases in the Human
and Agent delete-projection triggers in V088. The four target aliases and
their correlated references were repaired on `main` in isolated commit
`6682acd` (`fix: repair V088 SQLite delete projection triggers`) before PR2 was
rebased. The repaired statements retain their selection order and projection
conditions. A static search found no remaining `UPDATE ... AS ...` target
aliases or references to the removed `legacy`/`current_task` aliases in V088.
The `sqlite3` command-line tool was unavailable, so no SQL execution check was
performed. V088 is no longer a known migration-chain blocker; implementation
still requires the normal migration and runtime verification gate.

## Second-pass implementation review corrections

The implementation review found and closed several gaps that were not safe to
leave implicit:

* the reserved legacy `agent_id = 'human'` value is excluded from ActorRef and
  HarnessSession backfill, recorded as unresolved history, rejected for new
  writes, and excluded from bounded resume fallback;
* pending HarnessSessions cannot be advertised or reused as active continuity,
  and the repository status update cannot establish external identity;
* executor result callbacks are the only generic external-session authority;
  Human callbacks fail closed and empty identities are rejected;
* re-execute records causal `parent_execution_id`, failed claim records retain
  the semantic role/purpose, and role follow-up compares the current Task
  workspace before reusing a session;
* MCP execution projections expose the additive PR2 authority fields.
* current Task workspace scope is used by action, recovery, cascade, review,
  and manual-stop resumability checks; the generic session retains its
  historical workspace token even if operational workspace reset deletes the
  corresponding `workspace` row;
* current non-session executor families (`shell`, `gemini`, and `null`) do not
  receive fabricated pending sessions. Unknown harness kinds remain opaque and
  can materialize a session only when a result supplies an external identity;
* HarnessSession predecessor identity and the legacy
  `execution.agent_session_id` projection are guarded against cross-identity
  or divergent direct writes.
* cancellation preserves the execution snapshot whenever an explicit
  HarnessSession is attached, and follow-up/workflow-guard continuations use
  only the current Task workspace rather than a stale lineage workspace.
* manual session follow-up passes the selected Execution as lineage only;
  current RoleMembership selects the new Actor before continuity is tested, so
  an Agent change cannot inherit the previous Actor's session.
* blocked-session recovery and workflow-guard continuations now apply the same
  Actor-first rule. A changed current RoleMembership Agent receives a fresh
  snapshot and no inherited HarnessSession; only a matching Actor may reuse
  the blocked/completed Execution's explicit continuity.

The second-pass authority review also made these session decisions explicit:

* `resumable_external_session` checks the historical ambiguity issue table
  before allowing either generic or bounded legacy continuity. It requires a
  persisted `ActorRef::Agent` matching the legacy Agent id, a non-sentinel id,
  non-empty external id, compatible workspace, and matching expected Agent.
* Task action resolution receives Execution ids already validated by that
  common service helper. It no longer makes its own decision from the legacy
  projection, so ambiguous rows cannot advertise `SessionFollowUp` or
  `WorkflowResume`.
* Follow-up and re-execute historical materialization also request the
  external identity from that helper before binding it. Cascade, blocked
  recovery, review continuation, and recovery annotations already use the
  helper, so the ambiguity rule is shared rather than repeated per caller.
  Full/light task response projections also suppress stale `ResumeSession`
  hints when the target Execution fails that authority check; stored history is
  not rewritten.
* Routed pending creation is delayed when `routing.selected_candidate_key` is
  absent. The local runner and remote terminal writer persist the resolved
  route snapshot and external session result in one Execution update. The DB
  writes that snapshot first inside its transaction, then materializes or
  activates HarnessSession continuity from the resulting snapshot and projects
  the legacy id before commit. This captures both cross-harness and
  same-harness account/profile fallback without mutating established session
  history. If an external id arrives while the route winner is still absent,
  the DB rejects the result rather than binding it to the primary candidate.
