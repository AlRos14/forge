# CLAUDE.md / AGENTS.md

This file provides repository guidance to coding agents.

The user-facing entry point is [README.md](README.md). Current implementation contracts:

- [docs/architecture.md](docs/architecture.md) — post-PR13 runtime, authority, and persistence.
- [docs/migration/architecture-v2.md](docs/migration/architecture-v2.md) — ordered PR0–PR15 migration contract and invariants INV-001–INV-040.
- [docs/concepts/](docs/concepts/) — Actor, role, execution, WorkUnit, review, validation, Gate, and collaboration contracts.
- [docs/api.md](docs/api.md) — REST, MCP, and SSE.
- [docs/getting-started.md](docs/getting-started.md) — source build and end-to-end REST walkthrough.
- [docs/cli.md](docs/cli.md) — forge-ctl reference.
- [docs/deployment.md](docs/deployment.md) — container identity, persistence, and network exposure.
- [docs/upstream.md](docs/upstream.md) — origin, independent maintenance, and MIT attribution.

When behavior or a public contract changes, update the affected documentation in the same change. Keep README short and link to details.

## Product state

The workspace source version is 0.8.0. This repository has no verified published GitHub release as of the PR14 documentation audit. Build from this repository to use the current implementation; do not present ForgeAILab npm, Homebrew, installer, or release artifacts as this fork's distribution.

PR0–PR15 are merged. The current implementation uses the Actor and harness orchestration model, TaskLifecycle, TaskRole/RoleMembership, WorkUnits, explicit Executions and HarnessSessions, deterministic Gates, and generic collaboration. Main Agent, Project Agent, Agent Chat, Product Genesis, Project OS operations, legacy Attention, and the old WorkflowEngine are retired from current runtime/public authority. Their migration provenance and historical snapshots remain historical evidence.

Authentication and authorization use current durable authority. Reusable JWT, PAT, OAuth, and daemon credentials are accepted only in `Authorization: Bearer`; `?token=` is rejected on REST, MCP, and daemon WebSockets. The browser SSE transport uses a 30-second single-use ticket from `POST /api/v1/events/ticket`. JWT verification and OAuth MCP verification load the current user row; token claims never supply current admin authority. `Project.owner_id` is canonical and must have an exact role-`owner` membership; V135 repairs historical representation without claiming ownerless Projects.

The product name remains Forge until its owner provides a new name. Do not rename public or persisted identifiers, crates, commands, packages, variables, paths, or database records without that exact decision and a reviewed preservation plan.

## Change rules

- Breaking public changes must be visible in CHANGELOG.md under Unreleased; do not introduce compatibility aliases or silent legacy behavior.
- Preserve installed user data. Add forward numbered migrations and never edit historical migration files. Any destructive test reset must be limited to disposable ./test data.
- Treat REST routes, MCP tool names/schemas, forge-ctl flags, SSE payloads, JSONL logs, configuration, and on-disk paths as deliberate public contracts.
- When a design changes, update the implementation and its callers to match the current contract; do not layer retired authority back into the runtime.
- Read Architecture and Architecture V2 before changing domain wiring, Execution, WorkUnit, workspace, role assignment, Gate, or TaskLifecycle.
- TaskLifecycle is the only current Task state authority. TaskRole and RoleMembership define responsibility and eligibility; WorkUnit allocation defines executable scope. Do not infer an Actor, Workspace, or HarnessSession from a Task or the latest Execution.
- A WorkUnit must be explicitly created and allocated before Agent Execution. forge-ctl currently has no WorkUnit creation command.
- Project visibility is not write authority. Preserve exact Actor, RoleMembership, WorkUnit, WorkspaceLease, and provider-start checks.
- Project owner mutations revalidate the caller, target, owner set, and Project inside one `BEGIN IMMEDIATE` transaction. Do not trust a preflight owner count.
- Durable records are authoritative; EventBus and SSE are delivery/projection mechanisms.
- Never put reusable access tokens in URLs, logs, errors, telemetry, or public events. Short-lived single-use transport tickets are the only query credential exception.
- Preserve MIT licensing and upstream attribution.

## Build and check commands

```bash
cargo build
cargo test
cargo test -p db
cargo test -p api --test happy_path
cargo run --locked -p forge-client -- --help
cargo build -p forge-client
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
FORGE_SKIP_WEB_BUILD=1 cargo check -p forge-cli
```

The Rust build script installs the locked web dependencies and builds the frontend unless FORGE_SKIP_WEB_BUILD=1 is set. Do not run cargo clean; reuse the existing target cache. Choose checks proportional to the change. Documentation-only changes do not need a workspace Rust build.

The API happy-path test is a focused end-to-end contract test. Lifecycle changes must name touched invariants and keep the relevant lifecycle/API tests green.

### Local server and web UI

Use Makefile targets for local testing. They use the disposable ./test/ data directory; make dev binds to 127.0.0.1:8080 for the Vite proxy. The direct server default binds to 127.0.0.1:0, selects an available loopback port, and saves it in server.json.

```bash
make dev
make dev-demo
make dev-no-daemon
make frontend
make clean-test
```

make clean-test removes only ./test/. It must not be used against ~/.forge or another real data directory.

Frontend checks:

```bash
cd web
pnpm install --frozen-lockfile
pnpm lint
pnpm typecheck
pnpm test
pnpm build
pnpm run e2e
```

pnpm is the package manager. Do not add npm or Yarn lockfiles.

## Architecture and implementation conventions

```text
forge-cli → api → services → db
                → events
          → mcp-server → same domain services
          → HarnessAdapter → cli-adapters → forge-daemon
          → workspace → git
          → config
          → api-types
forge-client → REST API
web/ → REST API and authenticated SSE
```

- Humans and harness-bound Agents are peer Actors. Executions record one Actor, Role, Purpose, and explicit context.
- Harnesses own cognition and native session semantics; Forge owns work, authority, collaboration, evidence, and lifecycle.
- Concurrent repository WorkUnits use separate workspaces and explicit leases. Integration is a separate serialized operation.
- SQLite and numbered migrations are current persistence.
- TaskLifecycleService owns aggregate Task state. task.status was removed in V129.
- Formal Review is a reviewer Execution with Purpose review and a ReviewReport Artifact. ValidationRun is deterministic evidence; Agent Purpose validate is cognitive work.
- Gate evaluates exact immutable inputs; it does not run tests, review code, integrate, or merge.
- The current Orchestrator is event-driven and has a bounded typed output set. Do not claim unsupported autonomous steering, assignment, or merge actions.

Public API response changes touch the route in crates/api/src/routes/, DTOs in crates/api-types/, generated TypeScript types in web/src/types/generated/, and docs/api.md.

Errors flow DbError → ServiceError → ApiError; map them at boundaries. Concurrent updates use expected versions and return a typed conflict (HTTP 409). REST pagination uses opaque keyset cursors and response field items.

## Database

SQLite uses WAL mode. Migrations are named V{NNN}__{name}.sql, recorded in _migration, and must be additive and data-preserving. Primary keys and timestamps are generated by the application. Keep migrations, row mappers, and tests aligned.

For isolated DB tests, use create_sqlite_pool("sqlite::memory:"). Do not change historical V001–V135 migrations; add a new numbered file when schema work is authorized.
