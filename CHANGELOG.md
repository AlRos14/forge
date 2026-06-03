# Changelog

All notable changes to Forge are documented in this file.

Forge follows Semantic Versioning. During the `0.x` public beta period, APIs and workflows may change between minor versions.

## [Unreleased]

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
