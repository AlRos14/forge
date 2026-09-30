# Plan PR6: Event-driven orchestrator role and policy

Status: implementation ledger for `feat/plan-pr6-event-driven-orchestrator`.

## Baseline and scope

- Main after PR5: `87a945215dc3f13318c7caa6f6b751013152e147` (PR5 commit `51b056f148c28700daca9e16d26af8b63a51d5eb` is an ancestor).
- PR5 migration head: V093.
- PR6 owns durable Task-scoped orchestrator wake admission, target selection, fresh `purpose=orchestrate` Executions, and a small typed action policy.
- PR6 does not replace Task lifecycle/workflow/Gates, planning, review/validation, Agent Host, Project OS, public API/UI, or historical schema cleanup.

## Invariants in scope

PR6 implements INV-001/004/005/006, 007/008/009/010, 017/018/019/020/021/022/023/024/025/026/029/030/031/032/033/034/035. Role membership remains eligibility, an Execution is one historical attempt, `purpose` does not grant permission, the HarnessAdapter is the only harness boundary, and domain mutations remain deterministic service authority. EventBus is a hint only.

## Legacy audit and compatibility ledger

| Legacy path | Classification | PR6 behavior / owner |
|---|---|---|
| `TaskDispatcher` 10-second workflow scan, Task lifecycle recovery | KEEP / DEFER PR9 | Continues transitions, planning, review, and legacy recovery. Its `dispatch_role_agent` branch must skip canonical `orchestrator`; it is no longer an orchestrator cognition authority. |
| Workflow `dispatch_role_agent` targeting planner/implementer/reviewer | KEEP / DEFER PR7/PR8/PR9 | Remains for those roles until their named cutovers. |
| Workflow `dispatch_role_agent` targeting canonical `orchestrator` | ADAPT IN PR6 | No initial or recovery dispatch through workflow role selection. Durable PR6 runtime owns orchestrator cognition. |
| `CoordinationOutcomeConsumer` | KEEP | Remains a Project OS/Agent inbox and commitment projection; it is not the Task orchestrator consumer. |
| `coordination_service`, `AgentAction`, Main/Project Agent orchestration | DEFER PR11 | Names overlap, authority does not. PR6 does not route TaskRole orchestrators through it. |
| `EmbeddedTaskExecutor` and Task Agent Host sessions | DEFER PR10 | PR6 must not use role-name-based Agent Host session continuity as HarnessSession continuity. Unsupported harness behavior is recorded and fails closed. |
| Legacy Task review and merge branches | DEFER PR8 / PR9 | Still owned by existing review and lifecycle services. PR6 does not make Proposal/Decision a review or merge trigger. |
| Legacy Task assignments and `task_role_assignment` | COMPATIBILITY until PR13 | Not an orchestrator eligibility source when canonical `TaskRole` exists. |
| PR5 WorkUnit cleanup compile blockers | ADAPT IN PR6 (compatibility repair) | Preserve cleanup behavior while converting the repo source to `Path` and mapping filesystem inspection errors into the existing durable fail-closed cleanup error path; release the lock guard before awaiting terminal cleanup. |
| Destructive schema cleanup | REMOVE PR13 | V094 and V095 are additive; V001–V093 remain unchanged. |

## Durable event audit and wake matrix

Only committed `domain_event` rows are admission authority. `domain_event.committed` on the broadcast bus can prompt an immediate SQLite claim; event payloads from EventBus are never dispatched directly. A periodic bounded claim is the recovery path after restart, no subscriber, or bus lag.

| Event type / entity | Durable today? Producer / scope | Target data and enrichment | PR6 decision |
|---|---|---|---|
| `execution.started` / `execution` | Yes; PR5 WorkUnit execution binding and PR6 Task-level Execution creation append the row atomically. Task scope. | Load exact Execution and verify Task, Actor, Purpose, and WorkUnit. | Wake for a meaningful non-orchestrator Execution start. `purpose=orchestrate` is intercepted and never becomes a generic wake. |
| `execution.completed`, `execution.failed`, `execution.cancelled`, `execution.stalled` / `execution` | Yes at PR6-touched local and remote runner, cancellation, and recovery mutation boundaries; Task scope. Status update and event append share a transaction. | Load exact Execution and verify event scope, Task, Actor, and Purpose. | Completed/failed/stalled non-orchestrator Executions can wake. Cancellation is durable but is not a generic worker wake. An orchestrator's exact terminal event reconciles its wake only when the current durable Execution status matches; it never wakes another orchestrator generically. |
| `work_unit.completed`, `cancelled`, dependency add/remove, allocation change, and integration success/conflict/failure / `work_unit` | Yes; PR5 WorkUnitService commits these events with the mutation. Task scope. | Load exact WorkUnit and same-Task dependencies/readiness; retain that WorkUnit ID only. | These signals may wake according to the current mode and exact assignment. `work_unit.created` is not a wake: bounded WorkUnit creation is itself a PR6 action, and waking peers from it would create an indirect orchestration loop. No sibling WorkUnit is attached to a WorkUnit-scoped wake. `work_unit.updated` is not a wake candidate. |
| `message.created` / `message` | Yes; CollaborationService. | Load exact Message to verify Task, target Actor/TaskRole ID/Task, and contextual WorkUnit. | Wake only the addressed active orchestrator membership under the captured mode policy. Task-targeted messages fan out only in collaborative mode. |
| `handoff.created`, `handoff.status_changed` / `handoff` | Yes; CollaborationService. | Load exact Handoff; its target is an Actor, TaskRole ID, or Task; verify contextual WorkUnit is in the same Task. | Each durable addressed Handoff event may wake its exact eligible target under the mode policy. It never changes membership. |
| `proposal.created`, `decision.recorded` / proposal or decision | Yes; CollaborationService. `proposal.withdrawn` is durable but not a wake candidate. | Load exact Proposal/Decision and its Task; resolution targets only the exact proposer Actor. | Open Proposal creation and Decision resolution may wake by TaskRole policy. Neither record executes an action. |
| `artifact.created` / `artifact` | Yes; CollaborationService. | Producer and Task lookup; artifact may be linked from a relevant Message/Handoff/Proposal. | No standalone wake; wake through its addressed collaboration record to avoid broad fanout. |
| `task.created`, `task.transitioned`, `task.status_changed`, `task.blocked`, `task.unblocked`, `task.failed` / `task` | V095 adds same-transaction triggers for Task creation and block/fail metadata changes; existing Task transition/status writers remain durable. | Verify entity and scope name the exact Task. Creation is a durable source record but does not wake by itself; first eligible membership has its own event. | Durable state changes may wake under current TaskRole targeting. EventBus-only Task payloads are not consumed. |
| `orchestrator.task_role_changed` / `task_role` | V095 trigger appends when an orchestrator coordination mode or policy changes while active membership exists. | Re-read exact TaskRole and require the event's TaskRole version. Partitioned mode carries an exact active Actor and assigned orchestrator WorkUnit. | Re-evaluate current policy and coordination targeting; stale-version events are receipted without a wake. |
| `orchestrator.membership_changed` / `role_membership` | V095 triggers append for canonical orchestrator membership insertion/reactivation/suspension/end and compatibility SQL writers. | Re-read exact membership and TaskRole; require the current membership version and active same-Task role. | Active membership targets its exact Actor. Inactive membership changes can target the Role under current mode rules, but cannot select an inactive Actor. |
| `orchestrator.bootstrap_reconciled` / `task_role` | V095 inserts one explicit current-state reconciliation event per eligible exact target after the existing consumer cursor. It is not a reconstructed historical Task event. | Uses current nonterminal Task and active orchestrator membership; independent requires a unique member, and partitioned requires an exact assigned orchestrator WorkUnit. | Materialize one ordinary durable wake through the same event receipt, targeting, policy, lease, capacity, and Execution path. |
| `review.status_changed` / `review` | Yes; ReviewRepo appends it transactionally with status changes. Task scope. | Load exact Review; require `status=failed`, exact Task, and its exact reviewer Execution for WorkUnit context. | Review failure may wake under the TaskRole mode. Broader review and ValidationRun replacement remains PR8. |
| Merge conflict, quota issue, free-form Human comments/input | Current producers are EventBus, legacy rows, or have no Task-scoped durable record. A Human Message is durable collaboration. | No durable exact target for these signals today. | Do not promote EventBus-only signals. Defer durable producer to PR9 (merge/lifecycle), PR11 (Agent Host), or the owning producer. Human Messages are supported. |
| `agent.wake.admitted` / `agent_wake` | Durable Attention wake admission, but uses Agent identity/scope cooldowns, not TaskRole and Execution identity. | Does not identify a TaskRole wake Execution. | Not reused as PR6 authority. |

## Durable authority model

The existing event cursor/lease/receipt is sufficient to claim and checkpoint a source event across processes, but cannot represent the required `event -> exact TaskRole -> Actor -> unique wake Execution` obligation or a retryable dispatch. PR6 therefore adds one additive V094 wake ledger plus a small action replay ledger. V095 adds exact TaskRole policy snapshots and durable bootstrap/activation events. One source event, Task, and Actor create at most one row, even if the exact TaskRole is later recreated. No in-memory debounce/coalescing is used.

The V094 schema preserves source event and sequence, TaskRole, captured `CoordinationMode`, exact Actor, optional exact WorkUnit, correlation/causation/depth, runtime policy version/digest, stable per-attempt Execution ID, dispatch state, attempts, lease, backoff, and error evidence. V095 adds the TaskRole's exact version and raw policy JSON snapshot. A uniqueness constraint covers `(event_id, task_id, actor_kind, actor_id)`; Execution identity is unique. Wake provenance cannot be edited or deleted, and referenced Task/Role/WorkUnit rows are retained while a wake refers to them. The V094 consumer cursor remains at its original migration-time high-water mark; V095 does not reinterpret historical events. Instead, its bounded set-based reconciliation reads only current nonterminal TaskRole/member/allocation rows and writes explicit typed bootstrap events after that mark. The migration and event dedupe keys make that cutover idempotent and restart-safe.

Consumer admission inserts all per-Actor wake obligations before completing the claimed source event. If it crashes between the insert and event receipt, replay hits the uniqueness constraint and then completes the receipt. Wake dispatch uses a single conditional SQLite `UPDATE ... RETURNING`; multiple Forge processes may see the bus hint, but only one can acquire the wake lease. Before dispatch, the consumer verifies that the captured mode still matches, reloads the durable cause, recomputes target eligibility, and confirms the exact Actor remains selected and active. Capacity and temporary unavailability update the same row to a retryable pending state with bounded backoff.

## Targeting and multiple orchestrators

Eligibility comes only from the exact same-Task `TaskRole(role='orchestrator')` and `RoleMembership(status='active')`. No legacy assignment or SQL row order selects an Actor. Cross-Task references, missing TaskRole, unknown mode with ambiguous membership, stale membership, malformed payload, and unresolved target fail closed.

| Mode | Untargeted Task event | Explicit Actor target | Explicit orchestrator Role target | WorkUnit context |
|---|---|---|---|---|
| `collaborative` | One wake for every active orchestrator member, including Humans as pending work. | Only that active same-Task member. | Every active member of that exact TaskRole. | Keep the single exact event WorkUnit; do not include siblings. |
| `partitioned` | Only when the event has a WorkUnit whose role is `orchestrator` and whose exact allocation names an active member. | Only that active same-Task member. | Exact allocated member when WorkUnit context proves it; without that context, allow only a unique active member. Ambiguous selection fails closed. | Load the exact WorkUnit and same-Task dependency evidence; do not infer from siblings. |
| `independent` | No wake. | Only that active same-Task member. | A unique active member only; no fanout. | Never infer from sibling state. |
| mode absent | Generic event does not wake. An explicit Actor target is still exact. | Exact active member. | Only when precisely one active member exists. | Exact same-Task WorkUnit only. |

Handoff and Message target resolution uses the durable collaboration row, not payload guesses. Role-targeted Handoff targets the exact `TaskRole` ID. The durable mode is recorded at admission and rechecked before dispatch; a mode change fails the existing wake instead of silently retargeting it. An addressed Message/Handoff from an orchestrator Execution may wake its explicit target, with self-target suppression; an orchestrator lifecycle event or an untargeted output from that Execution cannot produce a generic wake.

## Human and Agent behavior

Each admitted Human wake remains durable `awaiting_human` work. It creates no Execution, fake Agent, HarnessSession, or automatic dispatch. An Agent wake may dispatch only when that exact Agent is an active orchestrator member and currently has execution capacity. Capacity/status unavailability returns the same obligation to pending with a 15-second backoff. An unsupported adapter or sandbox fails explicitly. An ambiguous Start result becomes `uncertain` and is never blindly started again; a later terminal event for that exact Execution can reconcile it. Every dispatch and completed-action replay checks the stored runtime policy ref/version/digest plus exact TaskRole version/policy JSON; unsupported fields or versions and stale snapshots fail closed.

## Execution and session continuity

Each admitted Agent wake reserves a stable new Execution ID for each actual attempt and records exact Task, Actor, `role='orchestrator'`, `purpose='orchestrate'`, parent/cause provenance, durable source event, WorkUnit context, and policy identity. A retry after a proven Execution failure gets a new Execution; a crash before row creation reuses the reserved ID. Execution creation atomically creates and attaches a fresh pending HarnessSession for that exact Agent; PR6 always uses Harness Start and never infers Resume from role name, latest Execution, latest Agent session, or other heuristics. Orchestrator cognition has no WorkspaceLease and uses an isolated read-only scratch directory plus bounded Task context; it cannot mutate a worktree.

The context builder reads same-Task TaskRole/memberships, WorkUnits and readiness, relevant Executions, Artifacts, Messages, Handoffs, Proposals, Decisions, Task integration state, and the durable event cause. A WorkUnit wake receives that exact WorkUnit and its dependency evidence, not sibling scopes. Project OS and other Tasks are excluded.

## Action policy

The fixed PR6 v1 runtime policy reference/digest is recorded with each wake. In addition, the TaskRole snapshot schema is version 1: `{}` defaults to automatic Agent dispatch, all four typed actions, at most 16 actions per Execution, and at most four WorkUnit creations. The allowlist can only restrict actions and both numeric values can only lower those limits. PR6 rejects duplicate/unknown action names, unknown fields, malformed values, and unsupported schema versions at dispatch; valid unknown JSON is retained in storage so a future implementation can interpret it, but current PR6 never silently ignores it. Message, Handoff, bounded WorkUnit creation, and Proposal are the supported typed output actions. A Message communicates but grants no authority. A Handoff directs intent toward an existing TaskRole, Actor, or WorkUnit but never changes RoleMembership. An Execution may create at most four WorkUnits, each through `WorkUnitService`, with title ≤512 bytes, scope ≤4096 bytes, and role ≤128 bytes. A WorkUnit-scoped wake may create only a child of that exact WorkUnit; its Messages, Handoffs, and Proposals remain in that same WorkUnit scope. Stable action result IDs make retries idempotent, and created WorkUnits inherit the wake correlation and causation. Creation does not allocate a workspace or start an Execution. A Proposal and Decision record intent/outcome only.

V096 closes the cross-process policy TOCTOU at the effect transaction boundary.
The shared `domain_event` insert guard maps each reserved action result ID to
its exact action type and verifies the completed orchestrator Execution, its
current wake attempt, active membership, current TaskRole version/policy JSON/
coordination snapshot, same-Task effect, and exact `execution.started`
correlation/causation. The entity row and event are written by one SQLite
transaction, so a stale snapshot aborts both. That guard error maps to a
deterministic PR6 stale-action error and terminal reconciliation fails the old
wake; it is not a transient retry. Existing completed action replay remains
idempotent on `execution_id + action_index`, digest, and stable `result_id`.

`stop`, `cancel`, `reassign`, `discard`, `invalidate`, `merge`, and `override` are protected actions. PR6 may record a Proposal with exact target/version/digest and policy reference; it does not turn an approval Decision into an automatic mutation. Any later action executor must recheck the exact Proposal, Decision, target/version/digest, and policy identity. No model-selected exception can bypass the deterministic policy. No raw SQL, arbitrary Git command, or unbounded workspace write is exposed.

PR6 has no direct Harness steering, stopping, or resuming action. Messages and Handoffs preserve communication intent only; the implementation does not report them as native, emulated, queued, or successful live steering. A future typed steering operation must query the exact target Execution's HarnessAdapter capability and preserve each support level distinctly.

## Crash recovery and idempotency

| Crash point | Durable state and recovery |
|---|---|
| After source event claim | Event lease expires; same cursor position is claimed again. |
| After wake admission, before event receipt | Unique event/TaskRole/Actor key returns the existing obligation; source receipt then commits. |
| Before Execution row creation | The leased wake has one durable `reserved` attempt and Execution ID. After lease expiry, retry reuses that ID. |
| After Execution row creation, before recording `start_requested` | The exact row and `reserved` attempt are durable. Retry reads the same row and proceeds with its first Start request. |
| After recording `start_requested`, before the Harness call returns | The outcome is ambiguous, including a crash just before the call. Recovery marks the attempt `uncertain` and never issues a second Start for that Execution. If it remains running without activity, the existing stalled-Execution recovery emits a durable terminal event, which retries the wake with a new Execution. |
| After Harness Start is accepted, before the caller records `running` | The durable attempt is still `start_requested`; lease expiry makes the wake `uncertain` and does not issue another Start. The exact Execution's later terminal event can reconcile it. |
| After orchestrator action output, before action receipt/wake completion | `orchestrator_action` stores one UUIDv4 result ID per Execution/action index and verifies the action digest. Collaboration replay with that ID returns only the same existing record; mismatched output fails closed. |
| Before source/wake completion | The source event receipt and wake execution/action state are independently replayable; no source is acknowledged before admission is durable. |

Causation depth is preserved and bounded by the existing 0–16 ledger constraint. A wake is admitted only when there is room for its Execution-start event and one collaboration output within that ceiling. Classification excludes `purpose=orchestrate` lifecycle before depth is considered. Self Actor events are suppressed; only explicit targeted collaboration may cross between orchestrators. `1 event = 1 Actor wake` is the initial durable grouping rule.

## TaskDispatcher coexistence

`TaskDispatcher` continues its workflow scan for legacy Task lifecycle, planning, review, and recovery paths until PR9. PR6 prevents it from dispatching the canonical orchestrator role from either initial scheduling or active recovery. The new durable consumer admits source events and the wake dispatcher owns only orchestrator cognition. WorkflowEngine remains installed.

## Tests

Focused `pr6_` tests cover source-event replay/lease competition, event classification and orchestrator no-ping-pong, exact TaskRole/Actor/WorkUnit targeting, Human-vs-Agent dispatch, every coordination mode, capacity retry, fresh Execution/Start semantics, typed collaboration and WorkUnit action idempotency, protected-action fail-closed behavior, TaskDispatcher compatibility, V095 upgrade bootstrap including ambiguous independent targeting, new-Task/member activation, TaskRole policy snapshot/replay, and V096 atomic effect rollback for each typed action mapping. The service tests use the Harness adapter boundary with a recording executor; they do not prove external daemon, provider, or network behavior.

Baseline validation recorded before V095:

- `cargo test -p db pr6_ -- --nocapture`: PASS, 4 selected tests.
- `cargo test -p services pr6_ -- --nocapture`: PASS, 16 selected tests.
- `FORGE_SKIP_WEB_BUILD=1 cargo check -p forge-cli`: PASS; skipped the frontend build as documented by the repo.
- `cargo fmt --all -- --check`: FAIL because four untouched baseline files need formatting: `crates/db/tests/pr5_workunit.rs`, `crates/services/src/project_deletion.rs`, `crates/services/src/task_integration_operation.rs`, and `crates/services/src/terminal_service.rs`; the touched `workspace_cleanup.rs` also contains pre-existing formatting drift outside the small compile repair. PR6-added Rust code is formatted; unrelated hunks were left unchanged.
- Workspace/full suite, release build, and `cargo clean` were not run.

Final V095/P1/P2 validation on the PR6 branch (reusing `/home/alejandro/Proyectos/forge/target`):

- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services pr6_new_task_member_activation_is_durable_without_eventbus_authority -- --nocapture`: PASS, 1 test.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services pr6_repeated_task_block_metadata_changes_are_durable -- --nocapture`: PASS, 1 test.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services pr6_v095_bootstrap_reconciles_head_cursor_once_with_mode_exact_targets -- --nocapture`: PASS, 1 test.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services pr6_task_role_ -- --nocapture`: PASS, 2 tests.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services pr6_agent_capacity_retries_then_starts_fresh_execution_and_replays_typed_actions -- --nocapture`: PASS, 1 test.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p db pr6_wake_admission_is_deduplicated_and_lease_expiry_is_retryable -- --nocapture`: PASS, 1 test.
- Final filters: `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p db pr6_ -- --nocapture` PASS, 4 tests; `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services pr6_ -- --nocapture` PASS, 21 tests.
- `rustfmt --edition 2021 --check` on the eight changed Rust files: PASS. `git diff --check`: PASS.
- A first compile attempt exposed two missing test-fixture fields; those fixtures were corrected before the passing runs. The passing builds retain an unused-import warning in untouched `crates/services/src/task_integration_operation.rs:272`.
- Workspace/full suite, global check, release build, `cargo clean`, and all-workspace formatting were not run.

## Deferred work

- PR7 planning authority and legacy planning removal.
- PR8 ValidationRun/review replacement and full review-failure signal taxonomy.
- PR9 Task lifecycle/Gate replacement, atomic durable Task execution lifecycle events outside PR6's touched boundaries, and merge-conflict producers.
- PR10 Agent Host removal and its role-based embedded session behavior.
- PR11 Project OS/Main/Project Agent removal.
- PR12 public API/UI exposure of Human pending work and API cleanup.
- PR13 destructive schema cleanup.

## Exit criteria

Durable wakes are exact-targeted, retryable and cross-process safe; Agent wakes create one fresh exact orchestrate Execution through Harness authority; Human wakes remain explicit pending work; action records use durable collaboration and deterministic policy; TaskDispatcher no longer dispatches orchestrator cognition; focused PR6 tests and final diff review pass. PR6 is ready only as an independent-review candidate, never self-approved for merge.
