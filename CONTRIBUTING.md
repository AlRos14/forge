# Contributing to Forge

Thanks for contributing. The product and architecture overview are in the root [README](README.md) and [architecture guide](docs/architecture.md).

## Prerequisites

- Stable Rust
- Git
- Node.js 24
- pnpm 10 through Corepack

## Local development

```bash
git clone https://github.com/AlRos14/forge.git
cd forge
corepack enable
corepack prepare pnpm@10 --activate
make dev
```

`make dev` uses the ignored `./test` data directory and binds the server to `127.0.0.1:8080` so the Vite proxy can reach it. Run the frontend in another terminal:

```bash
make frontend
```

Vite serves the UI on `http://localhost:5173` and proxies `/api` and `/mcp` to the local server. A direct server start defaults to `127.0.0.1:0` and persists the selected port in `server.json`; set `FORGE_SERVER_BIND=127.0.0.1:8080` when the fixed Vite proxy port is needed. Use `make clean-test` only for the disposable `./test` directory; it does not target `~/.forge`.

Building the `forge-cli` binary runs the locked web install/build unless `FORGE_SKIP_WEB_BUILD=1` is set.

## Verification

The repository CI workflows and local scripts are the source for current checks:

```bash
./scripts/ci-rust.sh
./scripts/ci-web.sh
```

The Rust script runs formatting, clippy, workspace check, and workspace tests with the web build skipped. The web script installs from `web/pnpm-lock.yaml` and runs lint, typecheck, unit tests, and build. Its Playwright smoke is run by GitHub Actions after the web checks. Use the focused package or surface checks appropriate to a change; do not run a full workspace suite for documentation-only edits.

For an API response change, update the route handler, `api-types`, generated TypeScript types, and `docs/api.md` together. For domain changes, read [Architecture](docs/architecture.md) and [Architecture V2](docs/migration/architecture-v2.md), name the affected invariants, and preserve lifecycle and authority boundaries.

## Repository conventions

- No unsafe Rust; the workspace denies it.
- Keep Rust formatted and Clippy clean for affected code.
- Map errors at boundaries: `DbError` → `ServiceError` → `ApiError`.
- Use optimistic versions for concurrent updates. Version conflicts return HTTP 409.
- REST pagination is keyset-based and returns `items`, `has_more`, and an opaque cursor.
- Add numbered forward migrations; never rewrite historical migrations or silently discard installed data.
- TaskLifecycle is the only current Task state authority. WorkUnit allocation and RoleMembership are distinct; do not infer assignment from membership.
- A WorkUnit must be explicitly created and allocated before Agent Execution. `forge-ctl` currently has no WorkUnit creation command.
- Durable domain records are authoritative; SSE is a filtered projection.
- Preserve upstream MIT licensing and attribution; see [Upstream relationship](docs/upstream.md).

## Pull requests and security

Create a focused branch from the current `main`, describe the behavior and evidence, and report checks as PASS, FAIL, or NOT RUN. Do not claim CI passed when GitHub has no check run. Do not post vulnerability details publicly. The current private reporting setting and any owner-provided contact are recorded in [Security Policy](.github/SECURITY.md).
