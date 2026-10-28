# Post-V2 hardening acceptance ledger

This ledger records the single post-migration hardening pass requested after
Forge V2. It is not a new migration PR and does not change the PR0–PR15
architecture. Historical PR15 evidence remains in
[plan-pr15-acceptance-report.md](plan-pr15-acceptance-report.md).

## A. Base and branch

| Item | Value |
|---|---|
| Verified base | `origin/main` = `7fb78d9e845869122e61cce5cb510a3b0853afc9` (PR #20 merge) |
| Working branch | `fix/post-v2-security-hardening` |
| Schema | V001–V135; V135 is the only new forward migration |
| Merge/publication | Not performed |

## B. Defect matrix

| Finding | Root cause | Repair | Evidence |
|---|---|---|---|
| F-02 reusable URL credentials | REST/MCP accepted `?token=`; EventSource used the JWT in its URL; `forge-ctl` embedded a PAT in the MCP URL | Header-only reusable credentials; 30-second single-use SSE ticket; MCP install writes an Authorization header; daemon WebSocket query token removed; legacy MCP URL detection/redaction | `api/tests/auth.rs`, `mcp_oauth.rs`, `daemon_scoping.rs`, `daemon_connect.rs`, `forge-client` MCP tests, `api` MCP config tests |
| F-03 frozen JWT privileges | `verify_token` returned `is_admin` from claims; OAuth MCP verification did not load the user | Every JWT/PAT/OAuth/refresh/SSE ticket path resolves current durable authority; MCP tokens are marked and rejected as REST tokens | `services::auth_service` tests, `api/tests/auth.rs`, `mcp_oauth.rs` |
| F-04 last-owner race | owner count/caller checks ran before the mutation transaction | `BEGIN IMMEDIATE` for add/update/remove; caller/target/owner set re-read after the writer lock; canonical transfer or `409 last_owner` | `project_member_service` concurrency/reopen/rollback tests |
| F-05 owner/membership inconsistency | `owner_id` conferred authority without a membership row; creation was best-effort; bootstrap claimed ownerless Projects | V135 repair; atomic creation; membership-only authorization; canonical transfer on removal/demotion; no ownerless claim | `project_ownership.rs`, V135 migration test, `project_access.rs`, admin-delete test |

## C. New findings closed in this pass

| Priority | Finding | Repair |
|---|---|---|
| P0 | MCP config response returned legacy `?token=` PATs | Credential-like query parameters and userinfo are redacted; legacy URL reports `installed=false` |
| P1 | Any authenticated account could read/write server-global `user`/`local` MCP config | Server-admin requirement for non-project scopes |
| P1 | Admin account deletion bypassed canonical-owner transfer | Transactional owner transfer or `409 last_project_owner` |
| P1 | V135 could abort on a dangling `owner_id` | Repair only existing users; transfer to another existing owner; otherwise leave fail-closed |
| P1 | Agent/orchestrator/Execution authority still used owner equality alone | Exact current Project membership is required in eligibility, wake admission, and provider-start claim |
| P1 | Terminal WebSocket kept shell access after membership revocation | Recheck Project write authority on attach and every five seconds |
| P2 | Last-admin checks raced | Checked demotion/deletion inside `BEGIN IMMEDIATE` |
| P2 | Project update/pause/resume/delete authority was check-then-mutate | REST mutations recheck owner/admin membership inside the write transaction. MCP keeps its preflight; the remaining narrow MCP update TOCTOU is a residual P3 documented in the PR. |
| P2/P3 | Test fixtures used owner rows without membership or JWTs without users | Fixtures and negative tests now seed exact current authority |
| P3 | SSE ticket froze `is_admin` | Consume resolves the current user row |
| P3 | MCP usable check ignored origin and credential aliases | Full origin comparison and credential-key detection |
| P3 | MCP config followed symlinked files | Symlinked config paths are rejected |

Residual risks are recorded in the PR report; no P0/P1 remains known.

## D. Acceptance levels

| Level | Meaning | This pass |
|---|---|---|
| A | Real SQLite, Git, shell, loopback server, Docker | Auth/ownership/DB/migration/Docker checks |
| B | Fake provider/daemon with controlled failures | Reused PR15 S06/S09 plus changed admission tests |
| C | Native Codex/Claude/harness execution | **NOT RUN** in this environment; no private credentials were used |

Level B is not reported as Level C. Native harness execution remains an
external, credential-dependent action.

## E. Local commands

Results are recorded at the exact final HEAD in the pull request. Commands are
listed in [security-audit.md](../security-audit.md),
[release_checklist.md](../release_checklist.md), and the PR body.

## F. Recorded local evidence

These checks ran on 2026-10-28 on the working branch. The full API suite was
collected at `2de74710`; after a test-only password-helper change, the `auth`
targets were rerun and passed again (9 API and 19 services tests). The
documentation commit that follows does not change runtime code. The PR check
rollup at the exact final HEAD remains the merge gate.

| Check | Command | Result |
|---|---|---|
| Formatting | `cargo fmt --all -- --check` | PASS |
| Clippy | `FORGE_SKIP_WEB_BUILD=1 cargo clippy -p services -p api -p db -p mcp-server --all-targets --locked --offline -- -D warnings` | PASS, no warnings |
| API suite | `FORGE_SKIP_WEB_BUILD=1 cargo test -p api --tests --no-fail-fast --locked --offline -- --test-threads=4` | PASS: 230 passed, 1 ignored, 0 failed across the lib plus 38 integration binaries; EXIT 0 |
| Auth target after test-password change | `FORGE_SKIP_WEB_BUILD=1 cargo test -p api --test auth --locked --offline` | PASS: 9 passed |
| Auth service tests after test-password change | `FORGE_SKIP_WEB_BUILD=1 cargo test -p services --lib auth_service::tests --locked --offline` | PASS: 19 passed, 428 filtered |
| Services library | `FORGE_SKIP_WEB_BUILD=1 cargo test -p services --lib --locked --offline` | Partial, not a completed run: 205 passed and 0 failed before the run was stopped to avoid duplicating the CI workspace run |
| Frontend | `pnpm lint`, `pnpm typecheck`, `pnpm test`, `pnpm build` in `web/` | PASS; unit tests 118 passed; web tree unchanged by later code commits |
| Rust dependencies | `cargo audit` | No vulnerabilities; two documented non-vulnerability warnings |
| npm dependencies | `pnpm audit --prod` in `web/` | Clean |
| Docker image | `docker build -t forge-postv2-rc:local .` | PASS |
| Container identity | `docker top forge-postv2-test -o pid,user,args` | `forge --data-dir /data` runs as UID/GID `10001`; PID 1 is `docker-init` under `--init` |
| Container health and exposure | `curl -fsS http://127.0.0.1:18081/healthz`; `docker port forge-postv2-test` | `ok`; published only on `127.0.0.1` |
| Restart persistence | Register, create a Project, restart the container, list Projects with the same token | Project count remained `1` |
| Root-owned volume | Start without opt-in, then with `FORGE_MIGRATE_ROOT_OWNED_DATA=1` | Without opt-in: exit 1, no files changed. With opt-in: process UID `10001`, health `ok` |

Native harness execution (Level C) was not run. No private harness credentials
were used. The full-workspace Rust check, including the complete `services`
suite, runs in CI with `./scripts/ci-rust.sh`; an empty or pending check is not
evidence of success.

## G. Human review closure

| Finding | Root cause | Repair | Regression evidence |
|---|---|---|---|
| P1-1 terminal input after revocation | Authorization ran on attach and every five seconds, but `Input`/`Resize` frames did not revalidate before dispatch | `TerminalService` checks current Project write authority before every input and resize; the route closes the socket on `AuthorizationDenied` | `terminal_service::tests::input_and_resize_recheck_current_authority`: allowed before revocation reaches dispatch, denied after; API terminal suite and happy-path terminal execution pass |
| P1-2 literal MCP bearer tokens | `forge-ctl mcp install` wrote the token into project/local files with default permissions | Project/local configs use Claude `${VAR}`, Cursor `${env:VAR}`, or Codex `bearer_token_env_var`; install fails closed without the referenced variable. `user` scope writes literal tokens with `0600` | `forge-client` MCP tests: env-reference output for all three clients, no token in file, fail-closed without env, `--token` rejection, private user-file mode |
| P2-1 cwd MCP config fallback | Project scope with no `project_id` resolved `env::current_dir()` before URL validation | Project scope requires a non-empty `project_id` before path resolution; GET and POST share the check | `api/tests/mcp_config_http.rs`: non-admin GET/POST without `project_id` and a blank id return `400 project_id_required`; server-global scopes still return `403 admin_required` |
| P2-2 intermediate symlinks | Only the final path component was checked; `create_dir_all`/`write` followed directory symlinks | Canonical root plus descriptor-relative traversal, `O_NOFOLLOW` on every directory and final file, normal-component-only relative paths, private `0600` writes | MCP unit tests: parent traversal/absolute paths reject, symlinked config directory rejects without outside write, symlinked final file rejects without changing its target, written file mode is `0600` |

Focused commands:

```bash
FORGE_SKIP_WEB_BUILD=1 cargo test -p forge-client mcp::tests --offline
FORGE_SKIP_WEB_BUILD=1 cargo test -p services --lib terminal_service::tests::input_and_resize_recheck_current_authority --offline
FORGE_SKIP_WEB_BUILD=1 cargo test -p api --lib routes::mcp_config::tests --offline
FORGE_SKIP_WEB_BUILD=1 cargo test -p api --test mcp_config_http --offline
FORGE_SKIP_WEB_BUILD=1 cargo test -p api --test terminal_sessions --offline
FORGE_SKIP_WEB_BUILD=1 cargo test -p api --test happy_path forge_happy_path_end_to_end --offline
FORGE_SKIP_WEB_BUILD=1 cargo clippy -p api -p services -p forge-client --all-targets --offline -- -D warnings
```

Adversarial review of the changed surfaces found no remaining P0/P1 in the
terminal or MCP-config paths. Residual notes: the terminal idle revalidation
timer still closes silent connections within five seconds, but input/resize no
longer wait for it; MCP config writes are Unix-hardened with descriptor-relative
traversal (non-Unix builds use canonical-root checks); `--token-env` requires
the operator to export the variable in the client's environment.
