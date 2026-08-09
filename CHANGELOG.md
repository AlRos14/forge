# Changelog

All notable changes to Forge are documented in this file.

Forge follows Semantic Versioning. During the `0.x` public beta period, APIs and workflows may change between minor versions.

## [Unreleased]

### Fixed

- `forge-ctl login` now hides interactively entered passwords, restores terminal settings after success, failure, cancellation, or EOF, and directs non-interactive callers to `--password-stdin` instead of consuming piped input implicitly.

## [0.7.0] - 2026-08-08

### Breaking

- Saving a workflow (workflow template save or project workflow update) now requires an explicit `canonical_phase` on every state; definitions without phases are rejected with a field-level error naming the offending state. Existing stored workflows continue to load and run unchanged — only re-saves must add phases.

### Added

- `CanonicalPhase` (`backlog`/`ready`/`working`/`review`/`done`) as the product-wide grouping language: optional `canonical_phase` on workflow states with ordered fallback derivation for legacy definitions, an additive `canonical_phase` field on task responses (derived at read time, never persisted), and a `canonical_phase` filter on project task lists that composes with existing filters (`phase=done` includes cancelled tasks).
- `autonomous_v1` builtin workflow preset: one `worker` role owns plan → implement → self-test; no planning gate; Forge-run `ci_steps` validation gates review and a failure automatically resumes the same worker thread; review requires human approval with no auto-dispatched reviewer; merge states stay within the Review phase; worktrees are cleaned up on done/cancelled.
- Intent-oriented task action endpoints: `POST /api/v1/tasks/{id}/{start|pause|resume|submit|request-changes|approve|cancel}` resolve the project workflow to the correct transition without clients hardcoding state names; unavailable actions return a structured 409 with `available_actions` and `reason`. The raw `/transition` endpoint is unchanged.
- Typed transition actor (`Actor`: User/Agent/System) replaces string-prefix actor checks throughout the engine, services, API, and MCP server. Audit log strings are format-compatible.
- Product terminology adapter in the web UI: user-facing copy now says Run (execution), Runtime (daemon), and Phase; routes and API names unchanged.
- Legacy workflow/DB compatibility fixtures with schema-parity regression tests, seeding future migration tests.
- Smith agents forward `reasoning_effort` to the CLI as `--effort`. `SmithConfig` gains an `effort` field, populated from the agent record like `model` already is; when the agent sets no `reasoning_effort`, no flag is emitted and behavior is unchanged. Requires a Smith build that accepts `--effort` — older Smith releases select effort only through a named profile or `SMITH_REASONING_EFFORT`.

### Changed

- Attribution fixes from the typed-actor refactor: human review approvals/rejections and MCP-initiated transitions are no longer audit-logged as `system`; claim hooks attribute the actual assignee. `AgentOnly` workflow hooks now match all system components, so the dependency gate applies to dispatcher-initiated launches. Human review rejections now record `rejection: true` and count toward the review retry budget. Display values: `system:daemon` → `system:executor` (execution `stopped_by`), `system` → `system:workflow` (recovery `blocked_by`).
- The task list `status` filter accepts custom-workflow state names instead of only built-in statuses.

## [0.6.1] - 2026-08-08

### Changed

- Smith execution options are now discovered from the user's `~/.smith/config.toml` — configured models, main-enabled profiles with their provider/model pairings, and provider names — instead of a hardcoded model list. Hosts without a Smith config discover empty lists.
- Bumped managed CLI pins: `@anthropic-ai/claude-code` 2.1.220 → 2.1.226, `@openai/codex` 0.146.0 → 0.147.0. `@musistudio/claude-code-router` stays on 2.0.0: v3 replaced the `ccr code <args>` pass-through with profile-based invocation and needs its own adapter rework.

## [0.6.0] - 2026-08-07

### Added

- First-class support for `Smith` (`smith`) CLI agent executor across `executors`, `cli-adapters`, embedded daemons, MCP tool descriptors, database migrations, and the web UI.
- Executor fallback chains: an agent's `config_json` may declare ordered `fallbacks: [{executor_type, config}]` candidates (e.g. multiple Smith provider profiles, or a cross-CLI fallback). Both the embedded path and remote daemons dispatch through a `FallbackExecutor` that advances only on structured availability failures (quota exhaustion, missing CLI, failed auth), keeps in-memory per-account cooldowns, aggregates token usage across attempted candidates, and logs every hop to the execution's JSONL log. The Smith and Claude Code adapters classify quota/auth signatures from structured stream signals only.
- New `FailureKind::ExecutorUnavailable`: when every candidate is unavailable, the task defers dispatch to the structured retry time **without consuming the execution retry budget** (transient), or blocks for manual reconfiguration (permanent). The generated `FailureKind` TypeScript union gains `'executor_unavailable'`.
- Daemon protocol: `ExecutionTerminalNotification` gains optional `failure_class`, `retry_at`, `resolved_candidate`, and `route_attempts` fields (additive — older daemons degrade to generic executor-failed handling).
- Session resume is now candidate-identity-aware: follow-ups promote the parent execution's winning candidate when it is still routed, and a candidate switch (including a different Smith profile on the same executor) starts a fresh session instead of replaying another account's session id. Smith executions now inject `resume_session_id` on follow-up like Claude Code/Cursor.

### Fixed

- Cancelling a shell execution now always SIGKILLs the whole process group after the grace period. Previously, if the direct child died to SIGTERM while a TERM-ignoring descendant survived, the escalation was skipped and the execution stalled until the descendant exited on its own (deterministic on Linux).
- Playwright smoke tests in CI use the container's bundled Chromium instead of requiring a Google Chrome install; local runs still use the `chrome` channel.

## [0.5.0] - 2026-08-03

### Breaking

- Replaced `PUT /api/v1/tasks/{id}/position` and its `PositionRequest`/`PositionResponse` types with the atomic, versioned `POST /api/v1/tasks/{id}/move` command. Project task-list pages now include `board_revision`; board clients must submit both that revision and the moved task's version.
- Board status moves now publish the canonical `task.moved` SSE event instead of a `task.status_changed` event for the direct move. The payload includes operation identity, old/new status and position, resulting task and board versions, and requested neighbors.

### Added

- Persisted project board revisions and idempotent move-operation records, with transactional neighbor validation, position renormalization, actionable concurrency conflicts, and replay of completed operation IDs.

### Changed

- The production web client now lazy-loads route screens and editor-backed dialogs, and the server Brotli/gzip compresses eligible responses while retaining immutable cache headers for hashed assets.
- Updated the Forge-managed Codex CLI to 0.146.0 and Claude Code CLI to 2.1.220. Adapter discovery now advertises the current GPT-5.6 and Claude 5 model families plus model-specific reasoning choices, including Codex `max`/`ultra` and Claude Code `xhigh`/`max`/`ultracode`; Gemini discovery now includes its stable model aliases and current Gemini 3.x catalog, and the web selectors filter effort choices for the selected model.

## [0.4.0] - 2026-07-03

### Changed

- Task interruption kinds are now a closed, typed vocabulary (`FailureKind`): `Task.blocked.kind`, `Task.failed.kind`, the blocking annotation `type`, and the `task.blocked`/`task.failed` event payloads carry an enum value instead of a free string. Wire values are unchanged for all known kinds; the generated TypeScript types narrow from `string` to the union. Classification of recovery actions now depends only on the structured kind — rewording a reason/message no longer changes which actions are offered.
- Migration `V056__normalize_failure_kinds` renames legacy aliases in existing rows (`retry_budget_exhausted` → `retry_exhausted`, `crash` → `executor_failed`, `hook_failed` → `before_work_hook_failed`) and adds a structured kind to rows that were previously classified only by their reason phrasing. Unmappable kinds are preserved and render as info-only interruptions with no recovery actions.
- The web client no longer infers gate rejection semantics from workflow state names (`*_failed` suffixes). Reject buttons appear only when the workflow declares a `reject`/`fail` trigger edge or `gate_config.reject_target`; workflows relying on naming conventions must declare the edge.

### Added

- Notifications for hard task failures (`task.failed`) and for crash-recovery or agent-timeout states that need manual intervention (`task.recovery_required`). Graceful-shutdown recoveries auto-resume at startup and are not notified; user-initiated recovery actions are not echoed back as notifications.
- Failed lifecycle-hook details (command, exit code, stderr/stdout tails) now surface in the `workflow_exception.failing_step` summary, so the recovery panel shows them wherever it renders.

### Changed

- The task board modal now renders the same actionable recovery panel as the full task page, driven by `workflow_exception`. Failed tasks were previously a dead end in the modal (message with no actions); they now offer Restart Task / Cancel Task. `TaskBlockingBanner` is reduced to an informational fallback for interruption states without recovery actions.
- Failure severity colors are no longer inverted in the task UI: hard failures render red, recoverable blocked states amber.

### Fixed

- A hard-failed task with a leftover blocking annotation no longer offers retry/resume actions that the server rejects with 400: `failed_json` now supersedes the annotation in the derived `workflow_exception` (offering only Restart/Cancel), and `fail_task` clears the stale annotation at write time.
- The recovery panel no longer shows two "Cancel Task" buttons when the backend action list also contains `cancel_task`.

## [0.3.0] - 2026-07-02

### Added

- `DaemonReportRequest.active_execution_ids` — optional list of execution ids the reporting daemon is currently running. When present, the server reconciles stale server-side running executions owned by that daemon. Long-running daemon processes (`forge-daemon`, `forge-ctl daemon start`/`link`) claim their active set from startup onward; finished ids linger in reports for 120s so in-flight completions are never reconciled away.
- New execution stop reason `daemon_disconnected` and SSE event `execution.daemon_disconnected`, emitted when the server interrupts an execution whose remote daemon went away (120s disconnect grace via the heartbeat monitor, or immediately when a restarted daemon reports without the execution).

### Fixed

- Executions on a dead or disconnected remote daemon are now failed promptly with `stop_reason = daemon_disconnected` instead of waiting for the 300s activity stall timeout and being mislabeled `execution_stalled`. Failed executions follow the normal retry budget before blocking the task.
- The shell executor now honors `command`, `args`, and `env` from the agent config snapshot (previously silently ignored; empty configs keep the `sh -c <description>` default). Cancelling an execution whose process already finished is a no-op instead of an error.
- The heartbeat monitor no longer routes stall-cancellation of remote-daemon executions through the embedded executor.

## [0.2.0] - 2026-07-01

### Breaking

- Removed REST endpoints that had no consumers (web, CLI, or MCP): the legacy non-state-scoped gate decisions `POST /api/v1/tasks/{id}/gates/approve` and `/gates/reject` (use the state-scoped `/gates/{state_name}/approve|reject`), `GET /api/v1/tasks/{id}/conflicts`, `POST /api/v1/tasks/{id}/conflicts/abort`, `POST /api/v1/tasks/{id}/rebase`, `GET /api/v1/runtimes` and `/runtimes/{id}`, and the bare `GET /api/v1/workspaces/{id}` (`/workspaces/{id}/diff` remains).
- Removed the `override` field from `TransitionTaskRequest`; it was never read — user routing auto-escalation applies unconditionally, so observed behavior is unchanged.
- `forge-cli`'s build script now skips the frontend build only when `FORGE_SKIP_WEB_BUILD` is `1`/`true`/`yes` (previously any value, including `0`, skipped it).

### Added

- The JWT signing secret is now configurable via `server.jwt_secret` in the config file or `FORGE_JWT_SECRET`; when unset, Forge generates a random 32-byte secret on first start and persists it to `<data_dir>/jwt_secret.bin` (mode `0600`). Bcrypt cost is configurable via `server.bcrypt_cost` / `FORGE_BCRYPT_COST` (default 12).

### Changed

- User-initiated task transitions on subtasks now resolve against the project workflow, fixing rejections such as `state 'review' is not defined in workflow` when dragging a subtask to a state the board offered. Users may route a task to any defined workflow state, overriding missing-edge and system-only routing restrictions; content guards still apply. Override transitions are audited as `triggered_by = "user:override:<source>"`.

### Fixed

- Updated the Rust lockfile to pull patched `quinn-proto` and `anyhow` releases so `cargo audit` passes for the 0.2.0 release.

- User moves no longer fail with `state '<name>' is not defined in workflow` from downstream layers: the workflow is resolved once per transition and threaded through hooks and cascades; all undefined-state errors now enumerate the defined states. Any user move that changes state cancels in-flight executions, and parking a task to backlog keeps its agent assignment without relaunching.

- The false "Recovered after server restart" banner: crash recovery now annotates only tasks whose running execution it actually cancelled, skips user-assigned tasks, and clears stale recovery banners automatically at startup.

- Production servers previously signed session JWTs with a hardcoded development secret at bcrypt cost 4; they now use the configured or per-install generated secret at cost 12.

- Fixed memory search pagination so cursors follow the result ordering, escaped punctuated memory search input before passing it to SQLite FTS, and made review/execution/conversation memory indexing idempotent by source reference.

## [0.1.11] - 2026-06-08

### Added

- Memory layer: a new append-only `memory_item` store (FTS5-indexed) that automatically captures execution summaries, reviews, task comments, failure/hook-error transitions, and conversation messages as searchable, project-scoped, attributed memories.
- New REST endpoint: GET /api/v1/projects/{id}/memory/search — project-scoped layered memory search with pagination
- New REST endpoint: GET /api/v1/memory/{id} — memory item retrieval by id
- New MCP tool: forge_memory_search — project-scoped memory search with injection-guard wrapper
- New MCP tool: forge_memory_get — memory item retrieval by id
- New REST endpoint: POST /api/v1/memory/backfill (admin) — backfill memory index from existing data
- New CLI command: forge-ctl memory backfill
- Effective prompt preview: GET /api/v1/tasks/{id}/prompt-preview (read-only, no dispatch), MCP tool forge_preview_prompt, and CLI forge-ctl task prompt-preview

### Changed

- Prompt contracts v2: all default prompt builders updated with managed-execution contract, explicit role boundaries, structured handoffs (coder family), and structured reviewer findings. Builder ids bumped: coder_implementation_v1→v2, coder_review_fix_v1→v2, coder_merge_fix_v1→v2, reviewer_default_v1→v2, planner_default_v1→v2, generic_default_v1→v2.

### Fixed

- Task comments created through the REST API were not indexed into the memory layer because the handler bypassed the indexing service path; user comments now route through `TaskService::add_user_comment` and are indexed.
- Codex executor model list now advertises currently supported models (gpt-5.5, gpt-5.4, gpt-5.4-mini, gpt-5.3-codex-spark); removed stale entries (gpt-5.3-codex, gpt-5.2-codex, gpt-5.1-codex-max, gpt-5.4-fast) that the current Codex CLI rejects.

## [0.1.10] - 2026-06-06

### Fixed

- Daemon command-stream disconnects now mark the daemon offline immediately, server startup clears stale external daemon online state, and command-stream heartbeats refresh last-seen state while the daemon remains connected.
- Task and workspace diff endpoints now compare against the workspace branch's merge base instead of the moving default branch, so unrelated default-branch changes do not appear in task diffs.

## [0.1.9] - 2026-06-03

### Fixed

- Daemon link/start/report now create the configured workspace root before reporting it, so Add Local Repository can browse the launch directory instead of failing on `path=.` when the directory is missing.

## [0.1.8] - 2026-06-03

### Fixed

- Fixed existing databases that already recorded migration version 53 before the Cursor executor migration so daemon reports can create `cursor` agents.

## [0.1.7] - 2026-06-03

### Added

- Added `forge-ctl daemon start` to restart a previously linked daemon from saved credentials without repeating initial registration.

## [0.1.6] - 2026-06-02

### Fixed

- User-managed task and subtask status moves are no longer blocked by dependency or root-managed subtask guards; AI dispatch and execution launch still enforce dependency gates before starting work.
- Board status transitions now retry genuine task version conflicts once and show the real API error for other HTTP 409 responses.
- MCP initialize responses now report the crate package version instead of a hard-coded server version.

## [0.1.5] - 2026-05-30

### Added

- Added a first-class Cursor executor backed by `cursor-agent` headless stream JSON mode, including daemon detection, agent registration, web UI configuration, session resume, and execution log normalization.

### Changed

- Updated the Forge-managed Codex, Claude Code, and Claude Code Router package pins to their current npm `latest` versions.

### Fixed

- Linked `forge-ctl daemon link` sessions now keep the daemon command stream open so filesystem browsing and remote agent dispatch work from server-managed local daemons.
- Daemon reports with a full authenticated CLI set no longer fail while checking existing daemon-scoped agents.
- Remote daemon `execution.start` failures now fail and block the execution for recovery instead of leaving it stuck in `running`.
- `forge-ctl` now defaults to the stored login server before falling back to the last local server state.
- Project list responses from older servers without `project_hooks` fields deserialize correctly.
- Repo-less tasks no longer auto-dispatch agent work, and stopped executions now surface in workflow health.

## [0.1.4] - 2026-05-21

### Added

- Added the project-wide hook engine with committed task-event evaluation, all-work-completed trigger support, hook actions for dispatching agents, creating tasks, comments, and notifications, plus hook-run history access.
- Added project hook persistence and observability foundations: `project_hooks_json`, `task.is_automation`, `project_work_epoch`, the `project_hook_run` table, `project_hook.run_changed` events, and `ProjectHookRule`/trigger/action/run response API types.
- Added `project_hooks` to project API responses and `PATCH /api/v1/projects/{id}` so project-wide hook rules can be validated and persisted.
- Task terminal sessions (disabled by default; enable via `terminal.enabled`), including `POST/GET /api/v1/tasks/{id}/terminals`, `GET /api/v1/tasks/{id}/terminals/availability`, `GET /api/v1/terminals/{id}`, `POST /api/v1/terminals/{id}/attach-token`, `POST /api/v1/terminals/{id}/resize`, `POST /api/v1/terminals/{id}/terminate`, `GET /api/v1/terminals/{id}/ws`, and the `task.terminal.session_changed` SSE event.

### Fixed

- Terminal resize/start now rejects row or column counts below 2 with `invalid_input`, drops reconnect scrollback after all clients detach, validates terminal session limit config on load, and serializes web reattach attempts.
- Refreshed the Rust dependency lockfile and compatibility fixes so `cargo audit` and Rust CI pass on the current stable toolchain.

### Breaking

- Task media now requires access to the owning project, restricts media deletion to project owners/admins, and rejects SVG uploads instead of serving them as inline media.

## [0.1.3] - 2026-05-16

### Added

- Linux release artifacts now include musl builds for Alpine and other musl-based environments.

## [0.1.2] - 2026-05-16

### Changed

- npm bootstrapper no longer opens a browser by default; pass `--open` to opt in.
- Forge persists the selected local server port so `forge-ctl` can discover the server without a manual `--server` URL.

## [0.1.1] - 2026-05-16

### Added

- `forge-ctl login`, `logout`, and `whoami` commands for API-token based CLI auth.
- MCP install flows can create/login with API tokens before writing client config.
- npm bootstrapper package so users can start Forge with `npx @forgeailab/forge`.

## [0.1.0] - 2026-05-15

### Added

- Initial public beta of the local-first Forge workflow engine.
- Rust server, REST API, MCP endpoint, `forge` server binary, `forge-ctl` client binary, and web UI.
- Task lifecycle, isolated workspaces, agent registration, execution logs, review flow, and merge flow.
- CI coverage for Rust workspace tests, web unit tests, cargo audit, and a Playwright app-shell smoke test.
- Release archives for Linux and macOS containing `forge`, `forge-ctl`, and built web UI assets.
- GitHub release checksum generation through `SHA256SUMS`.
- Docker image publishing to GitHub Container Registry with provenance and SBOM metadata.
- Public repository metadata for generated release notes, code ownership, dependency updates, CodeQL, and OpenSSF Scorecard.
- Runtime support for installed web UI assets through `FORGE_WEB_DIST_DIR` and the standard `share/forge/web/dist` location.
