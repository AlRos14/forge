# Plan PR3 — Capability-Driven HarnessAdapter Exit Ledger

## Boundary and result

Plan PR3 replaces the external harness protocol authority with the typed
`HarnessAdapter` boundary while retaining the ordered migration sequence.
`ExecutorKind` and the public `executor_type` vocabulary remain compatibility
names. This change does not implement planning artifacts, review workflow
migration, orchestration, embedded-runtime removal, or final public renames.

The implementation started from `5e5d8fb29a02264c46577905a1bdfe87820f19e2`
and the static preflight in [`plan-pr3-preflight.md`](plan-pr3-preflight.md).
The follow-up audit tightened Agent-bound routing, capability snapshot
evolution, daemon negotiation, remote winner provenance, and historical
session identity checks. Verification results below describe the commands
that actually completed; the final workspace test run was stopped after the
filesystem reached 0 bytes free.

## Old and new adapter authority

| Concern | Before PR3 | After PR3 |
| --- | --- | --- |
| External protocol and parsing | `CodingExecutorAdapter` implementations | The matching registered `HarnessAdapter` owns normalization, detection, capabilities, start/resume translation, usage observation, event/result normalization, and cancellation mechanism. |
| Registry | `AdapterRegistry<CodingExecutorAdapter>` | `HarnessAdapterRegistry`; the CLI default registry contains the eight external HarnessAdapters. |
| Supervisor and route policy | `TaskExecutor`, `AdapterExecutor`, `FallbackExecutor` | `TaskExecutor` remains a transitional generic facade for supervision, ordered Start fallback, cancellation, normalized logging/results, availability disposition, and winner provenance. It does not own harness cognition/protocol behavior. |
| Deterministic authority | Effective-policy/workspace and session checks in core | Remains in Forge core. Adapters report generic policy posture; core retains workspace containment, risk classification, leases, and authorization. |
| API discovery | `AppState.adapter_registry` without dimensional support | Same registry and adapter methods used by execution; additive `harness_capabilities` is returned with discovery. |

There are zero production `CodingExecutorAdapter` references in the audited
tree. No old trait alias or parallel registry remains. `ExecutorKind` remains
the current stable identifier and is not mass-renamed.

## Reader/writer classification

The preflight classified all relevant reads and writes as follows:

* **A — harness protocol translation:** concrete command/config normalization,
  native session protocol, output/usage parsing, availability detection, and
  harness-specific policy interpretation now belong to the adapter.
* **B — capability detection:** each candidate's typed support is computed by
  its own registered adapter from the actual normalized candidate config.
* **C — generic deterministic authority:** workspace containment, session
  identity/lifecycle checks, high-risk decisions, and execution purpose remain
  in core/services.
* **D — generic execution/routing infrastructure:** config-layer precedence,
  ordered candidate routing, fallback classification, candidate/account keys,
  cooldowns, cancellation propagation, and event delivery remain generic.
* **E — legacy embedded Agent Runtime:** `ExecutorKind::Embedded` and Agent
  Host execution stay on the existing compatibility path; this is bounded
  PR10 debt, not a second external HarnessAdapter implementation.
* **F — public compatibility:** `ExecutorKind`, `executor_type`, existing
  endpoint names, and generated API field naming remain in place; PR12 owns
  final public naming cleanup.
* **G — historical compatibility:** prior snapshot JSON and the exact PR2
  resumability fallback remain read-only bounded compatibility; PR13 owns
  their removal. Existing history is not rewritten.

The detailed baseline reader/writer paths, including service config,
TaskService, daemon transport, account usage, operator status, the Shell
reviewer-command compatibility path, and API discovery, are recorded in the
preflight document.

ReviewRunner changes are limited to invoking the registered HarnessAdapter,
normalizing its concrete config, and persisting the actual reviewer Execution
candidate's capability/policy snapshot. Review verdict, retry, and lifecycle
behavior remain unchanged because PR3 snapshots apply to every Agent Execution.

## Capability model and evidence

`CapabilitySupport` has four distinct values:

| Value | Available | Native |
| --- | --- | --- |
| `Native` | yes | yes |
| `Emulated` | yes | no |
| `Unsupported` | no | no |
| `Unknown` | no | no |

`Unknown` fails closed. Callers use shared `is_available()` and `is_native()`
semantics rather than treating unknown evidence optimistically. Operations
requiring native support must check `is_native()`. Unsupported optional
operations return an explicit `UnsupportedCapability` error with the support
level; they do not turn into prompts, fresh starts, or silent success.

The typed dimensions are `resume`, `cancel`, `structured_events`,
`usage_reporting`, `account_usage_observation`, `model_selection`,
`reasoning_controls`, `approval_policy`, `sandbox_controls`, `planning`,
`review_mode`, `fork`, `steer`, `pause_resume`, `compaction`, and `subagents`.
`usage_reporting` describes per-execution usage; account/quota observation is a
separate dimension. Capability metadata contains support facts only, not
credential material or sensitive environment values.

The matrix describes the current Forge integration, not upstream product
features. Evidence is in each adapter's command/protocol/config implementation
and is detailed in the preflight. Semicolon-separated entries follow the
dimension order above:

| Adapter | Support by dimension | Evidence summary |
| --- | --- | --- |
| Codex | Native; Emulated; Native; Native; Native; Native; Native; Native; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | App-server thread resume; normalized JSON-RPC events and usage; account rate-limit RPC; explicit model/reasoning/approval/sandbox params. The internal `thread/fork` helper is not reachable through the generic adapter boundary, so `fork` is Unsupported. `PermissionPolicy::Plan` maps restrictions only. Process cancellation is Forge-emulated. |
| Claude Code | Native; Emulated; Native; Native; Unsupported; Native; Native; Native; Unsupported; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | Explicit `--resume`, stream-json event parsing, model/effort/permission flags, usage normalization; the separate `ClaudeCodeConfig.plan` option emits native `--permission-mode plan`. It is not yet connected to `ExecutionPurpose::Plan`; the legacy `PermissionPolicy::Plan` mapping alone is not capability evidence. Process cancellation is Forge-emulated. |
| Cursor | Native; Emulated; Native; Native; Emulated; Native; Unsupported; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unknown; Unsupported; Unsupported; Unsupported | Explicit `--resume`, stream-json parsing and token usage, model/force flags; Forge PTY `/usage` observation and text parsing; no proven steering protocol; process cancellation is Forge-emulated. |
| OpenCode | Native; Emulated; Native; Unsupported; Unsupported; Native; Unsupported; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | `--session`, JSON event parsing, model and permission flags; no usage extraction; process cancellation is Forge-emulated. |
| Gemini | Unsupported; Emulated; Unknown; Unsupported; Unsupported; Native; Unsupported; Native; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | JSON-shaped line parsing is implemented but no structured-event contract is established; model, `--yolo`, and sandbox flags exist; no session ID or usage; process cancellation is Forge-emulated. |
| Smith | Native; Emulated; Native; Native; Unsupported; Native; Native; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | Explicit resume, stream-json parsing and per-turn usage, model/effort/approval flags; process cancellation is Forge-emulated. |
| Shell | Unsupported; Emulated; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | ShellExecutor process termination; no harness session, model, cognition, structured event, or usage protocol. Reviewer compatibility command is translated inside ShellAdapter. |
| Null | Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | Delay/no-op test adapter has no session or cognition protocol; cancellation is a no-op. |

Shell and Null expose no cognitive capabilities. Claude's explicit
`ClaudeCodeConfig.plan` option invokes its native `--permission-mode plan`
flag, so its planning capability is Native. The generic
`ExecutionPurpose::Plan` is not connected to that option yet, and
`PermissionPolicy::Plan`, Codex read-only settings, propose-only behavior, and
prompt conventions do not independently prove native planning.
`ExecutionPurpose::Plan` remains independent from permission and is not turned
into a planning workflow. Provider credential/runtime capability types remain
separate from HarnessCapabilities.

## Configuration, policy, and capability snapshots

Config precedence remains:

```text
Agent/profile config
→ execution overrides
→ selected HarnessAdapter normalization
→ immutable Execution snapshot
```

The generic layer owns merge precedence and candidate selection. Each adapter
normalizes its own concrete harness config. Capabilities and effective policy
are computed from that same normalized candidate using that same adapter.
Primary and fallback candidates are inspected independently; same-kind
profiles do not share capability evidence by assumption.

The legacy Agent/Profile `capabilities_json` and snapshot JSON `"capabilities"`
retain their prior authored tag/filter/embedded compatibility meaning. They
are not HarnessCapabilities and are never overwritten with the typed model.
New Execution snapshots add `"harness_capabilities"` for the effective
candidate. They also capture the primary adapter's
`effective_execution_policy` before dispatch. A route result replaces the
initial primary entries with the actual winner's config, effective policy,
and capabilities.

Credential values resolved from an Agent's `credential_ref` travel only in a
runtime-only `runtime_env` attached to the in-memory dispatch snapshot. The
executor applies it after candidate normalization to the actual invocation
config. Candidate keys, resolved candidate config, capability/policy evidence,
Execution snapshots, HarnessSession snapshots, and route attempts never
contain those injected values. Same-Agent fallback candidates receive the
same runtime environment only after their harness/account identity is
validated.

New HarnessSession `capabilities_snapshot_json` values come from
`harness_capabilities`. Pending-session materialization prefers that field;
result-time binding updates the pending session with the actual winner's
snapshot. Existing active session snapshots are not recomputed when an Agent
profile changes. Historical snapshots are not rewritten. Strict typed
snapshots are parsed as PR3 evidence; unknown/malformed content is Unknown.
The narrow PR2 compatibility resolver only recognizes its exact historical
snapshot shapes and explicit known resumable harness kinds. PR13 owns removing
that resolver.

Durable snapshots use `{ "schema_version": 1, "capabilities": { ... } }`;
this versioned JSON contract is separate from the current Rust struct. Readers
preserve known v1 dimensions when future dimensions are present, resolve a
missing dimension or unknown support label to Unknown, and reject partial
unversioned objects, malformed versions, and unsupported schema versions as
resume evidence. Existing rows are not rewritten. The exact PR2 compatibility
resolver is the only unversioned exception and remains owned by PR13.

No new capability table or migration was added. Existing V089 Execution and
HarnessSession JSON snapshot fields represent this authority. Migration head
before and after is V089.

## Start, Resume, routing, and winner provenance

Runtime intent is `HarnessInvocation::Start` or
`HarnessInvocation::Resume { external_session_id }`. The persisted continuity
authority remains `Execution.harness_session_id → HarnessSession.external_session_id`.
Services validate Actor/session/workspace identity and pass only the generic
intent. They do not author provider-native resume keys. Each adapter translates
the exact external ID using its own native protocol; unsupported or unknown
resume fails explicitly. Fresh Start removes or ignores stale session-scoped
fields inside the adapter.

Start retains ordered fallback only within one Agent identity. Every route is
revalidated against the Agent's harness kind and the PR0A identity-bearing
account key before dispatch, including old persisted route snapshots. A
Codex-to-Cursor route and an account-key or credential identity change are
rejected; selecting another harness or native account requires explicitly
selecting/reassigning another Agent. Codex `CODEX_HOME` and Smith
provider/profile identity follow the existing account-key rules. Same-harness,
same-account variations in model, reasoning effort, sandbox, approval, and
other non-identity settings remain valid. Each accepted route candidate carries
its own normalized config,
`HarnessCapabilities`, effective policy, candidate/account identity, and
adapter. The actual winner is attached to the local result and remote terminal
result, and its evidence flows into the Execution and HarnessSession. No
candidate-specific state from the primary survives a winner change.

Resume promotes and pins the exact session-producing candidate. If that exact
candidate is unavailable, another candidate is not started as a substitute.
Promotion recomputes the top-level candidate's capabilities and effective
policy through that exact candidate's HarnessAdapter, so the Resume Execution
does not retain the former primary candidate's evidence.
Resume support must be available in the historical HarnessSession capability
evidence and is rechecked by the current adapter at invocation time. Unknown
or unsupported historical evidence fails closed except for the narrow PR2
compatibility case above.

The same generic invocation and resolved candidate/capability data cross
`ExecutionStartParams` to the daemon. Every remote execution negotiates
`generic_harness_invocation_v1` through `daemon.protocol_capabilities` before
`execution.start`; an old daemon without that capability is rejected. This is
required for Start too: an older adapter could read stale provider resume keys
from a historical snapshot and turn generic Start into a continuation.
Reviewer Start additionally requires `execution_role_v1`, because the Shell
reviewer compatibility command depends on role context. An older server's
Start or Resume payload lacks the required `invocation` field and is rejected
by a PR3 daemon during parameter decoding, before execution dispatch. The
daemon does not infer invocation from provider config keys. Together with new
servers rejecting old daemons before dispatch, this requires a PR3 server and
daemon pair for remote execution during rolling upgrades. Pre-PR3 omission of
winner capabilities resolves to an all-Unknown versioned snapshot, never
primary capability evidence. If a legacy winner omits effective policy, the
server recomputes it through the registered adapter using the exact returned
winner kind/config; if that cannot be done, the stale primary policy is
removed. Candidate-key/config mismatch or an unlisted/identity-changing
winner is rejected. Cancel's request shape is unchanged.

Local and daemon execution converge on
`HarnessAdapter.start` / `HarnessAdapter.resume`; transport does not mutate
provider-specific config. The Shell reviewer compatibility role is carried as
generic role context so ShellAdapter owns its command translation. This does
not alter review workflow or verdict behavior.

## Usage, events, and cancellation

Harness-specific per-execution event and usage parsing remains within each
adapter and produces generic Forge `LogEntry`, `ExecutionResult`,
`TokenUsage`, or normalized account observations. Account/quota observation
uses the generic adapter operation. Cursor's periodic observation remains
Cursor-specific scheduling with its existing `cursor_poll` accounting source,
but the probe call dispatches through the same HarnessAdapter registry. Codex
and Cursor production service callers no longer call concrete CLI modules.

Cancellation remains safely broadcast by the transitional FallbackExecutor
to registered adapters because active-winner tracking was not part of this
bounded change. External process termination is reported as Emulated; Null's
no-op is Unsupported. Forge retains deterministic cancellation authority.

## Effective policy boundary

Adapters interpret concrete harness config into generic permission and
isolation posture only; the adapter cannot set risk classification or
workspace authority. Forge core classifies high-risk posture and retains the
existing workspace/lease and authorization checks at dispatch. The generic
policy's `is_high_risk` field is descriptive evidence and does not itself
grant or revoke a lease. The standalone `validate_workspace_policy` helper is
not an authorization path; production authority is checked by
`verify_execution_workspace_authority` and the active WorkspaceLease checks.
Codex `danger-full-access` remains high risk; Claude skip-permissions remains
high risk; Cursor force/propose behavior maps to the generic policy. Adapter
interpretation does not grant authority or authorize risky work. New
snapshots capture primary-adapter policy before dispatch; the selected route
result replaces it with the actual winner's policy. The old service-side
harness-kind interpreter remains only for historical snapshots (PR13) and
the bounded Embedded exception (PR10).

## Embedded exception and transitional facade

`ExecutorKind::Embedded`, embedded task execution, Agent Host, and its
Forge-owned cognition remain the existing bounded legacy exception. PR3 does
not add a permanent full EmbeddedHarnessAdapter or expand that runtime.
Cleanup owner: PR10.

`TaskExecutor` remains the supervisor-facing generic facade for TaskService,
FallbackExecutor, daemon execution, embedded routing, and tests. It owns
generic routing/fallback/cancellation coordination only. `CodingExecutorAdapter`
and its registry are removed as production authority.

## Compatibility shims and cleanup owners

| Shim | Why it remains and exact accepted evidence | Why it grants no new authority | Cleanup owner |
| --- | --- | --- | --- |
| PR2 HarnessSession capability snapshots | Only unversioned arrays containing strings or the exact empty object shape are recognized, and only for the explicit PR2 resume-capable harness allowlist. | These values are legacy Agent tags, not dimensional support; all other unversioned or malformed content is Unknown and fails closed. | PR13 |
| Pre-PR2 `Execution.agent_session_id` reader | Requires an exact persisted known `executor_type`, matching Actor/Agent, harness, account key, credential reference, and a non-ambiguous external ID. | It reads historical continuity only; it does not mutate old rows or infer support from product reputation. | PR13 |
| Embedded execution/policy path | Existing Agent Host and embedded operator policy readers remain isolated from external HarnessAdapters. | Embedded capabilities are not represented as external harness evidence. | PR10 |
| Historical effective-policy interpretation | Existing snapshots without adapter-interpreted policy may use the old snapshot reader; the Embedded path is separately bounded. | It cannot overwrite current candidate policy; a remote winner's policy is recomputed from that winner or removed. | PR13 / PR10 |
| Shell reviewer command | The exact generic `role == reviewer` marker selects the existing fixed command inside ShellAdapter. | It is a Shell compatibility command, not native review-mode capability or verdict authority. | PR8 |
| Daemon protocol negotiation | `generic_harness_invocation_v1` is required before every remote Start/Resume; reviewer Start also requires `execution_role_v1`; daemon Start params require `invocation`. | New servers reject old daemons before dispatch; PR3 daemons reject old-server payloads missing `invocation` before dispatch. There is no provider-specific dual-write or inference fallback. | Retained transport contract; naming cleanup is PR12 |

## Public surface and schema

Executor discovery adds typed `harness_capabilities` from the execution
registry. Existing executor endpoint/type vocabulary and provider-auth
capability endpoints are unchanged. Rust shared types, checked-in generated
TypeScript bindings, and the web API type were updated. The api-types
TypeScript export test completed once and emitted the checked-in bindings; the
`u64` schema version is annotated/exported as the JSON `number` it is. No
frontend build, test, or typecheck ran, so frontend integration still
**REQUIRES IMPLEMENTATION-TIME VERIFICATION**.

No migration was added, no persisted history was rewritten, and migration head
remains V089. The change is additive in capability snapshot JSON and API
discovery. Reverting the code preserves database rows; however, an older binary
does not understand the generic invocation or new capability member, so
resuming sessions created through PR3 after software rollback may require
forward-fix/recovery with the PR3 binary. No rollback or migration execution
was performed.

## Tests added and verification status

Focused test additions cover:

* Support-level distinctions, Unknown/Unsupported fail-closed behavior, and
  strict typed snapshot parsing.
* All eight adapter capability matrices, Shell/Null cognition limits,
  PermissionPolicy::Plan separation, and generic effective-policy posture.
* Generic exact Resume translation in Codex, Claude Code, Cursor, Smith, and
  OpenCode, plus Start sanitation of stale session configuration.
* Shell reviewer command translation inside the adapter.
* Generic adapter invocation, unsupported Resume, no Resume fallback,
  rejection of identity-changing routes, same-agent winner capability
  propagation, and usage
  observation dispatch.
* Historical PR2 capability parsing, legacy Agent tags versus new typed
  snapshots, actual-winner Execution/HarnessSession snapshots, and preserving
  active historical snapshots after profile changes.
* Historical session identity checks that allow same-account model/effort
  changes but reject harness, account, credential, missing, or malformed
  historical identity evidence.
* Daemon invocation serialization, rejection of legacy payloads missing
  invocation, remote Resume dispatch,
  role protocol negotiation, safe Start delivery to an old decoder, and remote
  winner capability results.
* Review adapter winner capability snapshot and retained legacy Agent tags.

Named focused test additions include:

* `support_levels_remain_distinct_and_unknown_fails_closed`
* `typed_snapshot_requires_all_dimensions_and_rejects_legacy_extra_fields`
* `registered_adapters_expose_only_integration_evidence`
* `explicit_claude_plan_mode_is_exposed_as_native_planning_support`
* `adapter_policy_interpretation_preserves_core_high_risk_classification`
* `core_classifies_adapter_reported_high_risk_posture`
* `permission_plan_does_not_create_native_planning_or_risk`
* `generic_resume_uses_exact_thread_and_start_clears_stale_thread_fields`
* `resume_fails_when_exact_source_thread_is_missing`
* `generic_resume_uses_exact_session_and_start_clears_stale_session`
  (Claude Code, Cursor, OpenCode, and Smith adapters)
* `missing_session_keeps_exact_resume_request`
* `reviewer_compatibility_command_is_owned_by_shell_adapter`
* `executor_snapshot_for_harness_resume_keeps_resume_intent_generic`
* `fresh_start_removes_dispatch_marker_but_adapter_owns_legacy_resume_keys`
* `resume_intent_reaches_adapter_and_does_not_advance_to_fallback`
* `resume_does_not_use_an_available_alternate_candidate`
* `sticky_resume_fails_when_exact_candidate_left_route`
* `fallback_cannot_switch_harness_identity`
* `sticky_resume_rejects_cross_harness_candidate`
* `same_agent_start_fallback_records_winning_model_capabilities`
  (also verifies runtime credential env reaches the winner without entering
  candidate provenance)
* `provider_env_injection_is_in_memory_only_and_refuses_oauth`
* `resolved_same_agent_ordered_fallback_admits_the_resolved_snapshot`
* `unknown_resume_is_an_explicit_unsupported_capability`
* `generic_usage_observation_routes_through_registered_adapter`
* `historical_resume_support_is_typed_or_narrow_pr2_compatibility`
* `historical_session_identity_allows_run_config_but_rejects_harness_account_and_credential_changes`
* `initial_harness_policy_comes_from_the_registered_adapter`
* `profile_and_capability_changes_do_not_rewrite_an_active_session_snapshot`
* `generic_start_and_resume_survive_daemon_transport_serialization`
* `pre_pr3_execution_start_without_invocation_is_rejected`
* `pre_pr3_resume_request_is_rejected_before_adapter_dispatch`
* `new_server_start_rejects_pre_pr3_daemon_before_dispatch`
* `remote_reviewer_start_rejects_old_daemon_that_cannot_preserve_role`
* `remote_resolved_candidate_carries_effective_harness_capabilities`
* `remote_resume_intent_reaches_harness_adapter_and_reports_capabilities`
* `resume_result_must_confirm_exact_external_session`
* `missing_claude_session_fails_before_starting_a_new_session`
* `auditor_snapshot_records_resolved_adapter_capabilities_without_overwriting_agent_tags`

The pre-PR2 `Execution.agent_session_id` compatibility reader additionally
requires an exact persisted `executor_type` in the historical Execution
snapshot, a known PR2 adapter with implemented resume, and equality with the
current immutable Agent harness identity. Contradictory/missing evidence,
Shell/Null, and historical ambiguity return no resumable session. This
read-only shim is bounded PR13 cleanup.

Verification that completed:

* `FORGE_SKIP_WEB_BUILD=1 cargo check --workspace` passed before later
  production parser/runtime-policy/credential-provenance changes, test-only
  dependency/fixture edits, and the TypeScript schema-version annotation. The
  final workspace compilation is **UNVERIFIED**.
* `cargo test -p api-types export_typescript -- --ignored --exact` passed once
  and emitted checked-in bindings. The subsequent `schema_version: number`
  annotation was mirrored statically in its TypeScript binding.
* `git diff --check` passed after source edits.
* `cargo fmt --all -- --check` failed. The clean `origin/main` baseline also
  fails formatting on existing files, and the current tree has additional
  formatter deltas in PR3-touched files. No workspace-wide reformat was applied
  because it would add broad unrelated changes.
* `FORGE_SKIP_WEB_BUILD=1 cargo test --workspace` was attempted several times.
  Early attempts exposed missing test-only imports/dependencies and stale
  fixture field access; those observed errors were corrected. A subsequent
  run was interrupted before the suite completed after the filesystem reached
  0 bytes free. `cargo clean` removed 33.0 GiB of generated Rust/Cargo files
  and restored 31 GiB free. The complete suite and focused tests are therefore
  **NOT VERIFIED**.

No frontend build/test/typecheck, Forge runtime, real provider invocation, or
database migration execution was performed. Tests are present but their final
post-fix execution **REQUIRES IMPLEMENTATION-TIME VERIFICATION**. The prior
workspace compilation result predates later production changes; current
compilation, tests, and CI-equivalent formatting are not fully verified, so
this ledger does not claim PR3 is merge-ready.

## Static audit results

At the final source audit:

* `CodingExecutorAdapter`: zero production references.
* Provider-specific resume strings under services/API/MCP/client/daemon: only
  compatibility assertions and fixtures; no production service translation.
* Direct `cli_adapters::{codex,cursor,claude,smith,gemini,opencode}` imports
  outside adapter implementations/registry construction: none found.
* Concrete Codex/Cursor usage probes in services: none; generic
  `TaskExecutor` usage observation dispatches through HarnessAdapter.
* Central `resolve_config_value` remains only under `#[cfg(test)]`; production
  normalization is adapter-owned. Effective-policy harness switches are
  removed from core.
* Remaining `ExecutorKind` branches are serialization/identification,
  generic routing identity and bookkeeping, bounded Embedded/Shell legacy
  handling, Cursor-only quota scheduling, and the exact PR2 historical
  compatibility resolver. Codex `CODEX_HOME` and Smith provider/profile
  candidate-account identity are retained to preserve PR0A routing semantics.
* Legacy Agent capabilities, typed effective `harness_capabilities`, and
  historical HarnessSession snapshots remain distinct.
* PR3 rejects Agent routing that changes harness or identity-bearing account.
  Historical PR2 sessions materialized from contradictory cross-harness or
  cross-account winners remain untouched but are not advertised or resumed
  under the current Agent identity.
* No Purpose-to-permission coupling, new planning/review workflow, new
  capability table, migration, or PR4+ lifecycle work was added.

Static searches and `git diff --check` do not establish compilation, test,
frontend, migration, or runtime correctness.

## Data preservation and rollback

Existing Execution and HarnessSession rows are not rewritten. Profile changes
affect only new executions. New effective capabilities use existing JSON
snapshot columns. The strict historical resolver is read-only and fails
closed when evidence is incomplete. Code rollback does not delete user data;
PR3-created invocation/capability semantics are not understood by older
binaries, as noted above. No destructive schema operation or ad-hoc DDL was
introduced.

## Deferred cleanup ownership

| Plan PR | Remaining owner |
| --- | --- |
| PR6 | Consume steer and pause/resume capabilities in event-driven orchestration. |
| PR7 | Consume native planning capability; remove planning permission/cognition compatibility and add plan Artifact lifecycle. |
| PR8 | Consume review-mode capability while migrating review Executions/Evidence. |
| PR10 | Remove embedded Agent Host/runtime exception, including its operator policy compatibility path. |
| PR12 | Final executor/harness public naming and surface cleanup. |
| PR13 | Remove exact PR2 historical capability fallback and historical policy/snapshot compatibility after safe persistence cleanup. |
