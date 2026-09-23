# Plan PR3 Static Preflight

## Audit baseline and limits

The required source reader/writer audit was performed before source edits,
after syncing `origin/main` and creating
`feat/plan-pr3-harness-adapter-capabilities` from
`5e5d8fb29a02264c46577905a1bdfe87820f19e2`. The unrelated untracked
`.recovery-pr0a/` directory was present and preserved. The migration files at
this baseline end at V089. This is a source and Git audit only; it does not
prove compilation, tests, runtime behavior, CLI compatibility, or migration
execution.

## Current readers, writers, and classification

Classification key:

* **A — harness protocol translation**
* **B — harness capability detection**
* **C — generic deterministic authority**
* **D — generic execution/routing infrastructure**
* **E — legacy embedded Agent Runtime / PR10 debt**
* **F — public compatibility surface / PR12 debt**
* **G — historical compatibility / PR13 debt**

| Current path | Current reader or writer | Class and PR3 treatment |
| --- | --- | --- |
| `crates/cli-adapters/src/{codex,claude,cursor,opencode,gemini,smith,shell,null}.rs` | Eight `CodingExecutorAdapter` implementations own command construction, protocol parsing, session IDs, usage extraction, cancellation, and availability checks. | A/B: move these operations behind `HarnessAdapter`; retain generic normalized logs and results. |
| `crates/executors/src/adapter.rs` | `AdapterRegistry` selects the concrete adapter; `AdapterExecutor` and `FallbackExecutor` call `execute`; fallback records candidate identity, account cooldown, attempts, and winner config. | D plus a transitional adapter authority: registry becomes `HarnessAdapterRegistry`; fallback remains generic and carries the selected adapter's capabilities. |
| `crates/executors/src/config.rs` | Central `ExecutorKind` switch performs typed harness config normalization. Separate helpers compute stable candidate and account keys, strip session-scoped identity from the hash, and build ordered routes. | Normalization is A and moves to each adapter. Candidate/account identity, ordering, cooldown, and deduplication remain D; Codex `CODEX_HOME` and Smith provider/profile key behavior must stay exact. |
| `crates/executors/src/effective_policy.rs` | Central `ExecutorKind` switch interprets Codex sandbox, Claude skip-permissions, and Cursor force/propose-only; the same module checks workspace containment. | Harness interpretation is A and moves to adapters. Workspace containment stays C in core. |
| `crates/services/src/task_service/config.rs` | Merges Agent/profile and execution override layers, normalizes primary and fallback config centrally, writes `resume_thread_id`, `resume_session_id`, Codex in-place/fallback-prompt keys, and recursively removes those keys for fresh dispatch. It snapshots Agent `capabilities_json` as `"capabilities"`. | Layer precedence and route selection are D. Resume key writing/removal and normalization are A and move to adapters. The `"capabilities"` field remains the legacy Agent tag/profile projection, not the new authority. |
| `crates/services/src/task_service/execution/{follow_up,recovery,cascade}.rs` and `task_service.rs` | Select an explicit PR2 HarnessSession, promote the session-producing candidate, then call the provider-key snapshot helpers before execution. | Session identity/Actor/workspace checks are C. Candidate promotion remains D. Provider translation is A and moves to the adapter; runtime invocation is generic Start/Resume. |
| `crates/services/src/task_service/execution/runner.rs` | Builds a fixed Shell reviewer `FORGE_RESULT` command inside execution-description construction. | A: this Shell compatibility command moves into `ShellAdapter`; the generic Execution Role is carried in the adapter context/daemon request. No review workflow or verdict semantics change. |
| `crates/services/src/task_service/execution.rs` | `resumable_external_session` validates Actor, lifecycle, external ID, harness kind, workspace, and legacy ambiguity; cursor polling directly calls `cli_adapters::cursor`. | Session authority remains C. Poll transport is A and moves behind the same adapter used for execution. Capability support must additionally gate resumability. |
| `crates/services/src/account_usage.rs` | Service helpers directly call Codex and Cursor usage probes. | A: callers move to `HarnessAdapter.observe_usage`; preserve the existing Cursor `cursor_poll` accounting source. |
| `crates/db/src/sqlite/{execution,harness_session}.rs` | Pending/result-time session materialization reads snapshot `"capabilities"` into `capabilities_snapshot_json`. | Snapshot persistence remains D/G. New rows prefer `harness_capabilities`; legacy snapshot content is retained only as bounded historical compatibility, with PR13 cleanup ownership. |
| `crates/api/src/routes/executor_types.rs` | Discovery obtains options from `AppState.adapter_registry`; no parallel API adapter table exists, but response omits dimensional capabilities. | F: keep this same registry source and add typed `harness_capabilities` additively. |
| `crates/services/src/daemon_transport/*`, `crates/forge-client/src/daemon_runtime.rs`, `crates/api-types/src/daemon_transport.rs` | Server sends config and prompt; daemon executes through `FallbackExecutor`; terminal response returns actual winner kind/config and route attempts, but neither side carries generic invocation or winner capabilities. | D: retain the transport and provenance; add generic invocation and resolved capability snapshot, and keep local/remote execution on the same adapter operations. |
| `crates/services/src/operator_status.rs` | Repeats provider-specific effective-policy interpretation from snapshot config instead of using the adapter's normalized result. | A: capture adapter policy before dispatch and replace it with the route winner's result; retain the old reader only for historical snapshots and Embedded compatibility. |
| `crates/services/src/embedded_daemon.rs`, `crates/forge-client/src/daemon.rs`, `crates/forge-daemon/src/detect.rs` | Build local CLI detection/version rows with `ExecutorKind` switches and binary names. | B/D: availability moves to `HarnessAdapter.detect`; generic daemon path/version inventory remains process infrastructure where required. |
| `crates/services/src/embedded_task_executor.rs`, `TaskExecutorRouter`, `ExecutorKind::Embedded`, Agent Host paths | Embedded runtime handles Forge-owned cognition outside the CLI registry. | E: bounded legacy exception only; do not duplicate it as a permanent harness adapter. Cleanup owner is PR10. |
| `crates/api-types`, REST/MCP, generated web types, and `ExecutorKind` serialization | Existing `executor_type`, `ExecutorKind`, and Agent capability/tag fields are public or persisted names. | F/G: preserve names and legacy semantics in PR3; public naming cleanup is PR12 and persistence cleanup is PR13. |

`TaskExecutor` is consumed by TaskService, `FallbackExecutor`, embedded routing,
daemon execution, recovery, shutdown, and review/runtime tests. It remains the
supervisor-facing transitional facade for routing, fallback, cancellation, and
generic result handling; it is not the new harness protocol interface.

## Capability evidence matrix at the audited baseline

Values describe the Forge integration in this repository, not upstream product
claims. `Native`, `Emulated`, `Unsupported`, and `Unknown` are distinct; only
Native and Emulated are available. The dimension order is:

`resume; cancel; structured_events; usage_reporting; account_usage_observation; model_selection; reasoning_controls; approval_policy; sandbox_controls; planning; review_mode; fork; steer; pause_resume; compaction; subagents`.

| Adapter | Dimensional support in the order above | Source evidence |
| --- | --- | --- |
| `codex` | Native; Emulated; Native; Native; Native; Native; Native; Native; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | `codex.rs` exposes app-server `thread/resume`; `ThreadStartParams` carries model, reasoning, approval, and sandbox; the client normalizes protocol events and usage. `query_account_usage` requests account rate limits over JSON-RPC. Public adapter cancellation signals/kills the child process, so it is Emulated. An internal `thread/fork` helper is not reachable through the generic adapter invocation and does not establish a supported `fork` capability. `PermissionPolicy::Plan` only maps to read-only sandbox/approval settings. |
| `claude_code` | Native; Emulated; Native; Native; Unsupported; Native; Native; Native; Unsupported; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | `claude.rs` emits `--resume`, `--output-format=stream-json`, `--model`, `--effort`, and `--permission-mode plan` for the explicit Claude `plan` config; normalized result events carry token usage. The native plan flag is not yet connected to `ExecutionPurpose::Plan`; the legacy `PermissionPolicy::Plan` mapping is not itself capability evidence. Cancellation signals the child process; missing exact sessions fail before launch. |
| `cursor` | Native; Emulated; Native; Native; Emulated; Native; Unsupported; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unknown; Unsupported; Unsupported; Unsupported | `cursor.rs` emits `--resume`, `--output-format stream-json`, `--model`, and `--force`; event parser extracts token usage and session IDs. `/usage` is a Forge-driven PTY command and text parser. Cancellation kills the child. No Forge steering implementation is present, so steering remains Unknown and fails closed. |
| `opencode` | Native; Emulated; Native; Unsupported; Unsupported; Native; Unsupported; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | `opencode.rs` uses `--session`, `--format json`, and `--model`; its JSON event parser extracts session IDs and text but no usage. `--dangerously-skip-permissions` is the implemented native permission flag. Cancellation kills the child. |
| `gemini` | Unsupported; Emulated; Unknown; Unsupported; Unsupported; Native; Unsupported; Native; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | `gemini.rs` emits `--output-format=json`, `--model`, `--yolo`, and `--sandbox`; it parses JSON-shaped lines but does not establish a structured event-stream contract, so structured events stay Unknown. It returns no session ID or usage. Cancellation kills the process. |
| `smith` | Native; Emulated; Native; Native; Unsupported; Native; Native; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | `smith.rs` emits `--resume`, `--output-format stream-json`, `--model`, `--effort`, and native approval flags; its structured runtime/result parser extracts session IDs and per-turn usage. Cancellation kills the child. |
| `shell` | Unsupported; Emulated; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | `shell.rs` delegates to `ShellExecutor`; cancellation terminates its process. It exposes no session protocol, structured harness event stream, model/reasoning/approval/planning/review interface, or usage probe. |
| `null` | Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | `null.rs` only delays and returns a generic success; cancellation is a no-op and no harness capability or session is implemented. |

The existing Agent/Profile `capabilities_json` is read as legacy authored tags,
filters, or embedded Agent capability data. It is not evidence for any matrix
entry and is not the effective HarnessCapabilities authority.

The table above was captured before refactoring and checked against the
migrated adapter methods during the final audit. In
particular, Codex, Claude Code, Cursor, OpenCode, Smith, Shell, and Gemini
cancel by Forge-managed process cancellation and are therefore `Emulated`;
Null cancellation is a no-op and remains `Unsupported`. Claude's explicit
`ClaudeCodeConfig.plan` path emits its native `--permission-mode plan` flag,
so that adapter reports planning support. The generic `ExecutionPurpose::Plan`
is not connected to the flag yet, and a `PermissionPolicy::Plan` value alone
does not prove native planning. PR7 consumes Purpose independently.

## Post-implementation static reader/writer result

The production `CodingExecutorAdapter` trait and `AdapterRegistry` authority
are removed. External protocol/config/session/usage operations use the
registered `HarnessAdapter`. `TaskService` receives the same registry instance
as local execution and API discovery; standalone service constructors use the
built-in registry. `TaskExecutor` remains the generic routing/supervisor
facade. No database migration was added; the migration head remains V089.

Service code now derives a generic Start/Resume invocation from the explicit
HarnessSession link. Provider-native resume strings remain only in adapter
implementations, adapter tests, and bounded legacy snapshot assertions. The
legacy Agent `"capabilities"` field remains untouched. New Execution snapshots
start with the effective primary candidate's typed
`"harness_capabilities"`; result-time routing replaces it with the actual
winner's evidence. Pending HarnessSession snapshots are updated to that winner
when the external session is bound. The legacy JSON fallback reader is limited
to exact PR2 shapes and known resumable adapter kinds; PR13 owns its removal.
When an explicit Resume promotes a different route candidate, the service
refreshes the top-level config, capabilities, and effective policy from that
exact candidate's adapter before dispatch. It does not carry the previous
primary candidate's evidence into the Resume Execution.

New external Execution snapshots also capture the primary candidate's
adapter-interpreted `effective_execution_policy` before dispatch, so active
single-candidate executions do not use the historical service switch. The
route result replaces it with the actual winner's policy. Only old snapshots
and the bounded Embedded exception use the historical operator-status
interpreter; PR13/PR10 own those readers.

Codex and Cursor account observations, including Cursor's periodic
`cursor_poll`, now call the generic TaskExecutor usage operation, which
normalizes and dispatches through the selected HarnessAdapter. `effective_policy`
no longer interprets harness configs in a central kind switch: adapters report
generic permission/isolation posture, while Forge retains high-risk
classification and workspace containment. Historical snapshots and Embedded
retain bounded PR13/PR10 compatibility readers.

The only remaining service-level `ExecutorKind` special cases are generic
Embedded routing/config handling (PR10), Shell process/host compatibility, and
the exact legacy PR2 resume allowlist (PR13). Codex `CODEX_HOME` and Smith
provider/profile account-key logic remains in generic routing identity code so
PR0A candidate/account/cooldown behavior is preserved; it does not select
harness protocol or capability behavior. Shell's reviewer command compatibility
is now translated by `ShellAdapter`; periodic quota polling remains Cursor-only
to preserve PR0A `cursor_poll` semantics.

## Required PR3 reader/writer changes

1. Replace the production `CodingExecutorAdapter` authority and registry with
   `HarnessAdapter` and `HarnessAdapterRegistry`; retain `TaskExecutor` only as
   the generic supervisor facade.
2. Normalize each concrete candidate through its registered adapter after the
   existing Agent/profile -> execution override merge. Use that same adapter
   for detect, capabilities, start/resume, cancel, and usage observation.
3. Replace provider-specific service config mutation with the exact generic
   HarnessSession external ID in a runtime Start/Resume invocation. Start must
   sanitize each adapter's stale session-scoped config internally.
4. Carry each candidate's normalized HarnessCapabilities with the route winner
   in local results, remote terminal notifications, Execution snapshots, and
   new HarnessSession snapshots. Preserve the legacy `"capabilities"` field.
5. Filter resumability using historical HarnessSession capability evidence;
   interpret unrecognized historical JSON as Unknown. A narrow compatibility
   resolver may trust exact active PR2 sessions only for the known adapters
   whose PR2 integration explicitly implements resume; name that reader for
   PR13 removal.
6. Keep Codex `CODEX_HOME`, Smith provider/profile account keys, opaque
   credential references, daemon-local account identity, cooldowns, fallback
   order, and route attempts unchanged; reject candidates whose harness or
   explicit identity-bearing account key changes the Agent identity.
7. Add typed discovery output from the same adapter registry. Provider
   credential/runtime capability types remain a separate domain.

## Schema and scope preflight

The highest existing migration is V089. The PR2 Execution and HarnessSession
JSON snapshot columns can carry the new typed capability object, so the
preflight identifies no schema need and plans no migration. Execution Purpose
remains separate from permission. No plan Artifact, review workflow, WorkUnit,
orchestration workflow, Agent Host removal, public executor rename, or
historical-row rewrite is in PR3 scope.

## Supplemental post-implementation audit findings

The baseline route accepted cross-harness fallbacks and same-harness account
switches, and PR2 could materialize a historical HarnessSession for the actual
winner under the primary Agent. PR3 now rejects those route snapshots before
local or remote dispatch. Historical rows remain immutable; the resume reader
compares Actor, Agent harness, session snapshot harness/account/credential,
Execution snapshot identity, and the current Agent. Contradictory PR2 history
is kept for audit but is not advertised or resumed as the current Agent's
continuity. Same-account changes to model, effort, sandbox, or permission
remain eligible as profile/run configuration.

The durable capability parser distinguishes versioned PR3 v1 snapshots,
unversioned PR2 data, malformed input, and unsupported future versions. V1
snapshots allow missing dimensions to resolve as Unknown and ignore unknown
dimension names; arbitrary partial unversioned objects never become support.
Only the exact PR2 string-tag array or empty-object shapes reach the bounded
known-harness compatibility allowlist. An initial static follow-up found that
the parser classified legacy arrays as malformed before that resolver; it now
classifies arrays as unversioned so the caller can apply the strict shape and
harness checks. Non-string arrays fail the compatibility shape check.

Every PR3 remote execution negotiates `daemon.protocol_capabilities` before
`execution.start`; no response, an old daemon's unsupported-method error, an
unknown schema, or a missing `generic_harness_invocation_v1` feature rejects
dispatch. This also prevents an older adapter from interpreting stale
provider resume keys in a fresh Start snapshot. Reviewer Start additionally
requires `execution_role_v1` because the Shell reviewer compatibility command
depends on that role context. An older server's Start payload still defaults
to Start on a PR3 daemon. A legacy remote route winner without capabilities
gets all-Unknown evidence; its policy is recomputed using its exact kind/config
or removed, never copied from the primary candidate.

The post-implementation searches found no production `CodingExecutorAdapter`
authority, no provider-specific resume mutation outside adapters, and no
direct concrete Codex/Cursor usage probe outside the adapter registry. The
standalone effective-policy workspace helper has test callers only; actual
execution workspace authority remains the service's lease/authority checks,
not that helper. `is_high_risk` is a core-derived policy classification and
does not itself grant or revoke a lease.

A final provenance audit found that provider credentials were being injected
into the normalized candidate config before fallback, which could affect the
candidate key and leak into winner snapshots. Injection now uses a runtime-only
`runtime_env` field on the in-memory dispatch snapshot; the executor overlays
it after normalization for adapter detection/invocation, while candidate
identity and all persisted winner evidence use the clean config. The focused
fallback and credential-injection tests cover this split. The DB admission
fixture was also narrowed from a cross-harness example to a same-Agent,
same-account model variation. Claude's existing explicit `plan` config flag
was verified against the emitted native CLI mode and is exposed as planning
support; the generic Purpose and legacy PermissionPolicy mapping remain
separate.

Verification status: workspace `cargo check` passed before final test-only
fixes. The TypeScript export test passed. Full workspace tests were attempted,
but after fixing observed test compile errors, the final run was stopped when
the filesystem reached 0 bytes free. `cargo clean` removed 33.0 GiB of
generated artifacts and restored 31 GiB free. No tests completed after the
final test-source edits; this supplemental audit does not claim behavioral
verification.
