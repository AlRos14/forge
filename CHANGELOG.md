# Changelog

All notable changes to Forge are documented in this file.

Forge follows Semantic Versioning. During the `0.x` public beta period, APIs and workflows may change between minor versions.

## Unreleased

### Added

- `DaemonReportRequest.active_execution_ids` — optional list of execution ids the reporting daemon is currently running. When present, the server reconciles stale server-side running executions owned by that daemon.

### Fixed

- Executions on a dead or disconnected remote daemon are now failed promptly with `stop_reason = daemon_disconnected` (120s disconnect grace via heartbeat monitor, plus reconcile-on-report for daemons that restart without those executions) instead of waiting for the 300s activity stall timeout and being mislabeled `execution_stalled`.
- The shell executor now honors `command`, `args`, and `env` from the agent config snapshot (previously silently ignored; empty configs keep the `sh -c <description>` default). Cancelling an execution whose process already finished is a no-op instead of an error.

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
