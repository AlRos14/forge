# Plan PR10: remove embedded Agent Host cognition

Status: complete. Base: `e8b21cf448ea4793b86dffe40722a269241528ba` (`origin/main`, PR9).

## Authority map at the verified base

The repository has two Agent execution paths. The production external path is
`TaskService` → `TaskExecutor` / `FallbackExecutor` → `HarnessAdapterRegistry`
→ the selected external adapter. The bounded legacy path is:

```text
api::AppState
  ├─ EmbeddedAgentService(db, JWT secret)
  │    ├─ SqliteProtectedRuntimeStore
  │    ├─ NativeAgentRuntimeBackend
  │    │    ├─ RuntimeBuilder / agent-runtime provider and context loop
  │    │    ├─ ScopeToolComposition / native Forge tools
  │    │    └─ InteractionBroker / runtime and checkpoint persistence
  │    └─ CoordinationToolProvider
  ├─ TaskExecutorRouter(FallbackExecutor, EmbeddedTaskExecutor)
  │    └─ EmbeddedTaskExecutor
  │         ├─ AgentProfile + `backend_kind=native` / `executor_type=embedded`
  │         ├─ AgentSession scoped by Task and role
  │         └─ NativeAgentRuntimeBackend.run_turn(AgentTurnRequest)
  ├─ AgentChatTurnWorker
  │    └─ FederatedAgentChatTurnRunner
  │         ├─ native profile → AgentSession → NativeAgentRuntimeBackend.run_turn
  │         └─ CLI profile → CliAgentChatSessionBackend → TaskExecutor
  ├─ TaskService.with_provider_credential_env(EmbeddedAgentService)
  ├─ ProviderAuthorizationService(EmbeddedAgentService / protected store)
  └─ embedded-agent and provider API routes
```

The current native Agent Host also has deterministic callers around the
cognitive path: `CoordinationToolProvider` is composed only by the native
runtime; its Main/Project action services also have direct API callers and
remain separate. Provider credential storage has non-cognitive consumers:
provider entry/authentication and usage routes, plus TaskService's in-memory
API-key environment injection for external CLI harnesses. The storage must
therefore move without moving runtime state or provider/model loops.

The source and manifest census found the workspace member/dependency in the
root `Cargo.toml`, `agent-runtime` as a workspace dependency, and direct
`forge-agent-host` dependencies only in `services` and `api`. Startup
composition is in `crates/api/src/state.rs`; the CLI bootstraps the registered
external adapters. The Embedded selection/config boundary is in
`crates/executors/src/{adapter,config}.rs`, default Agent creation in
`services/src/default_agents.rs`, and Task admission/dispatch in
`services/src/task_service/**`.

## Old and new authority

| Concern | Before PR10 | After PR10 |
| --- | --- | --- |
| Agent Task execution | Embedded snapshots route to `EmbeddedTaskExecutor`; other snapshots use `FallbackExecutor` and HarnessAdapters. | All admitted Agent work dispatches through the exact HarnessAdapter selected by the persisted route snapshot. Retired native/embedded snapshots produce an explicit unsupported/migration-required failure. |
| Agent Chat cognition | Native profile invokes `NativeAgentRuntimeBackend`; CLI profile invokes the external adapter. | Chat history, immutable messages, durable turn jobs, binding/profile provenance, operating-context provenance, and retry state remain stored and readable. Productive model invocation is fail-closed for every current adapter because none proves a real no-filesystem boundary. Historical native/embedded bindings also fail durably without provider calls or alternate identity selection. PR11 owns retirement or migration of these verticals. |
| Continuity | Native Task/Chat paths create or resume `AgentSession`; older Task records could resume through the legacy `Execution.agent_session_id` projection. | `HarnessSession` is the only generic Agent execution continuity authority. No new `AgentSession` is created or consulted for execution, and the legacy projection cannot authorize Resume or be materialized into a new session. Agent Chat jobs retain profile/binding provenance but fail closed before current production adapters start; no session is inferred. |
| Credentials | Credential encryption, OAuth refresh/revocation, provider authorization state, runtime checkpoints, and session state share `SqliteProtectedRuntimeStore`. | Credential-only encryption/storage and provenance move to a service named for credentials, retaining the existing ciphertext format and rows. Task Executions freeze `credential_ref`; only local in-process dispatch injects its secret in memory. Runtime/session/checkpoint/interaction state has no execution consumer and remains historical storage. |
| Native tool catalog | `CoordinationToolProvider` supplies scope-derived tools to RuntimeBuilder. | Removed with the cognitive runtime. Deterministic Main/Project action services and API routes remain; this does not migrate their verticals. |
| Gates and lifecycle | Forge's deterministic PR6–PR9 services already own these decisions. | No authority change. |

Harness identity remains part of Agent identity. A persisted `backend_kind=native`
or `executor_type=embedded` is not normalized to a CLI kind, fallback, Agent,
account, or credential. Public endpoints that create, connect, resume, steer,
or execute the retired runtime fail explicitly. Read-only profile/session
projections and all underlying historical records remain available where the
existing surface permits them.

## Compatibility storage classification

| Data | PR10 treatment |
| --- | --- |
| Embedded/native Agent and profile rows, including snapshots | Historical/audit storage; readable; cannot authorize a new execution or identity conversion. |
| `agent_session` and `agent_context_scope` | Historical/audit storage; no generic continuity authority and no new runtime session writes. |
| `execution.agent_session_id` | Historical/audit response projection; semantically dead for resume authority and never materialized into a `HarnessSession`. |
| Protected runtime state, checkpoints, interaction payloads, LCM rows, and runtime context manifests | Historical/audit storage with no post-PR10 cognitive consumer. Do not decrypt, resume, compact, or delete them in PR10. |
| Credential handles, encrypted API keys/OAuth bundles, credential usage, and provider authorization records | Retained for credential ownership, provider authorization/usage routes, and external API-key injection. Preserve the existing key derivation, ciphertext encoding, versions, and ownership checks. |
| Server-owned Agent Chat context manifests | Retain as durable operating-context provenance with redaction-safe source references and no `AgentSession` link. They do not imply that model invocation or Forge operations are currently available. Runtime/LCM manifest production is removed. |
| PR2 `HarnessSession`, Execution profile/capability snapshots, and legacy `execution.agent_session_id` projection | Explicit HarnessSession references remain authoritative. Legacy IDs are read-only projections and never authorize Resume; PR13 owns physical compatibility cleanup. |

## Phase E component disposition

| Component | Old owner | Post-PR10 consumer | New owner | Reason |
| --- | --- | --- | --- | --- |
| API-key and OAuth credential encryption, refresh, ownership, revocation, and in-memory adapter environment injection | `EmbeddedAgentService` and `SqliteProtectedRuntimeStore` | Provider routes, provider authorization, Agent credential ownership, and TaskService's external HarnessAdapter launch | `CredentialService` / `CredentialStore` | Retained as deterministic credential infrastructure; existing ciphertext table, key derivation, revision, and ownership records remain compatible. |
| Provider authorization state sealing | `SqliteProtectedRuntimeStore` | `ProviderAuthorizationService` browser/device flow | `CredentialService` / `CredentialStore` | Retained because OAuth authorization still has a post-PR10 provider/API consumer. |
| Native model/provider loop, native runtime sessions, `ScopeToolComposition`, interaction broker, and runtime LCM/context assembly | `NativeAgentRuntimeBackend`, `agent-host`, `agent-runtime`, and `EmbeddedAgentService` | None | Removed | No post-PR10 consumer; preserving it would retain Forge-owned cognition. |
| Native typed tool catalog | `CoordinationToolProvider` in `services::native_tools` | None after native runtime removal. Deterministic Main/Project action services and their API routes remain separate. | Removed | The catalog and composition are the cognition/tool boundary retired by PR10. PR11 still owns Main/Project vertical migration. |
| In-process `EmbeddedExecutionProvider` | `services::daemon_transport` | Local daemon execution dispatch | `services::daemon_transport` | Retained as transport/supervision only; it delegates to TaskService and the same HarnessAdapter-backed TaskExecutor and contains no cognition. |
| `agent_session` rows and `AgentSession` | Native runtime and `EmbeddedAgentService` | Read-only historical profile/session APIs, operator/attention projections, and non-cognitive credential revocation health update | No runtime owner | Preserved for audit and dependent-health display; never creates, resumes, or authorizes Agent execution. Physical cleanup is PR13. |
| `agent_context_scope` | Native runtime scope and `EmbeddedAgentService` | Historical session projections and durable Agent Chat context-manifest FK storage | No runtime owner; `AgentChatTurnWorker` creates deny-workspace storage scopes for manifests | Retained without tool or membership authority. No new session/runtime scope is created. Physical cleanup is PR13. |
| Protected runtime/session state, checkpoints, interaction payloads, and LCM rows | `SqliteProtectedRuntimeStore` and runtime/LCM services | None after PR10 | No runtime owner | Historical encrypted/audit storage; no new runtime reads or writes. Physical cleanup is PR13. |
| Context manifests | Native runtime plus Agent Chat worker | Server-owned Agent Chat operating-context provenance | `ContextManifestService` and `AgentChatTurnWorker` | Provenance remains useful without a runtime session; new manifests use a null `agent_session_id` and a deny-workspace scope. It is historical/auditable context, not proof of a running Agent Chat model call. |

No schema migration is planned. Code-level admission and dispatch fences are
sufficient to prevent new Embedded execution; historical rows and protected
payloads remain untouched for audit and future PR13 cleanup.

## Final Execution and Agent Chat boundaries

Admission selects a daemon while building a new Execution snapshot and stores
both `agent_daemon_id` and the concrete `resolved_daemon_id`. Start, Resume,
Cancel, graceful shutdown, and recovery of that admitted Execution use only
`resolved_daemon_id`. They never re-resolve the current Agent's daemon
binding or current availability order. A missing or invalid resolved host
fails closed; no fallback to a mutable Agent field is supported for historical
snapshots. A reconnect of the same logical daemon remains valid and continues
to use PR3's connection-generation checks.

The Execution snapshot also freezes `credential_ref`. Current Agent/profile
changes cannot select another credential. The secret is resolved in memory
for local dispatch only; a remote Start with a Forge-owned credential fails
before dispatch because the remote protocol cannot prove the same credential
identity.

Agent Chat remains stored and auditable: messages are immutable, turn jobs and
their retry outcomes are durable, and binding/profile and operating-context
provenance remain readable. Current production adapters cannot demonstrate
`permission_policy=deny` plus `isolation_posture=no-filesystem`; all
productive Main/Project Agent Chat model invocation therefore fails closed.
The generic adapter fence is tested separately with a contract fixture. No
native typed operations or scoped tool catalog are available through Agent
Chat. PR11 owns retirement or migration of Main/Project Agent Chat and Project
OS; PR10 adds no sandbox, process-isolation, or cognition infrastructure.

## Files and migration strategy

1. **Admission fence:** keep only the minimal legacy decode needed to recognize
   the persisted `embedded` spelling; remove it from discovery, default Agent
   creation, fallback candidates, capabilities, and execution dispatch. Add a
   bounded retired-profile error and prove there is no fallback identity change.
2. **Task execution:** delete `embedded_task_executor.rs` and the router. Keep
   `TaskExecutor` as the generic supervisor and dispatch directly through the
   existing FallbackExecutor/HarnessAdapter path. Preserve exact HarnessSession
   Start/Resume and adapter cancellation/recovery behavior.
3. **Agent Chat:** remove native runtime execution and runtime session
   creation. Keep Main/Project chat persistence, operating-context provenance,
   message history, profile/binding provenance, and bounded job failure/retry.
   Fail closed before model invocation while no production adapter proves
   no-filesystem isolation. PR11 owns vertical retirement or migration.
4. **Credential split:** extract only credential encryption, API-key/OAuth
   storage/refresh/revocation, ownership/provenance, provider authorization
   state, and non-cognitive usage probes into a service with real provider/API,
   TaskService, and provider-authorization consumers. Do not copy the runtime
   service or create an agent-host replacement crate.
5. **Dependency removal:** remove `crates/agent-host`, its workspace member,
   its direct consumers, and `agent-runtime`; regenerate lockfile changes only
   as a consequence. Keep deterministic coordination services and constants
   in a neutral service/API-types location if they still have API consumers.
6. **Persistence boundary and docs:** retain schema and historical records;
   update Architecture v2, PR3's bounded exception status, and the relevant
   Agent/Execution/Agent Chat docs. PR11 owns vertical retirement/migration,
   PR12 broad REST/MCP/UI naming cleanup, and PR13 physical schema cleanup.

## Implementation and verification result

Remote command work is owned by the daemon command connection generation
whose `DaemonRuntime` launched it. Disconnect and graceful shutdown close that
runtime to new Starts, cancel its active Executions through the same
`FallbackExecutor` and adapter instances, then wait for bounded task
termination before a reconnect can create a new runtime. A new generation for
the same logical daemon never adopts old process handles. Logs or a terminal
notification sent after the socket disappears may be lost; the existing daemon
report and recovery paths reconcile durable Execution state. If bounded
teardown cannot complete, the daemon stops instead of reconnecting alongside
work it can no longer control. This ephemeral control lifetime does not change
the Execution's frozen `resolved_daemon_id` or HarnessSession authority.

No database migration was required. Retired runtime and protected payload rows
remain historical; `execution.agent_session_id` remains readable but is no
longer consulted to resume or materialized into `HarnessSession`. The
`CredentialService` retains API-key and OAuth storage, refresh, ownership, and
provider-auth state for its live provider, authorization, and external adapter
consumers.

Focused verification on the final implementation covered frozen Start for a
pinned and an unpinned Agent, frozen Cancel, daemon-report recovery, remote
shutdown Cancel, fail-closed legacy snapshots, exact HarnessSession Resume,
PR3 connection-generation behavior, snapshot credential injection/revocation,
remote credential rejection, the production Agent Chat registry, and the
separate generic no-Workspace contract. The current production registry
rejects every claimed string-only no-filesystem posture; the test reads the
actual registry kinds rather than maintaining a duplicate adapter list.

Passing focused commands:

```text
cargo test -p services --lib frozen --locked --offline -- --test-threads=1
cargo test -p services --lib execution_host_does_not_follow_a_mutable_agent_daemon_binding --locked --offline -- --test-threads=1
cargo test -p services --lib legacy_execution_snapshot_without_resolved_daemon_fails_closed --locked --offline -- --test-threads=1
cargo test -p services --lib shutdown_cancels_remote_execution_on_frozen_daemon_after_agent_rebind --locked --offline -- --test-threads=1
cargo test -p services --lib shutdown_cancels_running_executor_processes_before_recovery --locked --offline -- --test-threads=1
cargo test -p services --lib remote_resume_requires_protocol_feature_and_dispatches_exact_session --locked --offline -- --test-threads=1
cargo test -p services --lib remote_dispatch_does_not_cross_daemon_connection_generation --locked --offline -- --test-threads=1
cargo test -p services --lib follow_up_execution_codex_resumes_explicit_harness_session --locked --offline -- --test-threads=1
cargo test -p services --lib legacy_execution_session_id_never_authorizes_resume_or_actions --locked --offline -- --test-threads=1
cargo test -p services --lib external_harness_credential_stays_encrypted_until_in_memory_injection --locked --offline -- --test-threads=1
cargo test -p services --lib remote_provider_rejects_snapshot_credentials_before_any_daemon_request --locked --offline -- --test-threads=1
cargo test -p services --lib agent_chat_uses_the_job_profile_credential_after_current_profile_changes --locked --offline -- --test-threads=1
cargo test -p services --lib generic_no_workspace_contract_accepts_adapter_declared_posture --locked --offline -- --test-threads=1
cargo test -p services --lib production_adapter_registry_does_not_prove_agent_chat_no_workspace --locked --offline -- --test-threads=1
cargo test -p services --lib main_chat_turn_keeps_durable_provenance_and_fails_closed_for_production_adapter --locked --offline -- --test-threads=1
FORGE_SKIP_WEB_BUILD=1 cargo check -p forge-cli --locked --offline
cargo fmt --all -- --check
git diff --check
```

An incidental `cargo test -p services --lib no_workspace --locked --offline --
--test-threads=1` filter also selected
`workflow::engine::tests::subtask_user_override_into_review_no_workspace_no_reviewer_completes`, which failed at the Task lifecycle projection
trigger in `crates/services/src/workflow/engine/tests.rs`. The two exact
Agent Chat no-Workspace tests above pass; the workflow test is outside the
changed files and was not used as PR10 acceptance evidence.

Static searches found no executable production references to
`agent-runtime`, `forge-agent-host`, `NativeAgentRuntimeBackend`,
`EmbeddedTaskExecutor`, `CoordinationToolProvider`, `ScopeToolComposition`, or
`ExecutorKind::Embedded`; remaining mentions are historical documentation.
No migration, workspace-wide test/build/clippy, or target cleanup was run.
The shared target measured 13 GB before focused compilation and 14 GB after;
free disk moved from 98 GB to 97 GB.
