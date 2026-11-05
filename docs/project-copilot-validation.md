# Project Copilot validation

The implementation contract is [Project Copilot](concepts/project-copilot.md).
This change adds Project management runs, not Task Executions. TaskLifecycle
and worker admission remain separate authorities.

## Invariants exercised

- INV-003, INV-013 and INV-032: the selected Agent keeps its harness identity;
  unsupported management invocation configurations fail explicitly.
- INV-008, INV-016 and INV-017: management runs create no placeholder Task,
  WorkUnit, worker Execution or repository WorkspaceLease.
- INV-030, INV-033, INV-034 and INV-038: current durable authority fences MCP
  operations, mutations identify the Agent and requesting Human, and events
  record committed results rather than granting authority.
- INV-035 and INV-040: migration V139 adds new records and retains canonical
  Project ownership and existing user data.

## Repeatable checks

Run from the repository root:

```bash
FORGE_SKIP_WEB_BUILD=1 cargo test -p db --test project_copilot_authority
FORGE_SKIP_WEB_BUILD=1 cargo test -p services project_copilot
FORGE_SKIP_WEB_BUILD=1 cargo test -p services copilot_mcp
FORGE_SKIP_WEB_BUILD=1 cargo test -p mcp-server
FORGE_SKIP_WEB_BUILD=1 cargo test -p api --test copilot_mcp_authority
FORGE_SKIP_WEB_BUILD=1 cargo test -p api --test happy_path
FORGE_SKIP_WEB_BUILD=1 cargo test -p executors
FORGE_SKIP_WEB_BUILD=1 cargo test -p cli-adapters
FORGE_SKIP_WEB_BUILD=1 cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Run `pnpm typecheck`, `pnpm lint`, `pnpm test` and `pnpm build` from `web/`.
The isolated browser contract test is
`CI=1 pnpm exec playwright test e2e/project-copilot.spec.ts`; it uses mocked
API responses and the bundled Chromium, rather than the local Chrome channel.

The DB and API regressions cover delegation revocation, Project and attached
Task boundaries, transactional mutation rollback, Agent attribution, retry
deduplication, and transcript/activity pagination. Service tests use a stub
harness to verify durable turns, safe failures, secret redaction, cancellation
and restart recovery. Executor tests exercise cancellation before dispatch.
Frontend tests cover navigation context and the persistent panel.
The browser test checks exact Task submission on desktop and modal behavior
on mobile. These mocks do not establish a live backend-to-harness round trip.

These checks do not establish acceptance against a live model provider.
Provider-backed management must also demonstrate a real MCP WorkUnit operation
and confirm that the harness cannot invoke tools outside its management
delegation. Use only disposable `./test` data for local server acceptance.

## Verification receipt — 2026-11-05

| Check | Result |
| --- | --- |
| Web typecheck, lint and production build | PASS |
| Forge binary build (`FORGE_SKIP_WEB_BUILD=1 cargo build -p forge-cli`) | PASS — web built separately above |
| Web unit tests | PASS — 26 files, 131 tests |
| Copilot desktop/mobile browser contracts with mocked APIs | PASS — 2 tests |
| Copilot DB authority and transcript regression tests | PASS — 3 tests |
| Copilot service and native MCP configuration tests | PASS — 8 tests |
| API happy-path contracts | PASS — 2 tests |
| API Copilot delegation, attribution, scope and revocation | PASS — 1 test |
| MCP context/tool contracts and WorkUnit management | PASS — 14 selection tests plus the repaired WorkUnit contract rerun (1 test) |
| CLI adapter and executor unit tests | PASS — 113 adapter tests and 68 executor tests |
| Workspace Clippy, all targets, warnings denied | PASS |
| Rust formatting and whitespace checks | PASS |
| Broad workspace test run | NOT RUN to completion — all targets compiled and 57 API unit tests passed; stopped before completing the unrelated integration suites to prioritize the affected contracts |
| Live model-provider round trip | NOT RUN |

## Harness and UI expansion — 2026-11-05

The Project Copilot panel now takes its available executor types and exact
authorized embedded daemon ID from `GET /projects/{id}/copilot`. Assignment
accepts Agents owned by the canonical Project owner that have no host pin or
name that host; an unpinned Agent's run freezes the resolved embedded host in
its immutable snapshot. Agent creation now defaults to automatic host
resolution, while administrators can still pin an existing Agent through
editing, REST, or the CLI. Remote daemons remain unsupported for Project
Copilot.

Claude Code, Cursor Agent, OpenCode and Gemini CLI adapters advertise scoped
management support only when the embedded daemon reports the harness as
authenticated. Cursor and OpenCode were checked against locally installed CLI
versions 2026.09.26 and 2.2.5; each run also checks required CLI flags. Gemini
was not installed on this host, so its adapter checks the required CLI flags
before starting any model turn. Smith remains unsupported because the CLI
syntax documented by its upstream does not match the Forge adapter's current
invocation. Codex CLI 0.166.0 remains unsupported because its app-server
protocol does not expose a proven native-tool allowlist, ignores the attempted
`include_apply_patch_tool` session setting, and merges run MCP configuration
with existing MCP servers.

For enabled adapters, MCP settings are assembled per run (Cursor, OpenCode and
Gemini use temporary harness configuration; Claude Code uses its isolated
run-context MCP file). The short-lived Bearer value is passed only through the
process environment. The MCP endpoint and dispatch layer still enforce the
exact Project/Task scope and tool set.

| Check | Result |
| --- | --- |
| Rust workspace `cargo check` | PASS |
| Workspace Clippy, all targets, warnings denied | PASS |
| Web typecheck and lint | PASS |
| Web production build | PASS |
| Rust format and `git diff --check` | PASS |
| Test suites | NOT RUN in this change |
| Gemini CLI preflight on this host | NOT RUN — Gemini is not installed |
| Live model-provider round trip | NOT RUN |

## Automatic host resolution — 2026-11-05

Project Copilot accepts an Agent owned by the Project owner without a host pin.
The turn snapshots the resolved embedded host, and a pinned Agent that names a
different host still fails closed. Agent creation no longer asks for a host.

| Check | Result |
| --- | --- |
| DB Copilot authority, including an unpinned Agent with a resolved host | PASS — 4 tests |
| Copilot service, including the unpinned success path | PASS — 7 tests |
| API Copilot delegation/authority | PASS — 1 test |
| Web unit suite | PASS — 26 files, 139 tests |
| Web typecheck and lint | PASS |
| Rust formatting and Clippy for `executors`, `cli-adapters`, `services`, `db`, `mcp-server`, and `api` | PASS |
| OpenCode private-server invocation | PASS — adapter command test |
| MCP `tools/call` with `_meta` metadata | PASS — 1 MCP test |
| Copilot DB authority and V140 unpinned event insert | PASS — 4 DB tests |
| MCP Copilot delegated-tool projection | PASS — 1 MCP test |

Live acceptance on 2026-11-05: the Project Copilot created and allocated seven
WorkUnits for Task `3ed1af02-c979-4eb0-9e6f-65976055a5f6`, with the dependency
graph verified through the scoped tools.

OpenCode v2 keeps a background server whose configuration is not reloaded per
invocation. Project Copilot now starts a private OpenCode server with a
preflight session so the per-run agent and MCP catalog are loaded before the
real turn. The policy is written as a real config file so MCP `{env:...}`
authorization resolves, and `tools/call` accepts the MCP `_meta` field. V140
recreates the Copilot domain-event guard against the frozen resolved host so
unpinned Agents can complete management mutations. Terminal failures remain
visible next to the composer. A **Harness trace** section reads the run's
redacted JSONL events through the scoped run-log endpoint while the turn runs
and after it ends.
