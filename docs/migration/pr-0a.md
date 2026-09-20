# Plan PR0A — Reconcile pre-migration operational fixes

Status: completed and merged. Plan PR0 / Repo PR #3 and Plan PR0A / Repo PR #4
are closed; Plan PR1 is ready to begin.

## Scope

This Plan PR reconciles operational work that was present after the Plan PR0 audit:

* port lossless execution-log rotation and compression from `71f478b`;
* keep small Cursor prompts on direct argv transport and move only oversized
  prompts to private, randomized runtime control files with deterministic
  cleanup and a bounded stale-file TTL;
* preserve explicit executable and environment overrides without shell-alias
  discovery;
* keep Codex native rate-limit events and bounded, cancellable Cursor quota
  polling observable with execution/account provenance;
* isolate the shared Cursor adapter's short usage cache by effective query
  configuration, without cross-configuration stale fallback;
* make remote/daemon-bound manual usage refresh explicitly unsupported instead
  of reporting an old observation as a successful refresh;
* make WorkspaceLease renewal independent of harmless Task revisions while
  retaining fail-closed authority checks;
* record the reconciliation decisions for the other local commits and legacy
  Repo PR #2.

This PR does not introduce ActorRef, multi-actor roles, HarnessSession,
ExecutionPurpose, HarnessAdapter, WorkUnit, plan Artifacts, review Executions,
or a new workflow/orchestration model. Existing legacy planning, review,
workflow, and session paths remain only as pre-migration code owned by their
  specified replacement Plan PRs; this Plan PR does not extend them.

## Invariants touched

The implementation touches INV-003, INV-008, INV-010, INV-012, INV-013,
INV-017, INV-030, INV-033, and INV-034.

* Execution configuration snapshots include the Agent's explicit daemon
  binding separately from scheduler-resolved routing. The resolved daemon is
  also retained as factual execution provenance when host-local credentials
  are consumed.
* Account quota keys use the harness kind, credential context, and the host
  that owns host-local credentials. A lexical `CODEX_HOME` value is never
  canonicalized through the server filesystem, and an executable/wrapper path
  is never used as account identity.
* Native Codex rate-limit events are persisted as provider events. Cursor
  `/usage` observations are persisted as `cursor_poll`, making polling
  explicitly non-native. Remote executions use the daemon-side observation
  path rather than probing the server's account. Cursor adapter cache entries
  are keyed by effective executable, arguments, and environment; failed
  probes never reuse another configuration's value.
* Lease authority remains deterministic and continues to check execution,
  principal, assignment, repository, capability, lifecycle, governance, and
  revocation. The issued Task revision remains historical provenance, not a
  revocation signal for unrelated metadata edits. When an explicit legacy role
  assignment exists, it takes precedence over the task-level fallback.

## Reconciliation ledger

| Source | Decision | Owning follow-up or proof |
| --- | --- | --- |
| Legacy Repo PR #2: Cursor large-prompt transport | Adapted. Prompts at or below 32 KiB remain direct argv input. Larger prompts are written to a private randomized directory under the system runtime directory, exposed through Cursor's `--add-dir` capability, and passed by a short instruction. Unix permissions are 0700/0600; RAII cleanup handles normal completion and launch failure, while a 24-hour stale-directory sweep handles hard-crash leftovers. Tests prove the worktree receives no control file or diff entry, an adapter fixture proves the configured Cursor process can read the external file, and an authenticated one-turn smoke run passed in a disposable workspace. | Plan PR3 will move this launch behavior behind `HarnessAdapter`; the current Cursor CLI smoke probe confirmed `--add-dir` and no documented prompt-file/stdin option. |
| Legacy Repo PR #2: executable/environment overrides | Kept in the existing explicit `CommandOverrides` contract. A raw executable path, wrapper, and environment map are reproducible inputs. Interactive shell alias parsing was not ported. | Plan PR3 owns the adapter boundary. |
| Legacy Repo PR #2: account usage/live quota | Adapted. Codex consumes native `account/rateLimits/updated`; Cursor uses bounded, cancellable PTY polling with the explicit `cursor_poll` source. Local and daemon execution share the server-side extraction/persistence boundary; daemon Cursor polling runs where the CLI/account exists. Each observation records execution, source, account key, and host/daemon provenance. Unpinned remote Agent usage resolves the newest execution-linked observation and returns its actual account/daemon instead of inventing a shared pool. The Cursor adapter cache is configuration-keyed and cannot cross-fallback on probe failure. | Plan PR3 will expose dimensional adapter capability/usage metadata. |
| Legacy Repo PR #2: WorkspaceLease false invalidation | Adapted in V086 and the service verifier. Task revision is retained for audit, while harmless description/metadata revisions do not revoke authority. Explicit role assignment takes precedence over a stale task fallback; repository, capability, lifecycle, governance, and revocation changes still fail closed. | Plan PR5 will bind final lease authority to the new Actor/WorkUnit/workspace model. |
| Legacy Repo PR #2: quota reassignment/session continuation | Not ported. Singular-role and inferred-session behavior would violate Plan PR1/Plan PR2. The requirement is carried forward: a reassigned execution must use the new Actor and an explicitly selected compatible session, never the previous Actor's latest session. | Plan PR1 and Plan PR2; acceptance hardening in Plan PR15. |
| Legacy Repo PR #2: optional plan-review state machine | Discarded. No `PLAN_REVIEW`, planner/reviewer loop, plan-review retry, or coupled UI/API was added. | Plan PR7 provides plan Executions, Artifacts, and optional Gates/Decisions. |
| Legacy Repo PR #2: UI/API changes | Split by responsibility. The plan-review checkbox, plan-review task-state/API payload, planner/reviewer loop controls, and related SSE/UI behavior are discarded with that state machine. Explicit executable/environment configuration, truthful usage provenance, and the Agent usage response are adapted here; the current legacy `re-execute` endpoint remains documented as transitional behavior, while inferred resume/reassignment semantics are deferred. | Plan PR2 owns explicit sessions; Plan PR7/Plan PR8/Plan PR12 own the replacement plan, review, and public-surface contracts. |
| Legacy Repo PR #2: shell alias introspection | Discarded. Forge does not inspect shell startup files or infer account identity from aliases. | Explicit `CommandOverrides` is the supported contract. |
| `71f478b`: execution log rotation | Ported selectively, without its old documentation changes. Rotated gzip segments preserve one logical sequence, historical reads, bounded tails, and final compaction. The active base path is not itself a complete historical export; relocation is protected by process-local path locks and service execution ownership. | Log storage remains infrastructure; no domain cognition depends on it. |
| `509205a`: workflow resume | Deferred. Its implementation relies on workflow roles and inferred `agent_session_id` continuity. | Plan PR2 (explicit session) and Plan PR9 (aggregate lifecycle). |
| `3d291dd`: current-role re-execution | Deferred. Its implementation relies on one singular current role assignment. | Plan PR1 (multi-actor membership) and Plan PR2 (explicit Actor/session identity). |

## Persistence and compatibility

### Workspace lease

* Old writer: `TaskService`/`WorkspaceLeaseRepo::issue` writes
  `workspace_lease.task_version` at admission.
* New writer: unchanged; the issued version remains an immutable audit field.
* Old reader: the SQLite renewal trigger and service verifier treated that
  field as current authority.
* New reader: migration `V086__workspace_lease_revision_independence.sql`
  and `TaskService::verify_active_workspace_lease` use live authority facts,
  not an unrelated Task revision.
* Authority: live execution/principal/assignment/repository/capability/
  lifecycle/governance checks remain authoritative.
* Cleanup: final lease/schema replacement belongs to Plan PR5 and destructive
  compatibility cleanup to Plan PR13.

### Account usage

* Old writers: adapter terminal results, daemon execution notifications, and
  agent refresh routes wrote `account_usage_snapshot`; execution snapshots
  sometimes omitted the host that owned the CLI credentials.
* New writers: execution/daemon observation paths write a host-aware account
  key. An explicit Agent daemon binding and the scheduler-resolved daemon are
  recorded separately; the resolved daemon is factual provenance and
  participates in quota identity when credentials are host-local. An explicit
  durable credential reference is the only transitional proof that the same
  account is shared across hosts. Native Codex events and Cursor polling use
  distinct sources. The compatibility Agent refresh route no longer writes
  snapshots: the current model does not contain a server-local Codex/Cursor CLI
  account, so it returns `409 usage_refresh_unsupported` rather than probing an
  unrelated server credential context.
  Daemon notifications for an unpinned remote Agent are accepted only from the
  scheduler-resolved daemon recorded in that Execution snapshot; other daemon
  senders are rejected before activity or usage persistence.
* Old readers: the agent usage API and execution usage projections continue to
  read `account_usage_snapshot`.
* New unpinned-remote reader: when an Agent has no daemon binding and no
  explicit credential reference, the usage endpoint joins snapshots through
  `execution.agent_id`, returns the newest actual observation, and exposes its
  `account_key` and `daemon_id`. It does not merge daemon-local pools or invent
  an `unresolved-daemon` identity. With no observation, both response fields
  are `null` and `available` is `false`.
* Authority: the execution snapshot plus `execution_id` and source provenance
  are the durable record. No dual-write source exists.
* Migration: `V087__cursor_poll_usage_source.sql` replaces the usage-source
  check constraint additively, preserving existing rows and adding the
  explicit `cursor_poll` provenance value. Historical migrations are not
  edited.
* Cleanup: Plan PR3 moves usage/capability semantics behind HarnessAdapter;
  Plan PR13 removes any transitional fields only after readers migrate.

`CODEX_HOME` is recommended as an absolute path in explicit launch
configuration. Absolute values are normalized lexically without reading the
server filesystem. Relative and `~` values remain opaque
(`relative:<configured-value>`); they are not expanded against the server's
`HOME` or current directory, because only the daemon that launches Codex can
interpret them. This preserves distinctions such as `../account` versus
`account` and `foo/../../bar` versus `bar` until a future credential reference
provides a stronger identity.

### Cursor control prompts

The prompt file is transient runtime state, not persistence. Small prompts stay
on the existing direct argv path. Oversized prompts are written to a private
per-prompt directory outside the worktree, with randomized names and Unix
0700/0600 permissions, and Cursor receives `--add-dir` for that directory plus
a short instruction. The file is removed when the adapter exits or fails to
launch; a bounded 24-hour stale-directory sweep recovers files left by a hard
process kill. The path is never included in execution Git state. The installed
Cursor CLI was checked for this exact capability, and a fake Cursor process
fixture now proves the real adapter launch can consume the external prompt. A
real authenticated one-turn plan-mode smoke run also completed successfully in
a disposable workspace, made no repository changes, and left no runtime
control directory. The smoke is validation evidence only, not a CI test.

## Dependency audit

The affected readers and writers were re-searched on the Plan PR0A branch before
editing:

* `account_usage_snapshot`: `services::task_service::execution`, remote
  notification handling, `services::account_usage`, API agent usage routes,
  Codex adapter events, and Cursor adapter polling;
* `resolved_daemon_id`/execution snapshots: task-service snapshot creation,
  routing outcome updates, execution launch/recovery, usage persistence, and
  daemon-side Cursor polling;
* WorkspaceLease: V077/V080 triggers, `WorkspaceLeaseRepo`, TaskService
  admission/verification/recovery, and DB lease tests;
* execution logs: executor adapters, daemon event transport, review context,
  workflow context, service recovery, API logs, and CLI daemon runtime;
* Repo PR #2 plan-review and alias code: reviewed but intentionally not copied.

## Validation and exit report

No legacy planning/review/workflow schema is dropped. No compatibility reader
is added, and no hidden second source of truth is introduced.

Validation performed on this branch:

* `cargo fmt --all -- --check` — passed after the final code changes.
* `FORGE_SKIP_WEB_BUILD=1 cargo check --workspace` — passed on a fresh
  recovery target directory.
* `FORGE_SKIP_WEB_BUILD=1 cargo clippy --workspace --all-targets -- -D warnings`
  — passed on the same fresh target; the web build was intentionally skipped
  for Rust validation.
* Focused tests passed: Cursor adapter tests (12), executor account-key tests
  (19), API unpinned usage (1), DB WorkspaceLease renewal (1), log-writer
  tests (7), daemon transport tests including unpinned remote usage and
  daemon-side Cursor polling, task-service execution tests (7), forge-client
  library tests (40), API execution-log tests (2), and API type-generation
  tests (501 passed, 1 ignored). The full Cursor adapter suite also passed
  (89 tests).
* `FORGE_SKIP_WEB_BUILD=1 cargo test --workspace --all-targets --no-fail-fast`
  completed but returned non-zero. The current run reported nine failed
  targets. Sandbox-restricted host-socket/filesystem cases were
  `api::daemon_connect`, `api::fs_daemon_routing`,
  `api::remote_execution_roundtrip`, the V076 media-write acceptance test,
  `forge-agent-host`, `forge-client::login_password`, and four services
  cases; the three API target suites, the V076 test, forge-agent-host (35),
  forge-client login/password (6), and all four services cases passed when
  rerun with the required host permissions. The remaining services log-path
  failure passed when isolated and is parallel-test interference, not a stable
  Plan PR0A failure.
  * The current run's two legacy workflow failures were
    `task_review_budget_override_wins_over_project_setting` and
    `human_unblock_resets_review_follow_up_budget_boundary`. The corrected
    Plan PR0 baseline audit also reproduced
    `auditor_failure_dispatches_follow_up_executor_with_thread_reuse`; it
    passed in this particular full run but remains a pre-existing, order-
    sensitive workflow failure. These three remain owned by Plan PR2/Plan PR9
    and are not expanded here.
  The full workspace command is therefore not claimed as wholly green; the
  focused Plan PR0A suites are green and no new stable Plan PR0A failure was
  identified.
* `pnpm lint`, `pnpm typecheck`, `pnpm test`, and `pnpm build` were attempted
  because frontend API types and the Agent usage panel changed. They were
  blocked before execution because `web/node_modules` is absent (`eslint`,
  `tsc`, and `vitest` were not found); no package installation was performed.

Next dependency: Plan PR1 — Actor + multi-actor TaskRole / RoleMembership.
