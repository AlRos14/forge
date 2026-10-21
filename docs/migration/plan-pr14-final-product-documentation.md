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
| `git diff --check` | PASS after final edits and again on the staged diff immediately before commit/publication. |
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
| Local `cargo audit` | NOT RUN on the original PR14 branch: `cargo-audit` was unavailable there. This inherited gate was repaired separately in PR18; its final Actions Security Audit passes, and its recorded fresh local audit found 0 vulnerabilities with two remaining warnings (see current evidence below). |
| CI workflow YAML parse | PASS: PyYAML parsed both CI and release workflows after adding the documentation contract job. |
| Docker image build/runtime | PASS: after two diagnostic failures (stale workspace list; Rust 1.88 E0658), final `docker build --tag forge-pr14-validation:local .` succeeds on Rust 1.89; ephemeral container returns `/healthz` = `ok` and serves `<title>Forge</title>` over published port 32778. No host data volume. |
| Isolated live Getting Started REST walkthrough | ORIGINAL HEAD FAIL after 9 successful setup operations: `POST /tasks/f464be02-ef47-47fb-abb5-5923574931bc/work-units` returned HTTP 500 `internal_error`, SQLite code 517 `database is locked`, after 34 ms. Read-only inspection confirmed WorkUnit `36d54cbe-ccf4-4415-94fb-a1ae655a1508` committed (version 1). No retry was sent because creation is non-idempotent. The focused repository-layer fix and deterministic reproduction pass; a fresh full HTTP walkthrough after the fix is NOT RUN. |
| MCP live smoke and workspace-wide Rust tests | NOT RUN locally for PR14; PR14 changed no MCP schema or route and the owner deferred broader reliability acceptance to PR15. The inherited CI gate was repaired and passed on PR18. No full workspace suite was repeated locally. |
| PR14 GitHub Actions at original audited HEAD — [push](https://github.com/AlRos14/forge/actions/runs/38888725568), [PR](https://github.com/AlRos14/forge/actions/runs/38888783654), [CodeQL](https://github.com/AlRos14/forge/actions/runs/38888783625) | HISTORICAL FAIL at `0a06faac...`; Rust Clippy, Web lint, and Security Audit failures were inherited baseline issues and are superseded by PR18. |
| PR14 GitHub Actions at correction SHA `e5db5bf6498e536c6e831bafcefe9afae41fd70e` — [push](https://github.com/AlRos14/forge/actions/runs/39065756747), [PR](https://github.com/AlRos14/forge/actions/runs/39065759791), [CodeQL](https://github.com/AlRos14/forge/actions/runs/39065759776) | HISTORICAL CI FAIL / CodeQL PASS. Rust Clippy, three stale Web assertions, and Security Audit failures were resolved by PR18; this row is not the current PR14 head status. |
| PR18 CI at reviewed head `49441b70b93248e20de05c61865ee64763657026` — [CI](https://github.com/AlRos14/forge/actions/runs/39568592986), [CodeQL](https://github.com/AlRos14/forge/actions/runs/39568592988) | PASS: Rust, Web, Security Audit, npm Bootstrapper, and CodeQL. The PR records a fresh local audit with 0 vulnerabilities; `unsafe-libyaml 0.2.11` and yanked `spin 0.9.8` remain warnings. Normal merge commit on `main`: `87c22862917a6059308cfb2a5dcb820cfcfb66ab`. |
| PR14 integration of updated `origin/main` | PASS: normal merge commit `271bb274f7a9019d9f568b989736efbce530670d`, merge base `87c22862917a6059308cfb2a5dcb820cfcfb66ab`; the only conflict was `CHANGELOG.md`, resolved by retaining both sides. `git diff --check` passes. PR14 Actions on the newly published head will be checked by GitHub after push; no local CI suite was repeated. |
| PR14 Actions on first integrated head `09cb91e81beb630e4fc9c7f3d3e93eb5735d773a` — [CI](https://github.com/AlRos14/forge/actions/runs/39579332444), [push CI](https://github.com/AlRos14/forge/actions/runs/39579331736), [CodeQL](https://github.com/AlRos14/forge/runs/119153257394) | Web, Security Audit, Documentation Contract, npm Bootstrapper, and CodeQL PASS. Rust Clippy found one `unnecessary_operation` in the PR14 SQLite test fixture: the `.record` field access had no effect. It was removed while retaining the WorkUnit creation call and `.expect`; a new GitHub run is required on the corrected head. |


## Current risks, findings and limitations (2026-10-21)

The bounded review found no P0/P1/P2 product or contract regressions. The first integrated PR14 CI run exposed one directly blocking test-only Clippy lint, corrected by removing a no-effect `.record` field access; GitHub must rerun on the corrected head. The older Clippy baseline, Web-test, and Security Audit failures are superseded by PR18.

- **WorkUnit create/finalization — fixed; full HTTP walkthrough deferred.** The original isolated request returned HTTP 500 / SQLite 517 after the row committed and was not retried. The focused file-backed, barrier-controlled DB tests reproduce the deferred-transaction failure and pass with `BEGIN IMMEDIATE`. The owner explicitly removed a fresh full HTTP walkthrough as a PR14 closure requirement; it remains NOT RUN after the fix and belongs to PR15 system acceptance.
- **CI baseline — repaired in PR18 and merged.** PR18 updates `async-trait`, fixes the two large enum payloads, uses the equivalent IO error constructor, removes one redundant conversion, aligns three stale Web assertions, and updates affected TLS lockfile versions without dropping HTTP/3. Rust, Web, Security Audit, npm Bootstrapper, and CodeQL pass on PR18's reviewed head. Its recorded fresh local audit found 0 vulnerabilities; unmaintained `unsafe-libyaml 0.2.11` and yanked `spin 0.9.8` remain warnings for later maintenance, not PR14 blockers.
- **Source compatibility — accepted.** On 2026-10-21 the owner expressly accepted the documented source-level `Box<Execution>` change in the two public DB enum variants for beta 0.x. The `CHANGELOG.md` breaking entry is preserved in the conflict resolution; persisted data and REST contracts are unchanged by that adjustment.
- The Docker image build/runtime and local Makefile bind corrections have focused PASS evidence above. No distribution was published; no product name was selected or renamed.
- Bounded merge review: the only conflict was `CHANGELOG.md`; its resolution retains both sides' entries. PR14 changes no migration file; `LICENSE`, `docs/upstream.md`, `docs/migration/architecture-v2.md`, `docs/concepts/`, and persisted identities are unchanged against updated `main`. No additional P0/P1/P2 regression from the combination was found.
- Deferred inherited items for the post-PR15 stabilization pass: GitHub private vulnerability reporting was previously observed disabled; stale Browserslist metadata and the large Vite chunk were non-failing warnings. Their current settings are not asserted here. PR15 implementation has not begun in this ledger revision.

## Final review state

PR14 has incorporated PR18's merged `main` by normal merge. The one newly exposed Clippy error in its test fixture is corrected and the corrected published head is awaiting GitHub checks. The fresh post-fix HTTP walkthrough is NOT RUN and is not a PR14 gate by owner direction. The bounded review found no PR14-scoped P0/P1/P2 blocker; the name remains Forge pending an explicit replacement. PR #17 remains open and unmerged; it will be marked ready for human review only after the updated branch is pushed and its required checks are green. PR15 remains outside this PR and begins after PR14 is cleared for normal merge.
