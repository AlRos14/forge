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
| Agent Chat cognition | Native profile invokes `NativeAgentRuntimeBackend`; CLI profile invokes the external adapter. | Only the external HarnessAdapter path runs. Each admitted turn uses the bound profile's exact adapter and starts a new invocation. Historical native/embedded bindings fail through the existing durable job failure path, with no provider call or alternate identity selection. |
| Continuity | Native Task/Chat paths create or resume `AgentSession`; older Task records could resume through the legacy `Execution.agent_session_id` projection. | `HarnessSession` is the only generic Agent execution continuity authority. No new `AgentSession` is created or consulted for execution, and the legacy projection cannot authorize Resume or be materialized into a new session. Agent Chat starts through its exact bound adapter with no inferred session. |
| Credentials | Credential encryption, OAuth refresh/revocation, provider authorization state, runtime checkpoints, and session state share `SqliteProtectedRuntimeStore`. | Credential-only encryption/storage and provenance move to a service named for credentials, retaining the existing ciphertext format and rows. Runtime/session/checkpoint/interaction state has no execution consumer and remains historical storage. |
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
| Server-owned Agent Chat context manifests | Retain for the external Chat path; they may contain redaction-safe operating-context references and need no `AgentSession` link. Runtime/LCM manifest production is removed. |
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
| `agent_context_scope` | Native runtime scope and `EmbeddedAgentService` | Historical session projections and external Agent Chat context-manifest FK storage | No runtime owner; `AgentChatTurnWorker` creates deny-workspace storage scopes for manifests | Retained without tool or membership authority. No new session/runtime scope is created. Physical cleanup is PR13. |
| Protected runtime/session state, checkpoints, interaction payloads, and LCM rows | `SqliteProtectedRuntimeStore` and runtime/LCM services | None after PR10 | No runtime owner | Historical encrypted/audit storage; no new runtime reads or writes. Physical cleanup is PR13. |
| Context manifests | Native runtime plus Agent Chat worker | Server-owned Agent Chat operating-context provenance | `ContextManifestService` and `AgentChatTurnWorker` | Chat provenance remains useful without a runtime session; new manifests use a null `agent_session_id` and a deny-workspace scope. |

No schema migration is planned. Code-level admission and dispatch fences are
sufficient to prevent new Embedded execution; historical rows and protected
payloads remain untouched for audit and future PR13 cleanup.

## Files and migration strategy

1. **Admission fence:** keep only the minimal legacy decode needed to recognize
   the persisted `embedded` spelling; remove it from discovery, default Agent
   creation, fallback candidates, capabilities, and execution dispatch. Add a
   bounded retired-profile error and prove there is no fallback identity change.
2. **Task execution:** delete `embedded_task_executor.rs` and the router. Keep
   `TaskExecutor` as the generic supervisor and dispatch directly through the
   existing FallbackExecutor/HarnessAdapter path. Preserve exact HarnessSession
   Start/Resume and adapter cancellation/recovery behavior.
3. **Agent Chat:** remove only native runtime execution and runtime session
   creation. Keep Main/Project chat persistence, operating-skill rendering,
   message history, bounded job failure/retry, and external adapter execution.
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

No database migration was required. Retired runtime and protected payload rows
remain historical; `execution.agent_session_id` remains readable but is no
longer consulted to resume or materialized into `HarnessSession`. The
`CredentialService` retains API-key and OAuth storage, refresh, ownership, and
provider-auth state for its live provider, authorization, and external adapter
consumers.

Focused verification passed: retired embedded routing, Task admission and exact
adapter dispatch, explicit HarnessSession follow-up, adapter cancellation,
external Agent Chat, durable native-chat failure, historical session read with
interaction rejection, provider credential redaction, project chat binding,
legacy `execution.agent_session_id` rejection, one PR6 durable wake/dispatch
regression, and representative PR9 lifecycle, Gate, and exact retry tests.
`cargo check -p services -p api --locked --offline`,
`cargo fmt --all -- --check`, and `git diff --check` passed. Static source and
manifest searches found no production dependency or invocation of
`agent-runtime`, `forge-agent-host`, `NativeAgentRuntimeBackend`, or
`EmbeddedTaskExecutor`.

The shared target was 13 GB before focal compilation and 23 GB afterward;
available disk changed from 79 GB to 70 GB. No clean, workspace build, workspace
test suite, or workspace clippy run was performed.

## Focused verification plan

Add or update focused checks for:

* static production references/dependencies for `agent-runtime`,
  `forge-agent-host`, `NativeAgentRuntimeBackend`, and `EmbeddedTaskExecutor`;
* historical `embedded`/native profile read plus explicit failed execution,
  with no alternate Agent, harness, account, or credential;
* Task Start/Resume dispatch through the exact HarnessAdapter and explicit
  HarnessSession; cancellation/recovery never consults AgentSession;
* external Main/Project Agent Chat success and deterministic native binding
  failure persisted by the existing job worker, with zero model invocation;
* one durable orchestrator wake → exact Execution → adapter dispatch path;
* a small representative PR9 lifecycle, Gate, and exact retry subset.

Use the shared target directory
`/home/alejandro/Proyectos/forge/target`; capture `df -h .` and target size
before targeted compilation. Run `git diff --check`, formatting checks, and
only targeted offline/locked compilation and focal tests. Do not clean the
target or run workspace-wide build/test/clippy commands.
