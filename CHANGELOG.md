# Changelog

All notable changes to Forge are documented in this file.

Forge follows Semantic Versioning. During the `0.x` public beta period, APIs and workflows may change between minor versions.

## [Unreleased]

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
