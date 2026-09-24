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
session identity checks.

The final verification compares the branch against `origin/main` at
`5e5d8fb29a02264c46577905a1bdfe87820f19e2`. The serial services suite has
22 shared failed test names and no PR3-only failures. The complete workspace
attempt stopped at `api/tests/daemon_connect`; all five tests also failed on
`origin/main` at the same localhost bind. The validation details below
separate these results from successful focused checks.

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
revalidated against the Agent's harness kind, the PR0A identity-bearing account
key, and equality of the generic opaque command override channels before
dispatch, including old persisted route snapshots. `env`,
`base_command_override`, and `additional_params` must be structurally identical
across same-Agent candidates because the core cannot prove that an adapter will
not use them to select another native account or context. This rejects, for
example, different Claude `CLAUDE_CONFIG_DIR` or `ANTHROPIC_API_KEY` values.
Selecting another harness or identity-bearing native account requires
explicitly selecting/reassigning another Agent. Codex `CODEX_HOME` and Smith
provider/profile identity follow the existing account-key rules. Same-harness,
same-identity variations in model, reasoning effort, sandbox, approval, and
other explicit run settings remain valid. Each accepted route candidate carries
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

Each remote execution call captures the current `DaemonConnection` generation
before protocol negotiation. Both `daemon.protocol_capabilities` and
`execution.start` are sent through that captured connection. The registry
reserves channel capacity first, then holds the same connections mutex used by
`register()` across the final generation check and synchronous permit send.
If replacement wins before that critical section, the call fails as
unavailable without sending Start on either connection. If dispatch wins the
lock first, enqueue linearizes before replacement. A later Execution performs
its own negotiation against the new connection.

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
| Daemon protocol negotiation | `generic_harness_invocation_v1` is required before every remote Start/Resume; reviewer Start also requires `execution_role_v1`; daemon Start params require `invocation`; capability check and Start share one captured connection generation, with final generation check and enqueue serialized against `register()`. | Mixed protocol versions reject before dispatch; a replacement that linearizes first returns unavailable without dispatch to stale A or replacement B. There is no provider-specific dual-write or inference fallback. | Retained transport contract; naming cleanup is PR12 |

## Related deferred reconnect debt

The incoming command-socket path still loses connection generation: API
`run_command_socket` has `connection_id`, but passes only `daemon_id` through
`handle_daemon_text_frame` to `DaemonConnectionRegistry::dispatch_incoming`.
That registry resolves the current connection by logical daemon ID, so a
frame selected from a stale socket during reconnect could be attributed to the
replacement generation. This PR3 change hardens outbound protocol negotiation
and Start enqueue only. **PR15** owns generation-bound inbound dispatch and
regression coverage for stale responses and execution notifications.

## Public surface and schema

Executor discovery adds typed `harness_capabilities` from the execution
registry. Existing executor endpoint/type vocabulary and provider-auth
capability endpoints are unchanged. Rust shared types, checked-in generated
TypeScript bindings, and the web API type were updated. The api-types
TypeScript export test completed once and emitted the checked-in bindings; the
`u64` schema version is annotated/exported as the JSON `number` it is. The
frontend build and tests were not run; `pnpm typecheck` passed, as recorded in
the verification table below.

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
  invocation, remote Resume dispatch, and same-generation protocol negotiation
  plus Start dispatch,
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
* `claude_fallback_cannot_change_config_dir_identity`
* `same_agent_fallback_cannot_change_command_environment`
* `same_agent_fallback_cannot_change_opaque_command_arguments`
* `same_agent_fallback_cannot_change_wrapper_command`
* `same_agent_fallback_allows_model_effort_sandbox_variation`
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
* `remote_dispatch_does_not_cross_daemon_connection_generation`
* `replacement_after_final_generation_check_cannot_dispatch_to_stale_connection`
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

## Verification status — 2026-09-25

### Serial services baseline comparison

Commands used the same offline Cargo environment, `RUSTFLAGS='-C debuginfo=0'`,
`FORGE_SKIP_WEB_BUILD=1`, and `--test-threads=1`.

| Revision | Result |
| --- | --- |
| PR3 final formatted tree | 647 passed, 22 failed, 1 ignored; 981.19 seconds |
| `origin/main` | 505 passed, 152 failed, 1 ignored; 870.62 seconds |

The unmodified `origin/main` services test target did not compile because two
assertions accessed `.execution` on a returned `db::Execution`. In the
temporary main worktree, only those two test field paths were corrected so
the same suite could run; production files were unchanged for that run. This
comparison uses the resulting test names:

**Shared failures (22):**

```text
lifecycle::runner::tests::test_script_hook_returns_non_zero_exit_code_for_failure
lifecycle::runner::tests::test_script_hook_returns_stdout_stderr_and_exit_zero
provider_authorization::tests::server_owned_loopback_serves_the_browser_callback
task_service::tests::service_tests::cases::dependencies::test_user_claim_bypasses_capacity_check
task_service::tests::service_tests::cases::executions::before_enter_blocks_when_required_before_work_hook_fails
task_service::tests::service_tests::cases::executions::executor_completion_guard_rejection_follows_up_before_blocking
task_service::tests::service_tests::cases::executions::follow_up_execution_codex_resumes_explicit_harness_session
task_service::tests::service_tests::cases::executions::follow_up_execution_on_blocked_task
task_service::tests::service_tests::cases::executions::follow_up_execution_reuses_explicit_harness_session
task_service::tests::service_tests::cases::executions::follow_up_execution_without_session_starts_new_execution
task_service::tests::service_tests::cases::executions::follow_up_on_cancelled_execution_reuses_active_harness_session
task_service::tests::service_tests::cases::executions::follow_up_on_cancelled_execution_without_session_starts_new_execution
task_service::tests::service_tests::cases::executions::interactive_execution_completion_does_not_trigger_review_cascade
task_service::tests::service_tests::cases::executions::launch_execution_creates_interactive_execution_and_workspace
task_service::tests::service_tests::cases::executions::re_execute_uses_current_membership_not_legacy_projection
task_service::tests::service_tests::cases::executions::role_follow_up_does_not_reuse_suspended_lineage_agent
task_service::tests::service_tests::cases::executions::role_follow_up_keeps_active_lineage_agent_over_legacy_projection
task_service::tests::service_tests::cases::executions::subtask_sequence_guard_rejection_runs_orchestrator_instead_of_coder_follow_up
task_service::tests::service_tests::cases::roles::legacy_singleton_mutations_cannot_collapse_multi_member_role
task_service::tests::service_tests::cases::roles::membership_does_not_transfer_workspace_lease_authority
task_service::tests::service_tests::cases::roles::membership_projection_failure_rolls_back_authority_and_projection
task_service::tests::service_tests::cases::roles::project_member_removal_rebuilds_projection_from_surviving_member
```

**Main-only failures (130):**

```text
agent_service::tests::effective_status_is_busy_at_capacity
daemon_transport::tests::execution_log_from_non_owner_daemon_is_rejected
daemon_transport::tests::execution_log_from_owner_updates_last_activity_at
daemon_transport::tests::execution_terminal_from_non_owner_daemon_is_rejected
daemon_transport::tests::unpinned_remote_usage_logs_use_execution_daemon_provenance
memory::tests::record_execution_summary_if_present_is_idempotent
memory::tests::record_review_result_if_final_is_idempotent
memory::tests::record_review_result_if_final_records_final_after_awaiting_human
merge_service::tests::clean_merge_returns_done
merge_service::tests::conflicting_merge_returns_conflict_and_aborts
merge_service::tests::dirty_target_repo_returns_target_dirty
merge_service::tests::dirty_worktree_returns_dirty
recovery::tests::cancel_running_executions_returns_cancelled_execution_metadata
recovery::tests::crash_recovery_cancels_running_executions
recovery::tests::crash_recovery_clears_stale_recovery_annotations_for_completed_execution
recovery::tests::crash_recovery_does_not_follow_up_resumable_execution
recovery::tests::crash_recovery_keeps_active_task_with_resumable_execution
recovery::tests::crash_recovery_preserves_legitimate_pending_recovery_annotations
recovery::tests::daemon_report_reconcile_interrupts_missing_old_running_execution
recovery::tests::daemon_report_reconcile_leaves_fresh_running_execution_untouched
recovery::tests::daemon_report_without_active_execution_ids_does_not_reconcile
recovery::tests::heartbeat_monitor_cancels_stalled_executions_of_daemonless_agents
recovery::tests::heartbeat_monitor_clears_disconnect_tracking_when_daemon_reconnects
recovery::tests::heartbeat_monitor_does_not_cancel_remote_stalled_executions_via_embedded_executor
recovery::tests::heartbeat_monitor_fails_remote_execution_when_daemon_stays_disconnected
recovery::tests::heartbeat_monitor_leaves_remote_execution_alone_within_disconnect_grace
recovery::tests::heartbeat_monitor_marks_stalled_executions_and_schedules_retry
recovery::tests::heartbeat_monitor_skips_embedded_daemon_executions_for_disconnect_check
recovery::tests::heartbeat_monitor_times_out_busy_agents_and_recovers_tasks
recovery::tests::rotated_logs_keep_execution_alive_but_silence_still_stalls
shutdown::tests::shutdown_cancels_running_executor_processes_before_recovery
shutdown::tests::shutdown_keeps_active_task_with_resumable_execution
shutdown::tests::shutdown_stops_accepting_work_and_recovers_in_progress_tasks
task_dispatcher::tests::dispatcher_dispatches_when_graceful_shutdown_stop_is_auto
task_dispatcher::tests::dispatcher_does_not_dispatch_when_graceful_shutdown_stop_is_manual
task_dispatcher::tests::dispatcher_enters_unassigned_auto_planning_gate_before_coder_dispatch
task_dispatcher::tests::dispatcher_recovers_task_stuck_in_unassigned_optional_planning_gate
task_dispatcher::tests::dispatcher_recovers_undispatched_active_task
task_dispatcher::tests::dispatcher_recovers_undispatched_reviewer_task
task_dispatcher::tests::dispatcher_respects_priority_ordering
task_dispatcher::tests::dispatcher_skips_auto_restart_for_task_cancelled_execution
task_dispatcher::tests::dispatcher_skips_auto_restart_for_user_cancelled_execution
task_dispatcher::tests::dispatcher_skips_legacy_stopped_execution_without_resume_policy
task_dispatcher::tests::dispatcher_skips_reviewer_until_configured_ci_has_finished
task_dispatcher::tests::dispatcher_skips_task_when_agent_at_capacity
task_dispatcher::tests::dispatcher_skips_unassigned_planning_gate_before_coder_dispatch
task_dispatcher::tests::dispatcher_waits_for_deferred_dispatch_cooldown
task_service::action_resolver::tests::explicit_harness_state_controls_resumability
task_service::config::tests::executor_snapshot_with_resume_thread_sets_codex_resume_thread_id
task_service::tests::action_resolver_role_targeting::test_resolve_execution_actions_targets_current_role
task_service::tests::diagnostics_exception::test_derive_workflow_exception_failed_planner_offers_retry
task_service::tests::diagnostics_exception::test_derive_workflow_exception_infers_actions_for_empty_exhausted_annotation
task_service::tests::diagnostics_exception::test_derive_workflow_exception_review_failed_no_annotation
task_service::tests::diagnostics_exception::test_merge_gate_stale_error_annotation_offers_retry_merge_when_window_available
task_service::tests::diagnostics_exception::test_retry_exhausted_blocked_metadata_takes_precedence_over_stale_error_annotation
task_service::tests::diagnostics_exception::test_reviewer_execution_failure_only_offers_retry_or_pass
task_service::tests::diagnostics_health::test_workflow_health_failed_when_coder_failed_without_block_marker
task_service::tests::diagnostics_health::test_workflow_health_running_reviewer
task_service::tests::diagnostics_health::test_workflow_health_stuck_when_coder_completed_without_transition
task_service::tests::recovery_events::test_reset_retry_window_publishes_recovery_and_resume_events
task_service::tests::recovery_reset_retry_window::test_reset_retry_window_preserves_history_and_refreshes_budget
task_service::tests::recovery_reset_retry_window::test_resume_process_moves_failed_review_back_to_in_progress
task_service::tests::service_tests::cases::claim::claim_assigns_implicit_assignee_and_uses_claim_execution
task_service::tests::service_tests::cases::claim::claim_recovers_task_branch_left_by_a_rejected_workspace_attempt
task_service::tests::service_tests::cases::claim::claim_root_with_subtask_does_not_dispatch_parent_coder_prompt
task_service::tests::service_tests::cases::claim::claim_uses_custom_workflow_active_target
task_service::tests::service_tests::cases::claim::create_claim_and_transition_task
task_service::tests::service_tests::cases::claim::default_workflow_assigns_declared_roles_not_assignee
task_service::tests::service_tests::cases::dependencies::test_done_transition_emits_dependency_satisfied_event
task_service::tests::service_tests::cases::executions::ambiguous_historical_session_fails_closed_for_resume_and_actions
task_service::tests::service_tests::cases::executions::before_enter_runs_required_before_work_hook_before_role_dispatch
task_service::tests::service_tests::cases::executions::claim_task_records_codex_overrides_in_normalized_snapshot
task_service::tests::service_tests::cases::executions::claim_task_records_execution_permission_policy_override_in_snapshot
task_service::tests::service_tests::cases::executions::dispatch_initial_role_execution_creates_execution_and_spawns
task_service::tests::service_tests::cases::executions::dispatch_initial_role_execution_runs_reviewer_when_agent_is_busy_on_same_task
task_service::tests::service_tests::cases::executions::executor_completion_comment_uses_execution_agent
task_service::tests::service_tests::cases::executions::executor_completion_guard_rejection_blocks_when_retry_budget_exhausted
task_service::tests::service_tests::cases::executions::failed_reviewer_execution_marks_running_review_failed
task_service::tests::service_tests::cases::executions::follow_up_execution_rejects_executor_mismatch
task_service::tests::service_tests::cases::executions::follow_up_execution_rejects_running_parent
task_service::tests::service_tests::cases::executions::follow_up_execution_rejects_terminal_task
task_service::tests::service_tests::cases::executions::follow_up_rejects_a_running_repository_role_without_mutating_task
task_service::tests::service_tests::cases::executions::passed_reviewer_execution_with_user_approval_gate_waits_for_human
task_service::tests::service_tests::cases::executions::planner_completion_marks_task_awaiting_plan_review_until_approved
task_service::tests::service_tests::cases::executions::re_execute_rejects_concurrent_running_execution
task_service::tests::service_tests::cases::executions::re_execute_rejects_running_parent
task_service::tests::service_tests::cases::executions::recover_reexecute_without_blocked_execution_dispatches_current_state_role
task_service::tests::service_tests::cases::executions::retry_hook_reruns_blocked_before_enter_and_dispatches_when_it_passes
task_service::tests::service_tests::cases::executions::run_execution_batches_execution_log_events
task_service::tests::service_tests::cases::executions::run_execution_dispatches_shell_adapter_and_updates_execution
task_service::tests::service_tests::cases::executions::run_execution_emits_terminal_execution_event
task_service::tests::service_tests::cases::executions::run_execution_rejects_when_terminal_active_in_workspace
task_service::tests::service_tests::cases::executions::skip_hook_once_bypasses_only_one_dispatch_attempt
task_service::tests::service_tests::cases::roles::cancel_execution_invokes_task_executor_cancel
task_service::tests::service_tests::cases::roles::cancel_task_cancels_running_execution
task_service::tests::service_tests::cases::roles::invalid_legacy_reassignment_preserves_running_execution
task_service::tests::service_tests::cases::roles::reassign_coder_to_human_mid_execution_cancels_and_moves_to_todo
task_service::tests::service_tests::cases::roles::reassign_coder_with_workspace_allows_both_reset_flags
task_service::tests::service_tests::cases::roles::reassign_coder_with_workspace_allows_reset_workspace
task_service::tests::service_tests::cases::roles::reassign_mid_exec_coder_with_reset_worktree_flag_in_event
task_service::tests::service_tests::cases::roles::reassign_non_coder_role_does_not_cancel_or_transition
task_service::tests::service_tests::cases::roles::reassign_role_cancels_running_active_executor
task_service::tests::service_tests::cases::roles::run_execution_rechecks_cancelled_status_before_adapter_launch
task_service::tests::service_tests::cases::roles::system_transition_does_not_cancel_running_active_executor
task_service::tests::service_tests::cases::roles::user_transition_cancels_running_active_executor_before_status_change
task_service::tests::service_tests::cases::subtask_modes::batch_5_6_root_claim_starts_subtask_sequence
task_service::tests::service_tests::cases::transitions::transition_to_review_runs_configured_review_runner
task_service::tests::service_tests::cases::user_override::override_move_out_of_active_state_cancels_running_execution
task_service::tests::service_tests::cases::user_override::park_running_task_to_backlog
task_service::tests::service_tests::cases::user_override::user_subtask_into_review_review_pass_cascade_and_hooks_succeed
workflow::actions::tests::auto_cascade_review_failure_at_budget_blocks_with_metadata
workflow::actions::tests::auto_cascade_review_failure_budget_blocks_with_metadata
workflow::actions::tests::ci_fails_reviewer_not_dispatched_cascade_handles_bounce
workflow::actions::tests::ci_passes_then_reviewer_dispatched_via_dispatch_role_agent
workflow::actions::tests::dispatch_role_agent_emits_event_for_coder_assignment
workflow::actions::tests::dispatch_role_agent_initial_dispatch_creates_execution_with_capacity
workflow::actions::tests::dispatch_role_agent_initial_dispatch_skips_at_capacity
workflow::actions::tests::dispatch_role_agent_initial_dispatch_skips_when_execution_already_running
workflow::actions::tests::merge_fix_re_review_runs_ci_only_and_skips_reviewer
workflow::actions::tests::planning_rejection_budget_allows_revision_at_configured_limit
workflow::actions::tests::reviewer_at_capacity_ci_runs_dispatch_queues
workflow::actions::tests::reviewer_dispatch_ignores_waiting_review_tasks_without_running_execution
workflow::actions::tests::run_ci_steps_creates_passed_review_record
workflow::actions::tests::run_ci_steps_failure_prevents_reviewer_dispatch
workflow::actions::tests::run_ci_steps_keeps_review_running_when_reviewer_at_capacity
workflow::actions::tests::run_ci_steps_pass_then_dispatches_reviewer
workflow::actions::tests::run_ci_steps_with_user_approval_gate_waits_for_human
workflow::actions::tests::run_ci_steps_without_reviewer_cascades_to_merging
workflow::actions::tests::subtask_root_still_dispatches_reviewer_after_coder_completion
workflow::actions::tests::unconfigured_review_with_user_approval_gate_waits_for_human
```

**PR3-only failures:** none.

The main-only set includes tests that fail before later assertions because
main's `execution` INSERT supplies 28 values for 27 columns. PR3 removes the
extra SQL placeholder. To compare behavior after that blocker, the temporary
main worktree received only that one-line SQL correction and the two test
accessor corrections. Then the full `task_service::tests::service_tests::cases::executions::`
filter failed 14 tests; those names and their post-insert failures match the
14 execution failures in the PR3 full services run. The exact
`test_user_claim_bypasses_capacity_check` also failed on both revisions after
the SQL correction. No production change from PR3 creates an additional
services failure.

### Focused checks

| Target | Result |
| --- | --- |
| `cargo test -p cli-adapters --lib -- --test-threads=1` | 105/105 passed on final formatted tree |
| `cargo test -p executors --lib -- --test-threads=1` | 64/64 passed on final formatted tree |
| `cargo test -p services --lib daemon_transport::tests:: -- --test-threads=1` | 15/15 passed on final formatted tree, including connection-generation serialization and exact Resume protocol cases |
| `cargo test -p api-types --lib harness_capability_tests -- --test-threads=1` | 3/3 passed on final formatted tree |
| `cargo test -p db --test pr2_execution_session -- --test-threads=1` | PR3: 15/16; main: 1/15. The exact `historical_session_migration_groups_only_coherent_identity` failed on both with the same SQLite foreign-key error. |
| `cargo test -p api --test fs_daemon_routing -- --test-threads=1` | PR3: 15/18. Main before the SQL correction: 13/18; main after that correction: 15/18. The same 3 tests fail on all runs while binding localhost with `PermissionDenied`. |
| `cargo test -p api --test remote_execution_roundtrip -- --test-threads=1` | PR3 and main: 0/4; all four fail binding localhost with `PermissionDenied`. |
| `cargo test -p api --test daemon_connect -- --test-threads=1` | PR3 workspace run and main: 0/5; all five fail binding localhost with `PermissionDenied`. |
| `cargo test -p api --test task_diff -- --test-threads=1` | PR3: 0/2 at the TaskRole lease guard. Main: 0/2 at the 28-values/27-columns INSERT; after replaying the one-line PR3 SQL correction in the temporary worktree, both reach and fail at the same TaskRole lease guard. |

The failed localhost tests are reported as comparison results, not inferred
environment labels: their exact test targets were run on both revisions and
returned the same bind error.

### Compile, format, bindings, and workspace

| Check | Result |
| --- | --- |
| `FORGE_SKIP_WEB_BUILD=1 cargo check --workspace` | PASS on final formatted tree (1m55s) |
| `cargo fmt --all -- --check` | FAIL on both PR3 and main only at unchanged `crates/api/src/routes/mod.rs` lines 11, 337, and 1241 |
| Individual `rustfmt --check` for all changed Rust files | PASS 87/87, using each crate's declared edition (2024 for `cli-adapters` and `agent-host`, 2021 elsewhere) |
| `cargo test -p api-types export_typescript -- --ignored --exact` | PASS 1/1; checked-in TypeScript bindings were generated |
| `pnpm typecheck` | PASS (`tsc -b`) |
| `git diff --check` | PASS after removing generated trailing whitespace |
| `cargo test --workspace -- --test-threads=1` | INCOMPLETE: stopped at `api/tests/daemon_connect` (0/5). The same exact target on main also failed 0/5 with the same bind error; Cargo did not reach later targets. |

No database migration was added; schema head remains V089. The full workspace
test command stopped at the first failed integration target, not because of
disk exhaustion. The task-local Cargo targets and temporary main worktree were
removed after recording the results. No frontend build/test, Forge runtime,
real provider invocation, database migration execution, or CI run was done.
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
