# PR14 — Final product documentation and rename preparation ledger

## Baseline and Git

- Repository: `AlRos14/forge`.
- PR14 branch: `feat/plan-pr14-final-product-documentation`.
- PR13 PR: [#16](https://github.com/AlRos14/forge/pull/16), merged normally on 2026-10-16 at 08:53:56 UTC.
- PR13 merge commit: `0afe361ad7be94cad71db037c46be73ddef3340e`.
- Merge parents: PR12/main `3d3b6b196672568c85e4c3cd2c11f010b1f62d9a`; final PR13 head `bcf14adff6e6f25d58120d1bd0087db93f6b7fc9`.
- The plan's originally cited PR13 checkpoint `447cbd387149019a44fdec006e58a54d529f0a41` was superseded by later reviewed PR13 commits. PR #16's actual head is `bcf14ad...`; its merge base with the PR12 main checkpoint remains `3d3b6b19...`.
- `bcf14ad...` is an ancestor of the merge commit and `origin/main`. V121–V134 are present.
- At PR14 start, local `main`, `origin/main`, and the PR14 branch base were all `0afe361...`; the worktree was clean before the PR14 ledger was created.
- Initial PR14 publication checkpoint: branch and remote SHA `50447e6bdc1ff11b54ade40a4779395ce03b49b6`; PR #17 was created from this head. The final delivery commit and head are reported in the PR and final delivery report.

PR13's repository Actions were enabled and CI, CodeQL, Release, and OpenSSF Scorecard workflows were active. GitHub returned zero PR13 workflow runs and zero check runs, including after a real push of the reviewed PR13 SHA to a temporary validation ref; that ref was deleted. The owner authorized the plan's local-evidence substitution for G1 on 2026-10-16. This is not a CI PASS.

The PR13 ledger records the focused evidence: Orchestrator claim/runtime `pr13_` tests (13 passed); Workspace-free Agent WorkUnit tests (6 passed); the REST non-repository WorkUnit Execution test (1 passed); the MCP `ExecutionStartUncertain` structured no-retry test; REST start and cancellation coverage; recovery, exact follow-up/idempotency, retry, concurrency and provider-start tests; `cargo fmt --all -- --check`; and `git diff --check`. The PR13 ledger retains exact commands and limitations, including one earlier API test that could not resolve its offline dependency. No workspace-wide test run is represented as passing.

## Objective and exclusions

Describe the implementation after PR13 through public documentation, examples, product presentation, developer entry points, and distribution guidance. Preserve upstream attribution, MIT licensing, historical evidence, and existing identifiers.

PR14 adds no new domain capability or migration. It does not publish a package, rename the product, reset or migrate user data, rewrite historical PR/ADR/migration records, or begin PR15 reference scenarios or final acceptance. Following review, it includes one narrowly scoped correction to a confirmed SQLite transaction-upgrade failure at WorkUnit creation finalization; broader reliability work remains outside PR14. The owner has not provided a replacement name. Public and persisted identities remain unchanged.

A narrow packaging correction was included and is tracked separately below: the Docker image now binds its already exposed port, and `make dev` uses the Vite proxy's fixed local port. These changes align existing entry points; they add no API contract or persistence schema. The local Docker image build and ephemeral port/health smoke pass. The original live Getting Started REST walkthrough exposed a WorkUnit create response failure after the row committed. The POST was not retried; barrier-controlled two-connection tests then reproduced the database cause and verified the minimal `BEGIN IMMEDIATE` repair. A fresh end-to-end HTTP walkthrough after the repair was not run, so the Getting Started acceptance remains partial.

## Classification and identity inventory

Classification is applied by context; the same text can be historical in one file and current in another.

| Reference family and locations | Classification | Treatment and compatibility |
|---|---|---|
| Current Actor, harness, TaskRole, RoleMembership, TaskLifecycle, WorkUnit, Workspace, Execution, HarnessSession, Artifact, collaboration, ReviewReport, ValidationRun, Evidence, Gate, Orchestrator and local REST/MCP/CLI/web surfaces | **Actual** | README, Getting Started, Architecture, API, CLI, logs and web guidance now describe the post-PR13 contracts and constraints. No autonomous cognition or implicit WorkUnit is claimed. |
| Main/Project Agent, Agent Chat, Product Genesis, Project OS, Attention, legacy WorkflowEngine and `task.status` authority | **Retirada** | Not presented as current operations. Current docs identify their removal where needed to distinguish retained history from runtime behavior. |
| V001–V134, PR/ADR/migration ledgers, old changelog entries, `agent_session_history`, immutable Execution records, ProjectRelease snapshots, migration receipts, Artifact/Evidence references, HarnessSession IDs and cross-record IDs | **Histórica** | Migration, ADR, upstream, license, and prior PR ledgers were not rewritten. The V071–V076 narrative moved to `docs/migration/history-v071-v076-project-os.md` with a history-only notice. Historical data never regains runtime authority. |
| Replacement product name, PR15 scenarios/acceptance, and any future AlRos14 package publication | **Futura** | No name or unimplemented capability is invented. PR15 remains unstarted. |
| ForgeAILab origin, MIT notice, `docs/upstream.md`, retained `npx-cli/`, and upstream npm/Homebrew publication jobs | **Upstream** | Preserved with attribution. The npm wrapper is explicitly marked upstream-owned. Release jobs for npm and Homebrew now run only in `ForgeAILab/forge`; CI's npm job is labeled as an upstream snapshot and only packs the retained wrapper. |
| Public brand `Forge`, `AlRos14/forge`, `forge` server, `forge-ctl`, `forge-cli`, logos, web title/metadata, Cargo repository metadata and installer repository | **Identidad pública** | Updated copy and repository links to this fork. Existing Forge logo/name remain pending an owner-supplied name. No mass replacement was made. |
| `FORGE_*`, `~/.forge`, `forge.yaml`, `forge.db`, `jwt_secret.bin`, `server.json`, `forge_ctl_credentials.json`, worktree/log/media locations, database schema/migration IDs, binary names, HarnessSession identities, UUIDs and persisted foreign references | **Compatibilidad persistida** | Inventoried and unchanged. A future rename affecting these requires explicit discovery and data-preservation design; no reset, migration or compatibility layer was added. |
| GitHub Releases, npm/Homebrew, Docker/GHCR and `install.sh` | **Identidad pública / distribución** | No AlRos14 GitHub Release existed at audit time; no own npm/Homebrew channel is verified. Source build is the current documented path. `install.sh` targets this repository but cannot install until a matching release exists. GHCR package inventory is NOT VERIFIED because the current GitHub token lacks `read:packages`; no absence claim is made. |

### Surfaces inspected

- Product landing page, images, badges and documentation links: `README.md`, `assets/`, web metadata.
- Commands, crates, manifests and package metadata: workspace `Cargo.toml`, `crates/forge-cli`, `crates/forge-client`, `npx-cli/`, root and web package manifests.
- Distribution: `Dockerfile`, Compose files if present, `install.sh`, release and CI workflows, tag/build matrices, GitHub repository/release links, npm/Homebrew/GHCR references.
- Runtime identities: `FORGE_*`, config and data paths, secrets, credentials, logs, workspaces, database and migration files, persisted IDs and histories.
- Current entry points: Clap command definitions/help, REST route and DTO registries, MCP descriptors, SSE event projections, web route/navigation sources, and relevant tests.

## Architectural contracts in scope

`docs/migration/architecture-v2.md`, INV-001–INV-037, domain contracts in `docs/concepts/`, and the PR11–PR13 ledgers remain authoritative. PR14 documentation preserves:

- Human and harness-bound Agent Actors; Task-scoped TaskRoles and RoleMembership authority.
- TaskLifecycle as the only Task state authority; explicit WorkUnit admission and immutable Execution identity.
- Harness-owned cognition behind HarnessAdapter; Forge-owned authorization, work records, collaboration, evidence, and lifecycle.
- WorkUnit dependency and workspace isolation; explicit serialized integration; Workspace-free non-repository executions only within the supported host/provider limits.
- Durable event-driven Orchestrator actions, provider Start receipts, exact uncertain-start recovery, and no automatic repeat Start.
- Distinct ReviewReport, deterministic ValidationRun, Evidence, and Gate responsibilities.
- History preservation without restoring old runtime authority, plus upstream attribution without an architectural-compatibility promise.

## Requirement → implementation → evidence matrix

| Requirement | Implementation | Evidence | Status |
|---|---|---|---|
| G1: merge PR13 normally before PR14 | PR #16 merged as `0afe361...`; PR14 branch starts at its `origin/main`. | Merge parents, merge-base, PR state, ancestry and V121–V134 verified. | PASS |
| G1: inspect remote CI and preserve exact status | Actions/workflows checked; PR13 had 0 runs and 0 check runs even after the temporary validation-ref push. | Owner-authorized local substitution; PR13 ledger evidence referenced above. | NOT RUN (CI); local evidence authorized |
| README reflects actual product and installation | Rewrote short landing page, domain-record summary, capability limits, links and source-build instructions. | Compared to architecture, live routes/CLI, manifests and release state; no obsolete screenshots or inherited badges remain. | PASS |
| Getting Started documents a complete, reproducible path | Source build, daemon discovery, Agent/profile, Project/Repo/Task, TaskRole, RoleMembership, explicit WorkUnit, Execution, result/review/gate, integration and close. | Focused order regression check and all thirteen Bash blocks pass; live baseline walkthrough returned HTTP 500/SQLite 517 after commit. A deterministic two-connection DB test reproduces that failure in deferred `finish()` and passes with the focused `BEGIN IMMEDIATE` fix. No uncertain POST was retried; full live REST walkthrough after the fix was NOT RUN. | PARTIAL (document/order and DB boundary pass; full REST walkthrough NOT RUN after fix) |
| Architecture describes post-PR13 state | Replaced transition-era model with current TaskLifecycle, roles, WorkUnits, executions, exact provider authority, persistence/history, paths and upstream boundaries. | Compared to `architecture-v2.md`, PR11–PR13 ledgers, migrations and active service/API code. | PASS |
| REST, MCP, CLI, SSE and logs match active surfaces | Updated route/error/resource notes, all active MCP tool names, CLI commands/flags, SSE/log and Workspace-free restrictions. | Source registries and tests reviewed; `forge-ctl --help` PASS; `cargo check -p mcp-server` PASS. The original REST failure is deterministically reproduced and repaired at the operation-finalization boundary; no post-fix REST create was submitted. MCP live smoke NOT RUN. | PASS (contracts and focused repair); full REST/MCP live smoke NOT RUN |
| Web and developer entry points are accurate | Updated actual web route/build guidance, contributor workflow, CLAUDE/AGENTS guidance and server help text. | `AGENTS.md` is a symlink to updated `CLAUDE.md`; server help PASS; TypeScript and Vite build PASS. | PASS |
| Distribution belongs to the right repository | Cargo and installer metadata point to `AlRos14/forge`; upstream npm/Homebrew jobs are gated to ForgeAILab; source build is the only verified install path. | Workflow YAML parsed; release workflow and repository release state inspected. GHCR inventory unavailable with current token scope. | PASS with GHCR NOT VERIFIED |
| Product name and persisted IDs protected | No brand, crate, command, package, path, schema, table or ID rename. Rename dependencies classified above. | Search over public/package/runtime surfaces; data paths and manifests compared; no migrations or identifiers changed. | PASS |
| Upstream license, attribution and historical docs preserved | Upstream notice retained; MIT file and historical records unchanged; V071–V076 moved without changing its historical claims. | Diff check and attribution search; historical source text compared. | PASS |
| PR15 excluded | No reference scenarios or broader reliability/acceptance work. The separately authorized WorkUnit fix is limited to the confirmed SQLite transaction-upgrade cause. | Scope/diff audit and changed-file inventory. | PASS (focused fix only) |

## Changes and scope decisions

- Rewrote `README.md`, `docs/getting-started.md`, `docs/architecture.md`, `docs/api.md`, and `docs/cli.md`; corrected `docs/execution-logs.md` and `web/README.md`.
- Rewrote current development/operations guidance in `CONTRIBUTING.md`, `CLAUDE.md` (also `AGENTS.md` via symlink), and `docs/release_checklist.md`.
- Added `docs/migration/history-v071-v076-project-os.md`; preserved original migration/ADR/PR ledgers and upstream/license documents.
- Updated Unreleased changelog and clarified that historical source entries are not proof of this fork's releases.
- Updated Cargo repository metadata, installer target, browser title/description, and product-help/Start descriptions.
- Restricted inherited npm and Homebrew publishing to the upstream repository.
- Set `FORGE_SERVER_BIND=0.0.0.0:8080` in the container to make its existing `EXPOSE 8080` reachable through Docker port mapping. Set `make dev` to `127.0.0.1:8080`, matching the existing Vite proxy. Aligned Docker's temporary Cargo manifest/source cache with the current workspace and raised its builder to Rust 1.89 for stable file-lock APIs. These are narrow packaging/developer-entry corrections; no service/API authority changed.
- After the original live POST returned 500 with SQLite 517 despite a committed WorkUnit, reproduced the deferred transaction's stale-snapshot upgrade failure across two file-backed SQLite connections. `TaskIntegrationOperationRepo::finish` now acquires its write reservation with `BEGIN IMMEDIATE` before reading the operation. The database tests confirm the WorkUnit stays committed, the old transaction ordering returns code 517 under a barrier-controlled concurrent commit, and the immediate transaction completes while serializing the competing writer. This is the narrow correction explicitly authorized during review; no API/schema change or blind create retry was added.
- Added a focused Getting Started order check to CI. The guide now requires completed Execution → current WorkUnit/version → `completed` status → integration using the exact Execution ID and stable idempotency key → outcome handling → TaskLifecycle/Gates preflight. `make` sets the loopback bind default only for its three local server targets; tests and builds inherit no fixed port unless the caller explicitly supplies one.
- Corrected a static JSX apostrophe in `ChangeModelDialog.tsx` to satisfy the Web lint rule; rendered text and runtime behavior are unchanged.
- No migration, schema, persisted path, `FORGE_*` variable, crate/bin/package rename, or data rewrite.

### Separately classified release-packaging finding

The first authorized local `docker build` failed before compilation because
`Dockerfile` copied `crates/review/Cargo.toml`, which is absent from current
`origin/main` after PR13. Comparing the Docker manifest cache to the current
Cargo workspace also showed that the active `crates/forge-daemon` manifest was
omitted. The tag workflow's `container` job builds and publishes this image to
GHCR, so the stale workspace list prevents that declared release job from
building. This is a **P1 distribution defect**, not a documentation preference.
The plan explicitly requires checking the actual distribution path and
separately classifying functional corrections. The correction is restricted to
the Dockerfile's manifest cache and placeholder source list; it adds no product
capability or runtime domain behavior. The Dockerfile's workspace manifest cache now matches the current Cargo members; the local image build and container smoke both pass after the follow-up Rust base correction.

After fixing the workspace list, the image build reached Rust compilation and
failed because the builder used Rust 1.88. Current `services` code calls
`std::fs::File::try_lock` and handles `TryLockError`, APIs stabilized in Rust
1.89.0 ([official standard-library docs](https://doc.rust-lang.org/std/fs/struct.File.html)); CI already uses the current stable toolchain. This is the second cause of the same P1 release-image failure. The Docker builder base is updated to `rust:1.89-bookworm`, the first stable version that supports these APIs. The corrected `rust:1.89-bookworm` image build and ephemeral container smoke passed; the image answered `/healthz` and served the web root through its published port.

## Decisions and pending information

- Replacement product name: **not provided**. Rename is not executed. An exact name plus reviewed public/persisted impact is required.
- AlRos14 GitHub Releases: none existed at the 2026-10-16 audit; source builds are documented as supported.
- Upstream npm wrapper remains in-tree for provenance and CI snapshot checks; it is not described as this fork's installer.
- GitHub private vulnerability reporting for `AlRos14/forge` was disabled at audit time. `.github/SECURITY.md` no longer points vulnerability reports to upstream or asks users to post them publicly. The owner must enable private reporting or supply a private contact channel. This repository-setting change is pending outside this PR.
- GitHub package API returned 403 because the authenticated token lacks `read:packages`; GHCR package inventory is NOT VERIFIED.
- Corepack's `pnpm` shim attempted to resolve `pnpm/latest` and DNS resolution to `registry.npmjs.org` failed. The installed locked `node_modules` binaries ran the exact package build scripts directly and passed.

## Validation record

| Check | Result |
|---|---|
| `git diff --check` | PASS after final edits (re-run before publication). |
| Internal Markdown links and heading anchors | PASS: 45 local links; one stale Getting Started anchor was found and corrected before final pass. |
| Getting Started order regression | PASS: `python3 scripts/check-getting-started-order.py`; CI job is wired to run the same focused check. |
| Bash examples in Getting Started | PASS: all 13 fenced Bash blocks pass `bash -n`. |
| Makefile bind scoping | PASS: `make -n dev dev-demo dev-no-daemon test build ci ci-rust ci-web frontend`; fake Cargo/pnpm environment checks confirm all three server targets receive `127.0.0.1:8080`, explicit `127.0.0.1:9999` is preserved on all three, and `test`, `build`, `ci`, `ci-rust`, `ci-web`, and `frontend` receive `FORGE_SERVER_BIND=UNSET`. These checks mocked executables and did not compile Rust. |
| Current docs stale-capability review | PASS: retired names occur only as explicit retirement/history context; changelog and migration records remain historical. |
| Upstream/MIT attribution and persisted identity census | PASS: `docs/upstream.md`, `LICENSE`, migration histories and IDs unchanged; no unauthorized rename. |
| `cargo run --locked -p forge-client -- --help` | PASS; displayed current `forge-ctl` command groups and options. |
| `FORGE_SKIP_WEB_BUILD=1 cargo run --locked --offline -p forge-cli -- --help` | PASS; displayed updated Forge product description. |
| `cargo check --locked --offline -p mcp-server` | PASS. |
| `./node_modules/.bin/tsc -b && ./node_modules/.bin/vite build` in `web/` | PASS. Existing warnings: caniuse data is six months old and one generated chunk exceeds 500 kB; no warning indicates this metadata change failed. |
| `bash -n install.sh && make -n dev` | PASS. |
| PyYAML parse of `.github/workflows/ci.yml` and `.github/workflows/release.yml` | PASS. |
| `cargo fmt --all -- --check` | PASS after formatting the focused DB change. |
| WorkUnit create/operation-finish reproduction and repair | PASS: `cargo test -p db --lib finish_ --locked --offline -- --nocapture --test-threads=1` runs two file-backed, barrier-controlled tests. The deferred `finish()` path reproduces SQLite code 517 (`database is locked`) after the real `WorkUnitRepo::create` commit and a second connection's committed write. The `BEGIN IMMEDIATE` path succeeds while a zero-timeout competing writer confirms `SQLITE_BUSY` until finalization commits, then writes successfully. No REST create was retried. |
| Targeted Web lint | PASS: `./node_modules/.bin/eslint src/features/federation/ChangeModelDialog.tsx --no-cache --format stylish`; the fix escapes only the static apostrophe. |
| Local `cargo audit` | NOT RUN: `cargo-audit` is not installed (`cargo audit --version` reports no such Cargo subcommand). The recorded nine findings come from the original-head GitHub Actions audit output; this environment was used only to inspect its dependency graph and the official advisory records. |
| CI workflow YAML parse | PASS: PyYAML parsed both CI and release workflows after adding the documentation contract job. |
| Docker image build/runtime | PASS: after two diagnostic failures (stale workspace list; Rust 1.88 E0658), final `docker build --tag forge-pr14-validation:local .` succeeds on Rust 1.89; ephemeral container returns `/healthz` = `ok` and serves `<title>Forge</title>` over published port 32778. No host data volume. |
| Isolated live Getting Started REST walkthrough | ORIGINAL HEAD FAIL after 9 successful setup operations: `POST /tasks/f464be02-ef47-47fb-abb5-5923574931bc/work-units` returned HTTP 500 `internal_error`, SQLite code 517 `database is locked`, after 34 ms. Read-only inspection confirmed WorkUnit `36d54cbe-ccf4-4415-94fb-a1ae655a1508` committed (version 1). No retry was sent because creation is non-idempotent. The focused repository-layer fix and deterministic reproduction pass; a fresh full HTTP walkthrough after the fix is NOT RUN. |
| MCP live smoke and workspace-wide Rust tests | NOT RUN; PR14 changed no MCP schema or route. CI Clippy failed before workspace check/tests; PR15 retains integration/reliability acceptance. |
| PR14 GitHub Actions at original audited HEAD — push run [38888725568](https://github.com/AlRos14/forge/actions/runs/38888725568) and PR run [38888783654](https://github.com/AlRos14/forge/actions/runs/38888783654) | FAIL at SHA `0a06faac...`. Upstream npm Bootstrapper Snapshot passed in both; CodeQL passed in PR run [38888783625](https://github.com/AlRos14/forge/actions/runs/38888783625). Rust Clippy 1.99 with `-D warnings` failed with 398 diagnostics before workspace check/tests: 394 `double_must_use`, 2 `large_enum_variant`, 1 `io_other_error`, and 1 `useless_conversion`, in then-untouched `executors` (10 total) and `db` (388 total). Web lint failed on the static apostrophe in `ChangeModelDialog.tsx:143:31`; this line is corrected locally and its file lint now passes. `cargo audit` scanned 401 lockfile dependencies and reported nine vulnerabilities plus four allowed warnings, detailed below. These are results from the original audited SHA; Actions for the correction commit are NOT VERIFIED. |

## Risks, findings and limitations

- Final unresolved findings: **P0: 0; P1: 1; P2: 2; P3: 2**. The Docker release build defect and the confirmed WorkUnit create/finish defect are repaired. The remaining P1 is the dependency audit gate.
- **WorkUnit create/finish defect — repaired, end-to-end acceptance partial.** The original live response remains recorded as HTTP 500/SQLite 517 after the WorkUnit committed; the POST was never retried. The deterministic test confirms a deferred transaction reads the durable operation, another connection commits a write, and the first transaction then fails to upgrade its stale WAL snapshot with code 517. Production `finish()` now begins `IMMEDIATE`, reserving the write lock before reading. The matching test confirms successful finalization and serialization with the competing writer. This establishes the database boundary cause and fix without claiming a post-fix HTTP walkthrough. Getting Started's order and DB-boundary checks are accepted; the full live REST path remains **NOT RUN after the fix**, so acceptance is **PARTIAL**. PR15 may still add broader reliability coverage, but the confirmed 517 defect is not left open.
- **P1: dependency audit gate.** The original-head Actions audit scanned 401 lockfile dependencies and found nine vulnerable package/advisory pairs in two locked crates:

  | Locked crate | Findings and corrected floor | Dependency scope |
  |---|---|---|
  | `quinn-proto 0.11.15` | [RUSTSEC-2026-0341](https://rustsec.org/advisories/RUSTSEC-2026-0341.html) (HIGH), [RUSTSEC-2026-0339](https://rustsec.org/advisories/RUSTSEC-2026-0339.html), and [RUSTSEC-2026-0342](https://rustsec.org/advisories/RUSTSEC-2026-0342.html): `>=0.11.17`; [RUSTSEC-2026-0340](https://rustsec.org/advisories/RUSTSEC-2026-0340.html) (HIGH) and [RUSTSEC-2026-0338](https://rustsec.org/advisories/RUSTSEC-2026-0338.html): `>=0.11.18`. Combined floor: `>=0.11.18`. | `cargo tree --workspace --invert quinn-proto@0.11.15` has no active default-graph inverse. It is present in `Cargo.lock` via Reqwest's optional `http3`/Quinn dependency; current Forge Reqwest declarations do not enable `http3`. The audit still scans it because it is locked. The two HIGH advisories are network denial-of-service issues in Quinn transport behavior. |
  | `rustls 0.23.38` | [RUSTSEC-2026-0348](https://rustsec.org/advisories/RUSTSEC-2026-0348.html) (LOW), [RUSTSEC-2026-0349](https://rustsec.org/advisories/RUSTSEC-2026-0349.html) (LOW), and [RUSTSEC-2026-0346](https://rustsec.org/advisories/RUSTSEC-2026-0346.html) (MEDIUM): `>=0.23.46`; [RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285.html) (MEDIUM): `>=0.23.45`. Combined floor: `>=0.23.46`. | Active through Reqwest/Hyper-Rustls in `cli-adapters`, `forge-client`, and `services`; Tokio-Rustls through client/API paths; and SQLx's SQLite Rustls runtime in `db`/services. The affected Rustls `brotli` feature is not enabled in the current workspace feature graph. |

  Upgrade the lockfile to the combined fixed floors in a separately reviewed dependency-maintenance change, inspect the resolved reverse graph/features, then run `cargo audit` and the relevant TLS/client/API tests. Do not add broad audit exceptions. The HIGH Quinn advisories and failing required Security Audit are merge blockers while present in the lockfile. Rustls confirms fixed floors at [0.23.45](https://rustsec.org/advisories/RUSTSEC-2026-0285.html) and [0.23.46](https://rustsec.org/advisories/RUSTSEC-2026-0348.html).
- The same audit reported four existing allowed warnings: `unsafe-libyaml 0.2.11` unmaintained (RUSTSEC-2024-0449), `anyhow 1.0.102` unsound (RUSTSEC-2026-0190), `event-listener 5.4.1` unsound (RUSTSEC-2026-0221), and yanked `spin 0.9.8`. Track them individually in dependency maintenance; no exception was added.
- **P2: 2** — (1) private vulnerability reporting is disabled for this repository; the policy states the limitation without redirecting users upstream, and owner action remains necessary; (2) Rust CI Clippy 1.99 fails under the existing `-D warnings` baseline before workspace check/tests. The original audited SHA produced 398 diagnostics: 394 `double_must_use` (async-trait `#[must_use]` on already must-use Results), 2 `large_enum_variant`, 1 `io_other_error`, and 1 `useless_conversion`; they were in then-untouched `executors` (10 total) and `db` (388 total). Fix the source warnings in a separate focused maintenance change, beginning with the repeated `async_trait` diagnostic, then rerun `scripts/ci-rust.sh`; do not remove `-D warnings`. This gate blocks merge until green or separately reviewed.
- The Web CI failure on original HEAD was a static `Agent's` apostrophe rejected by `react/no-unescaped-entities`. It is corrected to `Agent&apos;s`; targeted ESLint passes and the change has no runtime behavior. A new Actions run for the correction commit has not been verified yet.
- **P3: 2** — Vite reports stale Browserslist data and a generated chunk above 500 kB. The build succeeds; these pre-existing maintenance warnings are outside PR14's documentation/presentation scope.
- Distribution inventory is limited by missing GitHub token `read:packages` scope. No claim is made about whether an image is already present in GHCR.
- The Docker release build was repaired after two concrete failures: a stale crate manifest and a Rust 1.88 compiler older than the stable file-lock APIs required by `services`. The final Rust 1.89 image build and ephemeral port/health smoke pass. The authorized build was local-only and did not publish an image.
- The original live Getting Started walkthrough used only a fresh `/tmp` data directory, workspace root, and temporary repository; no user data was accessed. Its uncertain create result was inspected read-only and never retried. The focused database test uses a fresh file-backed SQLite database and barriers; no production data or host service was accessed.
- PR13's missing GitHub CI remains NOT RUN despite authorized local substitute; no CI PASS is claimed. PR14 Actions at the original audited SHA had CodeQL and the upstream npm snapshot pass; Rust, Web, and Security Audit failed as detailed above. New Actions results after this correction commit are NOT VERIFIED.

## Final review state

Completed the requirement, contract, compatibility, attribution, distribution, priority, and PR15 scope audits. The WorkUnit transaction defect is repaired with a deterministic regression test; Getting Started acceptance remains PARTIAL because a full fresh live REST walkthrough was not run after the fix. Merge decision: **NO-GO** while the required Rust Clippy and Security Audit checks fail; the targeted Web lint now passes locally but has no verified post-correction Actions result. PR #17 is intended to remain open as a draft against `main`; it is not merged. The final local and published branch SHAs, CI visibility, and worktree status are recorded in the final delivery report.
