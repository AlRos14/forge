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

### Event bus

The `events` crate wraps `tokio::sync::broadcast`. Services publish
`ForgeEvent` on state changes; the SSE endpoint at `/api/v1/events` subscribes
and streams them to web clients and other listeners.

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
`crates/services/src/workflow/engine.rs` is the new data-driven path;
`TaskService.transition()` still uses the legacy `TaskStatus`/`transition_allowed`
path. Treat the engine as a parallel code path until the split is removed.

Workflows are project-defined JSON in `project.workflow_definition`. Empty
string or `"{}"` resolves at runtime to the built-in `DefaultWorkflow`.
`WorkflowCache` caches resolved definitions per project and invalidates on
workflow updates.

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

1. Load task, check optimistic version, validate the graph edge or implicit
   cancellation path.
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

Hook audience filtering is uniform across phases. `HookAudience::All` always
runs. `AgentOnly` runs when `triggered_by` starts with `"agent:"` or equals
`"system"`; `UserOnly` runs only when it starts with `"user:"`. Non-matching
hooks are skipped without a hook-result entry.

Human-triggered transitions are treated as project-management actions. The
dependency gate does not block `user:*` card moves, including board drag
transitions, so users can reorder and reclassify work like they would in Jira.
Subtasks follow the same rule for status moves while still using the inherited
subtask workflow, so invalid child states remain rejected by the graph. AI
execution remains gated separately: initial role dispatch and interactive launch
both run dependency checks before creating an execution.

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

`transition_log` is the audit source of truth for state changes. The API
exposes it via `GET /api/v1/tasks/{id}/transitions`.

### Files of interest

- `crates/services/src/workflow/engine.rs` — lifecycle
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
  `→ cancelled`). Background tasks: `CrashRecovery` at startup,
  `HeartbeatMonitor`, `DaemonMonitor`, `WorkspaceCleanupScheduler`.
- **review** — `ReviewRunner` runs `task.review_config.ci_steps` as `bash -lc`
  commands in the worktree; empty steps auto-pass. Creates a `reviewer`-role
  execution sharing the executor's workspace. Depends only on `db`, `events`,
  `executors` — not on `api` or `services`.
- **api** — Routes in `routes/{projects,tasks,terminals,agents,repos,executions,events,daemons,clis,profiles,runtimes,executor_types}.rs`.
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
