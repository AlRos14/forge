# Plan PR3 — Capability-Driven HarnessAdapter Exit Ledger

## Boundary and result

Plan PR3 replaces the external harness protocol authority with the typed
`HarnessAdapter` boundary while retaining the ordered migration sequence.
`ExecutorKind` and the public `executor_type` vocabulary remain compatibility
names. This change does not implement planning artifacts, review workflow
migration, orchestration, embedded-runtime removal, or final public renames.

The implementation is based on `5e5d8fb29a02264c46577905a1bdfe87820f19e2`
and the static preflight in [`plan-pr3-preflight.md`](plan-pr3-preflight.md).
The source tree and tests were reviewed statically. Rust, Cargo, frontend,
database migration, and runtime verification were intentionally not run;
those claims **REQUIRE IMPLEMENTATION-TIME VERIFICATION**.

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
| Codex | Native; Emulated; Native; Native; Native; Native; Native; Native; Native; Unsupported; Unsupported; Native; Unsupported; Unsupported; Unsupported; Unsupported | App-server thread resume/fork; normalized JSON-RPC events and usage; account rate-limit RPC; explicit model/reasoning/approval/sandbox params. `PermissionPolicy::Plan` maps restrictions only. Process cancellation is Forge-emulated. |
| Claude Code | Native; Emulated; Native; Native; Unsupported; Native; Native; Native; Unsupported; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | Explicit `--resume`, stream-json event parsing, model/effort/permission flags, usage normalization; configured native `--permission-mode plan`; process cancellation is Forge-emulated. |
| Cursor | Native; Emulated; Native; Native; Emulated; Native; Unsupported; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unknown; Unsupported; Unsupported; Unsupported | Explicit `--resume`, stream-json parsing and token usage, model/force flags; Forge PTY `/usage` observation and text parsing; no proven steering protocol; process cancellation is Forge-emulated. |
| OpenCode | Native; Emulated; Native; Unsupported; Unsupported; Native; Unsupported; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | `--session`, JSON event parsing, model and permission flags; no usage extraction; process cancellation is Forge-emulated. |
| Gemini | Unsupported; Emulated; Unknown; Unsupported; Unsupported; Native; Unsupported; Native; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | JSON-shaped line parsing is implemented but no structured-event contract is established; model, `--yolo`, and sandbox flags exist; no session ID or usage; process cancellation is Forge-emulated. |
| Smith | Native; Emulated; Native; Native; Unsupported; Native; Native; Native; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | Explicit resume, stream-json parsing and per-turn usage, model/effort/approval flags; process cancellation is Forge-emulated. |
| Shell | Unsupported; Emulated; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | ShellExecutor process termination; no harness session, model, cognition, structured event, or usage protocol. Reviewer compatibility command is translated inside ShellAdapter. |
| Null | Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported; Unsupported | Delay/no-op test adapter has no session or cognition protocol; cancellation is a no-op. |

Shell and Null expose no cognitive capabilities. Only Claude Code reports
native planning, based on its configured explicit native plan-mode argument.
Codex `PermissionPolicy::Plan`, read-only sandboxes, propose-only behavior,
and prompt conventions do not imply native planning. `ExecutionPurpose::Plan`
remains independent from permission and is not turned into a planning
workflow. Provider credential/runtime capability types remain separate from
HarnessCapabilities.

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

New HarnessSession `capabilities_snapshot_json` values come from
`harness_capabilities`. Pending-session materialization prefers that field;
result-time binding updates the pending session with the actual winner's
snapshot. Existing active session snapshots are not recomputed when an Agent
profile changes. Historical snapshots are not rewritten. Strict typed
snapshots are parsed as PR3 evidence; unknown/malformed content is Unknown.
The narrow PR2 compatibility resolver only recognizes its exact historical
snapshot shapes and explicit known resumable harness kinds. PR13 owns removing
that resolver.

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

Start retains ordered fallback behavior. Each route candidate carries its own
normalized config, `HarnessCapabilities`, effective policy, candidate/account
identity, and adapter. The actual winner is attached to the local result and
remote terminal result. Its capability snapshot flows into the Execution and
pending HarnessSession. Thus Codex A → Codex B and Codex → Cursor both preserve
the winner's evidence rather than the primary candidate's.

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
`ExecutionStartParams` to the daemon. Local and daemon execution converge on
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
isolation posture. Forge core retains workspace containment and high-risk
classification/enforcement. Codex `danger-full-access` remains high risk;
Claude skip-permissions remains high risk; Cursor force/propose behavior maps
to the generic policy. Adapter interpretation does not grant authority or
authorize risky work. New snapshots capture primary-adapter policy before
dispatch; the selected route result replaces it with the actual winner's
policy. The old service-side harness-kind interpreter remains only for
historical snapshots (PR13) and the bounded Embedded exception (PR10).

## Embedded exception and transitional facade

`ExecutorKind::Embedded`, embedded task execution, Agent Host, and its
Forge-owned cognition remain the existing bounded legacy exception. PR3 does
not add a permanent full EmbeddedHarnessAdapter or expand that runtime.
Cleanup owner: PR10.

`TaskExecutor` remains the supervisor-facing generic facade for TaskService,
FallbackExecutor, daemon execution, embedded routing, and tests. It owns
generic routing/fallback/cancellation coordination only. `CodingExecutorAdapter`
and its registry are removed as production authority.

## Public surface and schema

Executor discovery adds typed `harness_capabilities` from the execution
registry. Existing executor endpoint/type vocabulary and provider-auth
capability endpoints are unchanged. Rust shared types, checked-in generated
TypeScript bindings, and the web API type were updated statically; generation
and frontend consistency **REQUIRE IMPLEMENTATION-TIME VERIFICATION**.

No migration was added, no persisted history was rewritten, and migration head
remains V089. The change is additive in capability snapshot JSON and API
discovery. Reverting the code preserves database rows; however, an older binary
does not understand the generic invocation or new capability member, so
resuming sessions created through PR3 after software rollback may require
forward-fix/recovery with the PR3 binary. No rollback or migration execution
was performed.

## Tests added, not executed

Focused test additions cover:

* Support-level distinctions, Unknown/Unsupported fail-closed behavior, and
  strict typed snapshot parsing.
* All eight adapter capability matrices, Shell/Null cognition limits,
  PermissionPolicy::Plan separation, and generic effective-policy posture.
* Generic exact Resume translation in Codex, Claude Code, Cursor, Smith, and
  OpenCode, plus Start sanitation of stale session configuration.
* Shell reviewer command translation inside the adapter.
* Generic adapter invocation, unsupported Resume, no Resume fallback,
  cross-harness and same-harness winner capability propagation, and usage
  observation dispatch.
* Historical PR2 capability parsing, legacy Agent tags versus new typed
  snapshots, actual-winner Execution/HarnessSession snapshots, and preserving
  active historical snapshots after profile changes.
* Daemon invocation serialization/backward defaults, remote Resume dispatch,
  and remote winner capability results.
* Review adapter winner capability snapshot and retained legacy Agent tags.

Named focused test additions include:

* `support_levels_remain_distinct_and_unknown_fails_closed`
* `typed_snapshot_requires_all_dimensions_and_rejects_legacy_extra_fields`
* `registered_adapters_expose_only_integration_evidence`
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
* `cross_harness_start_fallback_records_cursor_capabilities`
* `same_harness_start_fallback_records_profile_b_capabilities`
* `sticky_resume_promotes_cross_harness_capabilities_and_policy`
* `unknown_resume_is_an_explicit_unsupported_capability`
* `generic_usage_observation_routes_through_registered_adapter`
* `historical_resume_support_is_typed_or_narrow_pr2_compatibility`
* `initial_harness_policy_comes_from_the_registered_adapter`
* `profile_and_capability_changes_do_not_rewrite_an_active_session_snapshot`
* `generic_start_and_resume_survive_daemon_transport_serialization`
* `old_daemon_start_payload_defaults_to_generic_start`
* `remote_resolved_candidate_carries_effective_harness_capabilities`
* `remote_resume_intent_reaches_harness_adapter_and_reports_capabilities`
* `auditor_snapshot_records_resolved_adapter_capabilities_without_overwriting_agent_tags`

These tests were added but **NOT RUN**. No Cargo/Rust command, frontend build or
test, Forge runtime, provider invocation, or database migration execution was
performed. All behavioral and compilation claims **REQUIRE IMPLEMENTATION-TIME
VERIFICATION**.

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
