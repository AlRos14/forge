# Architecture

Forge is a Rust workspace (12 crates) plus a React/TypeScript frontend. This
doc explains the crate layout, the task state machine, the database, and the
event bus. For runtime configuration see [getting-started.md](getting-started.md);
for the HTTP surface see [api.md](api.md).

## Crate layout

```
crates/
├── forge-cli/     # Binary entrypoint, server startup, CLI commands
├── forge-client/  # forge-ctl CLI client
├── forge-daemon/  # Local daemon detection and reporting
├── api/           # Axum REST endpoints, SSE, middleware
├── api-types/     # Shared request/response types (zero internal deps)
├── db/            # SQLite schema, migrations, repository implementations
├── services/      # Business logic (task state machine, workflow engine)
├── executors/     # TaskExecutor trait, Shell executor, JSONL logging
├── cli-adapters/  # Codex, Claude, Cursor, Gemini, opencode, shell, null adapters
├── workspace/     # Git worktree lifecycle, locking, path guardrails
├── git/           # Low-level git operations
├── review/        # CI runner, auditor orchestration
├── events/        # In-memory event bus (tokio broadcast)
├── mcp-server/    # MCP JSON-RPC tools for agent integration
└── config/        # Configuration loading, defaults
```

### Dependency flow

```
forge-cli → api → services → db
                → events      ↑
          → mcp-server -------┘
          → executors (log schema, shell executor)
          → workspace → git
          → config
          → api-types (shared request/response types, zero internal deps)
```

## Architectural patterns

### Repository trait pattern

The `db` crate defines async traits (`TaskRepo`, `AgentRepo`, …) in
`repository.rs` and implements them all on a single `SqliteDb` struct in
`sqlite.rs`. Services and routes call trait methods as
`TaskRepo::create(&*state.db, ...)`.

### Error propagation chain

`DbError` (db) → `ServiceError` (services) → `ApiError` (api). The api crate's
`errors.rs` maps domain errors to HTTP status codes. All errors render as
`ErrorResponse { code, message, details, request_id }`.

### AppState wiring

`forge-cli/main.rs` creates `Arc<SqliteDb>` and `Arc<EventBus>`, passes them to
`AppState::new()` which constructs `TaskService` and `AgentService` internally.
`AppState` is `Clone` (all fields are `Arc`) and used as Axum state.

### HTTP shell and web assets

The API router also serves the built React application with an SPA fallback.
Hashed JavaScript and CSS assets receive immutable one-year cache headers and
eligible responses are Brotli/gzip compressed; HTML navigation responses remain
uncached so deployments pick up the current asset graph. The production client
keeps route screens and editor-backed dialogs behind dynamic import boundaries.

### Event bus

The `events` crate wraps `tokio::sync::broadcast`. Services publish
`ForgeEvent` on state changes; the SSE endpoint at `/api/v1/events` subscribes
and streams them to web clients and other listeners.

### Memory Layer

The memory layer is a read-only retrieval index over execution summaries,
reviews, comments, failure-bearing transition logs, and conversations. It
stores attributed memory items separately from the source records, with every
result carrying the memory id, source type, source id, project id, optional task
id, creation time, and creator attribution.

Retrieval is layered: callers request a `layer` value in the `0-255` contract
space, and the first tranche implements layers `1`, `2`, and `3` with
`token_budget` mapping for cheap-first disclosure. REST and MCP both call
`MemoryService` for search/get behavior. MCP responses add an explicit
injection guardrail that frames retrieved bodies as background context, not
agent instructions or directives. Raw execution JSONL log payloads are not
indexed or returned by the memory layer.

### Daemon command transport

Linked daemons keep a WebSocket command stream open at
`/api/v1/daemons/{id}/connect`. The API server routes filesystem requests
(`fs.list`, `fs.branches`) and daemon-owned managed executions
(`execution.start`, `execution.cancel`) over that stream. The daemon validates
paths against its advertised workspace root, runs the local CLI adapter, streams
execution logs back as `execution.log` notifications, and reports final status
through `execution.terminal`.

Managed execution dispatch currently assumes the server-created task worktree
exists at the same absolute path on the daemon host. That covers local daemons
and containers or hosts with a shared workspace mount. A daemon on a separate
filesystem can still browse paths under its own `--workspace-root`, but
`execution.start` rejects server-only worktree paths until Forge has a remote
workspace sync or git handoff path.

### Daemon lifecycle and execution recovery

Remote daemons periodically report local CLI availability and, when connected
over the command stream, their currently running managed execution ids via
`POST /api/v1/daemons/{id}/report` (`active_execution_ids`). The server uses
that snapshot to reconcile orphaned server-side `running` executions owned by
the daemon: any execution older than 60 seconds that is missing from the report
is failed with `stop_reason = daemon_disconnected` and manual recovery only.

Separately, the server `HeartbeatMonitor` (10s tick) watches remote executions
whose owning daemon has no live WebSocket connection. After a 120s grace period
from the first observed disconnect, it fails the execution with
`daemon_disconnected`, publishes `execution.daemon_disconnected`, and emits the
same `reconciliation.event` used for stalled executions so tasks enter the
blocked/recovery UX. Embedded-server executions are excluded from the
disconnect check; only embedded-owned stalled executions are cancelled via the
in-process executor.

If a remote execution keeps running but stops emitting activity, the existing
stall detector still fails it after `execution_stall_timeout` (default 300s)
with `stop_reason = execution_stalled`.

### Task terminal sessions

Task terminal sessions are a separate API and daemon path for interactive shell
access to an existing task worktree. They do not layer onto
`TaskService.transition()` or the workflow engine. Creating a terminal does not
claim, transition, reset, or launch the task.

The browser connects only to the API server in v1. REST calls create sessions
and issue short-lived attach tokens; the browser then upgrades to
`/api/v1/terminals/{id}/ws?attach_token=...`. There is no direct
browser-to-daemon connection. For daemon-owned workspaces, the API server
proxies terminal operations over the existing daemon transport:
`terminal.start`, `terminal.input`, `terminal.resize`, and
`terminal.terminate` requests flow to the daemon, while `terminal.output` and
`terminal.exited` notifications flow back to the server. Embedded server mode
uses the same service path and also runs a local PTY-backed process; it does
not use plain stdin/stdout pipes.

Process ownership lives on the daemon side for daemon-owned workspaces and on
the API server for embedded workspaces. The API treats a task as daemon-owned
when the task is directly assigned to an agent with `daemon_id`, or when the
current workflow state's effective role assignment points to an agent with
`daemon_id`; otherwise it uses embedded server process handling. Both runtimes
allocate a PTY, start the shell in the server-authorized worktree, forward input
and output, apply resizes, and terminate the process. Daemon-side starts
additionally reject workspace paths that escape the daemon workspace root.

The API server persists lifecycle metadata in `task_terminal_session`, including
task, workspace, daemon, dimensions, status, timestamps, creator, and exit
metadata. Attach tokens are stored only in memory and are single-use. Reconnect
scrollback is an in-memory bounded ring buffer per running session, capped by
`terminal.reconnect_scrollback_bytes`, and is dropped once all browser clients
detach from that session; full terminal transcripts are not persisted in v1.

Terminal sessions and managed Forge executions cannot run concurrently in the
same workspace. Terminal creation is blocked while a managed execution is
active, and managed execution startup must reject or defer while a terminal is
active for that workspace.

Cleanup is time- and ownership-bound. The default idle timeout is 30 minutes
(`terminal.idle_timeout_secs = 1800`) and the default absolute lifetime is
8 hours (`terminal.max_lifetime_secs = 28800`). Workspace cleanup terminates
running sessions before removing the worktree. If a daemon disconnects beyond
the heartbeat cleanup threshold, the daemon kills the terminals it owns and the
server records the sessions as exited, timed out, orphaned, or cleanup
terminated when it observes the terminal lifecycle event.

## Task state machine

```
todo ──────────────► in_progress ──────► review ──────► merging ──────► done
 │                      │                  │              │
 └──► cancelled ◄───────┴──────────────────┴──────────────┘
                                           │
                                      merge_failed ──► blocked
```

All non-terminal states can transition to `cancelled`. Terminal states: `done`,
`cancelled`. The default workflow lives in
`crates/services/src/workflow/default_workflow.rs` with sequence
`backlog → todo → planning → in_progress → review → merging → done` and
`merge_failed`, `blocked`, `cancelled` as auxiliary/failure/terminal states.

### Workflow engine (in progress)

Flexible workflow work is partially implemented. `WorkflowEngine` in
`crates/services/src/workflow/engine/mod.rs` is the new data-driven path;
`TaskService.transition()` still uses the legacy `TaskStatus`/`transition_allowed`
path. Treat the engine as a parallel code path until the split is removed.

Workflows are project-defined JSON in `project.workflow_definition`. Empty
string or `"{}"` resolves at runtime to the built-in `DefaultWorkflow`.
`WorkflowCache` caches resolved definitions per project and invalidates on
workflow updates.

The applicable `WorkflowDefinition` is resolved **exactly once** per transition
entry via `WorkflowEngine::resolve_workflow_for_task`, keyed on whether the task
is a root or subtask, whether its **current** state belongs to the inherited
subtask workflow, and the acting party (`triggered_by`). The result is passed
into wrapper pre-checks, `WorkflowEngine::transition` / `transition_inner`,
and `HookContext` so hook actions, cascades, and advance steps consume the same
definition — no downstream layer re-resolves for that transition. Each nested
transition entry (for example a system cascade step) calls the same function at
its own entry, which is correct because resolution keys on current-state
membership, not on who started the original user move.

| Task | Current state | Actor | Applicable workflow |
| --- | --- | --- | --- |
| Root | any | any | Project |
| Subtask | Not in inherited subtask workflow (e.g. `review`, `merging`) | any | Project |
| Subtask | In shared subtask-workflow state (e.g. `in_progress`) | User (`user:*`) | Project |
| Subtask | In shared subtask-workflow state | Agent or system | Inherited subtask workflow |

This aligns validation with the frontend, which presents target states from the
project workflow for all tasks. Automatic subtask lifecycle in subtask-workflow
states is unchanged. All undefined-state rejections — in the engine, hook
actions, cascades, recovery helpers, and prompt preview — flow through
`WorkflowEngine::undefined_state_message`, which enumerates the workflow's
defined states.

`StateKind` classifies states:

- **`backlog`** — parking lot; agent claims rejected.
- **`initial`** — exactly one per workflow; validation rejects zero or multiple.
- **`active`** — work state; may declare a role such as `coder`.
- **`gate`** — validation/processing state; `gate_config.max_rejections`
  enables retry-budget checks.
- **`terminal`** — absorbing state; outbound transitions and non-terminal
  cancellation targets are rejected.
- **`custom`** — no built-in behavior beyond graph validation.

`WorkflowEngine::transition` lifecycle for `A → B`:

1. Load task, check optimistic version, validate that `A` and `B` are defined
   states in the applicable workflow (undefined current or target rejects with
   an error enumerating defined state names), then validate the graph edge or
   implicit cancellation path.
2. Run filtered `A.before_exit` guards unless `B` is the cancellation target;
   `FailurePolicy::Block` failures return `GuardRejection` (HTTP 412).
3. Update `task.status`, increment `version`, write `transition_log`, publish
   `task.status_changed`.
4. Run filtered `A.on_exit`, filtered `B.on_enter`, then effective
   `B.after_enter` hooks. Gate states with `max_rejections` get
   `check_retry_budget` prepended unless already present.
5. Backfill `transition_log.hook_results_json`.
6. If an `after_enter` hook returns `HookResult::Cascade`, recursively
   transition with `triggered_by = "system"`; cascade depth is limited to 3.

Board moves use the same engine through `TaskService::move_task` and its board
persistence seam. A project owns a monotonic `board_revision`, advanced by
database triggers for board-affecting task inserts, deletes, status/position
updates, archives, and soft deletes. The public move command compares both the
task version and board revision after acquiring the SQLite write lock, validates
the destination workflow column and adjacent neighbor IDs, and writes status
plus board position once in a single transaction. Tight numeric gaps are
renormalized inside that transaction, so revisions are monotonic but not
gapless.

Same-column moves use the repository transaction directly and skip status
hooks. Cross-column moves run `before_exit`/`before_enter` guards before the
write, then reuse engine audit, `on_exit`, `on_enter`, `after_enter`, dispatch,
and cascade behavior from the committed task. The direct persistence step
increments the task version exactly once; a later cascade is a separate normal
transition and can increment it again. Rejected guards write no task, move
operation, or transition log.

`task_move_operation` stores normalized request identity, processing/direct
commit state, and the completed logical result. A same-ID/same-request retry
replays the result; different reuse conflicts. An incomplete record makes the
existing post-commit crash gap detectable, while board/task refetch remains the
recovery source of truth. Each newly committed direct move publishes exactly
one `task.moved` event after commit. Status-changing move events feed lifecycle,
project-hook, notification, and operation-status consumers in place of a second
direct `task.status_changed`; any synchronous cascade emits its own normal
transition event.

**User routing override:** When a user actor's move would be rejected solely
because (a) no trigger edge connects the states or (b) the matching trigger is
system-only (`Fail`/`Retry`), and `B` is a defined state in the applicable
workflow, the engine completes the transition via a user-routing-override arm
inside `transition_inner`: `before_exit`/`before_enter`
content guards still run and may block; `on_enter`/`after_enter` hooks run
normally; `task.status_changed` is published unconditionally; agent dispatch fires
only when a role/agent is assigned. Override transitions are audited as
`triggered_by = "user:override:<source>"` (e.g. `user:override:api`). This is
separate from `manual_override_transition`, a system-triggered primitive with
`skip_before_exit=true` used by `TaskService::advance_to_next_state`.

Hook audience filtering is uniform across phases. `HookAudience::All` always
runs. `AgentOnly` runs when `triggered_by` starts with `"agent:"` or equals
`"system"`; `UserOnly` runs only when it starts with `"user:"`. Non-matching
hooks are skipped without a hook-result entry.

Human-triggered transitions are treated as project-management actions. The
dependency gate does not block `user:*` card moves, including board drag
transitions, so users can reorder and reclassify work like they would in Jira.
Users may route a task to any defined workflow state via the override path when
strict routing would reject (see resolution rule above). Any user-initiated
transition that changes the task's state cancels in-flight executions with
`StopReason::UserCancelled`; same-state moves leave running executions
untouched. Parking an agent-assigned task in an Initial- or Backlog-kind state
retains role assignments but does not launch an executor from the move itself
— the task re-enters agent flow only through the normal scheduling path when it
later reaches a dispatchable state. AI execution remains gated separately:
initial role dispatch and interactive launch both run dependency checks before
creating an execution.

Cancellation is implicit from any non-terminal state to
`workflow.cancellation_state` (or terminal `"cancelled"` if unset), even
without an explicit edge. Project `before_exit` guards are bypassed for this
path; `on_exit` and cancellation-state `on_enter` hooks still run.

### Roles and assignments

Roles are declared by workflow (`roles[]`) and states can require a role
(`state.role`). Per-task assignments live in `task_role_assignment` keyed by
`(task_id, role_name)` with either `agent_id` or `user_handle`. Claiming
auto-assigns the claimed state's role to the claiming agent when no assignment
exists; a conflicting pre-assignment returns HTTP 409.

`assignee` is an engine-reserved role name. Active states without explicit
`state.role` implicitly bind `assignee`. This fallback applies only to Active
states; Gate, Initial, Backlog, Terminal, and Custom states without roles bind
no role. `state.role = Some("assignee")` on a non-Active state is rejected
during validation. `DefaultWorkflow` is unchanged and uses declared `planner`,
`coder`, and `reviewer` roles.

### Retry budgets

Audit-log derived. Gate states may set `gate_config.max_rejections`;
`check_retry_budget` counts `transition_log` rows with `from_state = gate` and
`rejection = true`, then cascades to `blocked` when exhausted. Generic
user-triggered gate-to-active bounces are logged with `rejection = false` and
do not consume budget.

### Crash recovery

`CrashRecovery` runs at server startup; `HeartbeatMonitor` applies the same
recovery primitive on agent timeout. Both annotate a task with a
`recovery_required` `error_annotation` only when they actually cancelled at
least one running execution for that task, and publish `task.recovered` only in
that case. Tasks whose assignee is a user are excluded from crash-recovery
selection — agent-oriented recovery is not meaningful for human-driven tasks.

After the orphan pass, startup runs a sweep that clears stale
`recovery_required` annotations when `blocked_execution_id` is missing, refers
to a nonexistent execution, or refers to an execution that is not in a stopped
state awaiting user recovery. The sweep is idempotent and only ever clears
annotations.

### Failure classification

Interruption kinds are a closed vocabulary: `FailureKind` in `api-types`
(serialized snake_case, TS-exported). It is the only classification signal —
`InterruptionMetadata.kind`, `TaskBlockingAnnotation.type`, and the
`task.blocked`/`task.failed` event payloads all carry it, producers
(`block_task`, `fail_task`, annotation writers) take the enum rather than
strings, and recovery/exception derivation branches exclusively on its
predicates (`is_retry_exhausted_metadata`, `is_budget_exhausted_annotation`,
`is_merge_recoverable`, …). Reason/message prose carries no classification
weight anywhere. Legacy database rows were normalized once by migration
`V056__normalize_failure_kinds`; kinds that migration could not map
deserialize to a read-only `Unknown` variant that renders info-only with no
recovery actions. Producers must never construct `Unknown`. The web client
likewise derives no failure semantics from workflow state names — gate
reject/bounce targets come only from explicit `reject`/`fail` trigger edges or
`gate_config.reject_target`.

Hard failures and recovery states surface to the user as notifications:
`task.failed` when `fail_task` sets `failed_json` (which also clears any stale
blocking annotation), and `task.recovery_required` when crash recovery or an
agent heartbeat timeout annotates a task for manual recovery.
Graceful-shutdown recoveries auto-resume at the next startup and are not
notified. In the derived `workflow_exception` summary, a hard failure
supersedes any blocking annotation — `recover_task` only accepts
`reset_to_initial`/`cancel_task` once `failed_json` is set, so only those
actions are offered. The web UI renders one actionable recovery surface,
`WorkflowExceptionPanel`, on both the task page and the board modal;
`TaskBlockingBanner` is an informational fallback for interruption states
without recovery actions.

`transition_log` is the audit source of truth for state changes. The API
exposes it via `GET /api/v1/tasks/{id}/transitions`.

### Files of interest

- `crates/services/src/workflow/engine/mod.rs` — lifecycle
- `crates/services/src/workflow/actions/` — curated hook actions
- `crates/services/src/workflow/default_workflow.rs` — built-in graph
- `crates/services/src/workflow/validation.rs` — workflow graph validation
- `crates/services/src/workflow/cache.rs` — per-project resolved definitions
- `crates/services/src/workflow/registry.rs` — action name resolution
- `crates/db/migrations/V009__workflow_engine.sql` — `project.workflow_definition`,
  `task_role_assignment`, `transition_log`

## Happy path

The canonical end-to-end flow is captured by `crates/api/tests/happy_path.rs`.
It boots the in-process Axum router with an embedded daemon and a real temp
git repo, drives a task through `todo → in_progress → review → merging → done`,
and asserts:

- The merge SHA lands on the default branch.
- The worktree is removed.
- One `review` row with `status=passed` is persisted.
- The expected event sequence appears on the bus.

Any refactor that breaks this test likely needs a spec realignment before
merging. Claiming a task auto-dispatches the executor via `tokio::spawn` in
`api::routes::tasks::claim_task` — there is no separate "dispatch" endpoint.

## Concurrency control

Tasks and agents use optimistic concurrency via a `version` column. Updates
require `WHERE version = ?` and increment on success. Version mismatch →
`DbError::VersionConflict` → HTTP 409.

## Database

SQLite with WAL mode. Schema in
`crates/db/migrations/V001__initial_schema.sql`. Migrations are numbered
`V{NNN}__{name}.sql` and tracked in `_migration` table. All primary keys are
app-generated UUID v4; all timestamps are app-generated RFC3339.

Connection pool sets `PRAGMA foreign_keys=ON`, `journal_mode=WAL`,
`busy_timeout=5000` per connection.

Tables: `project`, `repo`, `agent`, `skill`, `task`, `execution`, `review`,
`task_role_assignment`, `transition_log`, `task_terminal_session`,
`_migration`.

For tests, use `create_sqlite_pool("sqlite::memory:")` for an in-memory
database.

## Frontend

React + TypeScript + Vite + TanStack Query/Router. Source in `web/src/`. Uses
`@` path alias → `web/src/`. API client at `web/src/api/client.ts` calls
`/api/v1/*` endpoints. Types in `web/src/types/generated/api.ts` must match
`api-types` crate responses.

## Crate notes

- **db** — Enum serialization uses `Display`/`FromStr` (in `models.rs`) for
  SQLite TEXT columns. Row mapping is manual via `sqlx::Row::get()`, not
  compile-time checked macros.
- **services** — `TaskService.transition()` handles side effects (event
  emission, counter increments, `ReviewRunner` on `→ review`, `MergeService`
  on `review → merging`, `WorkspaceCleanupScheduler` on `→ done` /
  `→ cancelled`). Background tasks: `CrashRecovery` at startup (orphan
  execution recovery and stale-annotation sweep), `HeartbeatMonitor`,
  `DaemonMonitor`, `WorkspaceCleanupScheduler`.
- **review** — `ReviewRunner` runs `task.review_config.ci_steps` as `bash -lc`
  commands in the worktree; empty steps auto-pass. Creates a `reviewer`-role
  execution sharing the executor's workspace. Depends only on `db`, `events`,
  `executors` — not on `api` or `services`.
- **api** — Routes in `routes/{projects,tasks,terminals,agents,repos,executions,events,daemons,clis,profiles,executor_types}.rs`.
  Error module is `errors.rs` (plural). Middleware adds request IDs and CORS.
  `claim_task` auto-dispatches the executor.
- **executors** — `LogWriter` appends JSONL with schema version + sequence
  numbers. `ShellExecutor` spawns child processes with heartbeat supervision.
- **mcp-server** — JSON-RPC dispatch over `POST /mcp` with its own `McpState`.
  Does not depend on the `api` crate.
- **workspace** — File-based locking via `.forge.lock`. Path validation
  prevents traversal escapes.
- **config** — `ForgeConfig` with precedence: CLI flags > env vars > config
  file > defaults. Default bind uses loopback with an OS-selected port, then
  persists the selected port under the Forge data directory.
