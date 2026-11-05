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
