# PR 0A — Reconcile pre-migration operational fixes

Status: implementation branch for review; dependent on PR 0 (`docs/independent-orchestration-pr0`).

## Scope

This PR reconciles operational work that was present after the PR 0 audit:

* port lossless execution-log rotation and compression from `71f478b`;
* keep small Cursor prompts on direct argv transport and move only oversized
  prompts to private, randomized runtime control files with deterministic
  cleanup and a bounded stale-file TTL;
* preserve explicit executable and environment overrides without shell-alias
  discovery;
* keep Codex native rate-limit events and bounded, cancellable Cursor quota
  polling observable with execution/account provenance;
* make WorkspaceLease renewal independent of harmless Task revisions while
  retaining fail-closed authority checks;
* record the reconciliation decisions for the other local commits and legacy
  PR #2.

This PR does not introduce ActorRef, multi-actor roles, HarnessSession,
ExecutionPurpose, HarnessAdapter, WorkUnit, plan Artifacts, review Executions,
or a new workflow/orchestration model. Existing legacy planning, review,
workflow, and session paths remain only as pre-migration code owned by their
specified replacement PRs; this PR does not extend them.

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
  path rather than probing the server's account.
* Lease authority remains deterministic and continues to check execution,
  principal, assignment, repository, capability, lifecycle, governance, and
  revocation. The issued Task revision remains historical provenance, not a
  revocation signal for unrelated metadata edits. When an explicit legacy role
  assignment exists, it takes precedence over the task-level fallback.

## Reconciliation ledger

| Source | Decision | Owning follow-up or proof |
| --- | --- | --- |
| Legacy PR #2: Cursor large-prompt transport | Adapted. Prompts at or below 32 KiB remain direct argv input. Larger prompts are written to a private randomized directory under the system runtime directory, exposed through Cursor's `--add-dir` capability, and passed by a short instruction. Unix permissions are 0700/0600; RAII cleanup handles normal completion and launch failure, while a 24-hour stale-directory sweep handles hard-crash leftovers. Tests prove the worktree receives no control file or diff entry. | PR 3 will move this launch behavior behind `HarnessAdapter`; the current Cursor CLI smoke probe confirmed `--add-dir` and no documented prompt-file/stdin option. |
| Legacy PR #2: executable/environment overrides | Kept in the existing explicit `CommandOverrides` contract. A raw executable path, wrapper, and environment map are reproducible inputs. Interactive shell alias parsing was not ported. | PR 3 owns the adapter boundary. |
| Legacy PR #2: account usage/live quota | Adapted. Codex consumes native `account/rateLimits/updated`; Cursor uses bounded, cancellable PTY polling with the explicit `cursor_poll` source. Local and daemon execution share the server-side extraction/persistence boundary; daemon Cursor polling runs where the CLI/account exists. Each observation records execution, source, account key, and host/daemon provenance. | PR 3 will expose dimensional adapter capability/usage metadata. |
| Legacy PR #2: WorkspaceLease false invalidation | Adapted in V086 and the service verifier. Task revision is retained for audit, while harmless description/metadata revisions do not revoke authority. Explicit role assignment takes precedence over a stale task fallback; repository, capability, lifecycle, governance, and revocation changes still fail closed. | PR 5 will bind final lease authority to the new Actor/WorkUnit/workspace model. |
| Legacy PR #2: quota reassignment/session continuation | Not ported. Singular-role and inferred-session behavior would violate PR 1/2. The requirement is carried forward: a reassigned execution must use the new Actor and an explicitly selected compatible session, never the previous Actor's latest session. | PR 1 and PR 2; acceptance hardening in PR 15. |
| Legacy PR #2: optional plan-review state machine | Discarded. No `PLAN_REVIEW`, planner/reviewer loop, plan-review retry, or coupled UI was added. | PR 7 provides plan Executions, Artifacts, and optional Gates/Decisions. |
| Legacy PR #2: shell alias introspection | Discarded. Forge does not inspect shell startup files or infer account identity from aliases. | Explicit `CommandOverrides` is the supported contract. |
| `71f478b`: execution log rotation | Ported selectively, without its old documentation changes. Rotated gzip segments preserve one logical sequence, historical reads, bounded tails, and final compaction. The active base path is not itself a complete historical export; relocation is protected by process-local path locks and service execution ownership. | Log storage remains infrastructure; no domain cognition depends on it. |
| `509205a`: workflow resume | Deferred. Its implementation relies on workflow roles and inferred `agent_session_id` continuity. | PR 2 (explicit session) and PR 9 (aggregate lifecycle). |
| `3d291dd`: current-role re-execution | Deferred. Its implementation relies on one singular current role assignment. | PR 1 (multi-actor membership) and PR 2 (explicit Actor/session identity). |

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
* Cleanup: final lease/schema replacement belongs to PR 5 and destructive
  compatibility cleanup to PR 13.

### Account usage

* Old writers: adapter terminal results, daemon execution notifications, and
  agent refresh routes wrote `account_usage_snapshot`; execution snapshots
  sometimes omitted the host that owned the CLI credentials.
* New writers: the same paths write a host-aware account key. An explicit Agent
  daemon binding and the scheduler-resolved daemon are recorded separately;
  the resolved daemon is factual provenance and participates in quota identity
  when credentials are host-local. An explicit durable credential reference is
  the only transitional proof that the same account is shared across hosts.
  Native Codex events and Cursor polling use distinct sources. A server-side
  agent refresh does not probe a daemon-bound or unresolved remote CLI.
* Old readers: agent usage API and execution usage projections continue to
  read `account_usage_snapshot`.
* Authority: the execution snapshot plus `execution_id` and source provenance
  are the durable record. No dual-write source exists.
* Migration: `V087__cursor_poll_usage_source.sql` replaces the usage-source
  check constraint additively, preserving existing rows and adding the
  explicit `cursor_poll` provenance value. Historical migrations are not
  edited.
* Cleanup: PR 3 moves usage/capability semantics behind HarnessAdapter; PR 13
  removes any transitional fields only after readers migrate.

### Cursor control prompts

The prompt file is transient runtime state, not persistence. Small prompts stay
on the existing direct argv path. Oversized prompts are written to a private
per-prompt directory outside the worktree, with randomized names and Unix
0700/0600 permissions, and Cursor receives `--add-dir` for that directory plus
a short instruction. The file is removed when the adapter exits or fails to
launch; a bounded 24-hour stale-directory sweep recovers files left by a hard
process kill. The path is never included in execution Git state. The installed
Cursor CLI was checked for this exact capability; a full authenticated model
smoke run remains adapter validation owned by PR 3.

## Dependency audit

The affected readers and writers were re-searched on the PR 0A branch before
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
* PR #2 plan-review and alias code: reviewed but intentionally not copied.

## Validation and exit report

No legacy planning/review/workflow schema is dropped. No compatibility reader
is added, and no hidden second source of truth is introduced.

Validation performed on this branch:

* `cargo fmt --all -- --check` — passed after the final code changes.
* `FORGE_SKIP_WEB_BUILD=1 cargo clippy --workspace --all-targets -- -D warnings`
  — passed. The documented `FORGE_SKIP_WEB_BUILD=1` path was used because no
  frontend source changed.
* Focused tests passed: `cli-adapters` Cursor tests (11), `executors` config
  tests (18), `executors` log-writer tests (7), the DB lease renewal test,
  `services` execution tests (7), daemon usage provenance, `forge-client`
  daemon-runtime tests (6), and API execution-log tests (2).
* `FORGE_SKIP_WEB_BUILD=1 cargo test --workspace --all-targets
  --no-fail-fast` completed on the host filesystem after an initial `/tmp`
  target-directory attempt was blocked by filesystem quota. The final run
  used the required elevated local-socket permission and all crates and
  targets built. Four API targets reported failures in that broad run:
  `follow_up_review_budget_override`, `follow_up_review_fail`,
  `follow_up_unblock_resets_budgets`, and `happy_path`.
  * `task_review_budget_override_wins_over_project_setting` fails on the
    corrected PR 0-only baseline as well; it is legacy workflow/retry behavior
    owned by PR 2/9. PR 0A reaches a later session-continuity assertion in the
    same legacy test, but does not change its ownership or bring that deferred
    behavior into this PR.
  * `human_unblock_resets_review_follow_up_budget_boundary` fails on the
    corrected PR 0-only baseline as well; it is owned by PR 2/9.
  * `auditor_failure_dispatches_follow_up_executor_with_thread_reuse` and
    `autonomous_workflow_requires_human_review_and_resumes_worker` both passed
    when rerun individually on PR 0A immediately after the broad run. The
    latter also passed on PR 0; these are recorded as broad-suite
    interference, not stable PR 0A regressions.
  The full workspace command is therefore not claimed as wholly green; the
  touched PR 0A focused suites and the isolated reruns above are green.
* `pnpm lint`, `pnpm typecheck`, `pnpm test`, and `pnpm build` were attempted
  because the generated terminal-notification binding was refreshed. Each was
  blocked before execution because `web/node_modules` is absent (`eslint`,
  `tsc`, and `vitest` were not found); no network/package installation was
  performed. No frontend validation is claimed.

Next dependency: PR 0A must be reviewed and merged before PR 1 begins.
